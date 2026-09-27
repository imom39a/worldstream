//! Authenticated local Controller/Runtime backend for Agent Swarm.

use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worldstream_core::{Blake3DigestV1, CanonicalJsonV1, validate_pack_schema_document};
use worldstream_protocol::{
    ACTIVITY_PACK_CATALOG_VERSION, AccessMode, ActivityPackCatalogRevisionResponse, PackReference,
    PrincipalKind, UlidString,
};
use worldstream_runtime::{
    create_owner_only_renameable_file, prepare_data_directory, validate_owner_only_file,
};
use worldstream_studio_supervisor::{
    operator_connection::OperatorConnection,
    participant_handoff::{
        FixedDaemonParticipantConsoleGatewayV1, HumanSeatAuthorityV1, ParticipantActionRequestV1,
        ParticipantConsoleGatewayV1,
    },
    room_drafts::AgentAssignmentModeV1,
    room_setup_operations::{
        RoomSetupCreateRequestV1, RoomSetupOperationStageV1, RoomSetupOperationStatusV1,
    },
    room_setup_spec::{RoomSetupSpecificationV1, SetupAssignmentV1, SetupPrincipalV1, SetupSeatV1},
    scoped_connections::MembershipCredentialsV1,
};

use crate::{
    backend::{
        BackendError, ExactSwarmAction, SwarmActionOffer, SwarmActionReceipt, SwarmActor,
        SwarmBackend, SwarmObservation,
    },
    domain::{
        AcceptanceCriterion, CreateSwarm, MemberConfiguration, SwarmId, SwarmSummary, SwarmView,
        ValidatedCreateSwarm,
    },
};

const INTENT_SCHEMA: &str = "worldstream/agent-swarm-managed-local-intent/v1";
const RECEIPT_SCHEMA: &str = "worldstream/agent-swarm-managed-local-room-receipt/v1";
const SETUP_STATUS_VERSION: &str = "room_setup_operation.v1";
const MEMBERSHIP_CREDENTIALS_SCHEMA: &str = "worldstream/membership-credentials/v1";
const ACTION_OFFER_DOMAIN: &str = "worldstream/action-offer/v1";
const SUPPORTED_PACK_ID: &str = "worldstream.agent-swarm";
const SUPPORTED_PACK_VERSIONS: &[&str] = &["0.1.0", "0.2.0"];
const SOURCE_LABEL: &str = "authoritative WorldStream participant projection";
const HUMAN_SEAT: &str = "human-coordinator";
const MAX_PACK_TEXT_BYTES: usize = 4 * 1024;
const MAX_PACK_WORKERS: usize = 16;
const MAX_STORE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RESUME_ATTEMPTS: usize = 32;

/// Production backend using an already initialized and running managed Controller/Runtime.
pub struct ManagedLocalWorldStreamBackend {
    root: PathBuf,
    creation_pack: PackReference,
    runtime_url: String,
    control: Box<dyn LocalControlPlane>,
}

trait LocalControlPlane: Send + Sync {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), BackendError>;
    fn observe(
        &self,
        credentials: MembershipCredentialsV1,
        actor: &SwarmActor,
    ) -> Result<Value, BackendError>;
    fn act(
        &self,
        credentials: MembershipCredentialsV1,
        actor: &SwarmActor,
        request: &ParticipantActionRequestV1,
    ) -> Result<Value, BackendError>;
}

struct AuthenticatedLocalControlPlane {
    controller: OperatorConnection,
    runtime_address: SocketAddr,
    timeout: Duration,
}

impl LocalControlPlane for AuthenticatedLocalControlPlane {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), BackendError> {
        let response = self
            .controller
            .request(method, path, body)
            .map_err(|_| BackendError::StorageUnavailable)?;
        Ok((response.status, response.body))
    }

    fn observe(
        &self,
        credentials: MembershipCredentialsV1,
        actor: &SwarmActor,
    ) -> Result<Value, BackendError> {
        let authority = participant_authority(credentials, actor)?;
        FixedDaemonParticipantConsoleGatewayV1::new(self.runtime_address, self.timeout)
            .observe(&authority, None)
            .map(|observed| observed.browser_value)
            .map_err(|_| BackendError::AuthorityUnavailable)
    }

    fn act(
        &self,
        credentials: MembershipCredentialsV1,
        actor: &SwarmActor,
        request: &ParticipantActionRequestV1,
    ) -> Result<Value, BackendError> {
        let authority = participant_authority(credentials, actor)?;
        FixedDaemonParticipantConsoleGatewayV1::new(self.runtime_address, self.timeout)
            .act(&authority, None, request)
            .map_err(|_| BackendError::ActionUncertain)
    }
}

fn participant_authority(
    credentials: MembershipCredentialsV1,
    actor: &SwarmActor,
) -> Result<HumanSeatAuthorityV1, BackendError> {
    let (role, kind) = actor_role_kind(actor);
    HumanSeatAuthorityV1::new_for_principal(
        &credentials.room_id,
        &credentials.member_id,
        credentials.pack,
        AccessMode::Participant,
        Some(role.to_owned()),
        kind,
        credentials.bearer,
    )
    .map_err(|_| BackendError::AuthorityUnavailable)
}

fn actor_role_kind(actor: &SwarmActor) -> (&'static str, PrincipalKind) {
    match actor {
        SwarmActor::HumanCoordinator => ("human_coordinator", PrincipalKind::Human),
        SwarmActor::Worker { .. } => ("worker", PrincipalKind::Agent),
    }
}

impl ManagedLocalWorldStreamBackend {
    /// Opens the protected application index and proof-bound Controller connection.
    ///
    /// # Errors
    /// Rejects unsafe local state or Controller selections.
    pub fn open(
        state_dir: &Path,
        controller_address: SocketAddr,
        runtime_address: SocketAddr,
        pack: PackReference,
        timeout: Duration,
    ) -> Result<Self, BackendError> {
        if !runtime_address.ip().is_loopback()
            || runtime_address.port() == 0
            || !pack_is_supported(&pack)
        {
            return Err(BackendError::InvalidData);
        }
        let controller = OperatorConnection::open(state_dir, controller_address, timeout)
            .map_err(|_| BackendError::StorageUnavailable)?;
        let app_dir = prepare_data_directory(&state_dir.join("agent-swarm"))
            .map_err(|_| BackendError::StorageUnavailable)?;
        Ok(Self {
            root: app_dir,
            creation_pack: pack,
            runtime_url: format!("ws://{runtime_address}/v1/stream"),
            control: Box::new(AuthenticatedLocalControlPlane {
                controller,
                runtime_address,
                timeout,
            }),
        })
    }

    fn intent_path(&self, operation: &str) -> PathBuf {
        self.root.join(format!("{operation}.intent.json"))
    }

    fn receipt_path(&self, operation: &str) -> PathBuf {
        self.root.join(format!("{operation}.room.json"))
    }

    fn create_document<T: Serialize>(&self, path: &Path, document: &T) -> Result<(), BackendError> {
        let bytes = serde_json::to_vec_pretty(document).map_err(|_| BackendError::InvalidData)?;
        if bytes.len() as u64 > MAX_STORE_BYTES {
            return Err(BackendError::InvalidData);
        }
        let mut nonce = [0_u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| BackendError::IdentityUnavailable)?;
        let suffix = lowercase_hex(&nonce);
        let temporary = self.root.join(format!(".{suffix}.staged"));
        let mut file = create_owner_only_renameable_file(&temporary)
            .map_err(|_| BackendError::StorageUnavailable)?;
        let result = (|| {
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| BackendError::StorageUnavailable)?;
            drop(file);
            publish_create_new(&temporary, path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    BackendError::IdentityUnavailable
                } else {
                    BackendError::StorageUnavailable
                }
            })
        })();
        let _ = fs::remove_file(temporary);
        result
    }

    fn read_document<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, BackendError> {
        validate_owner_only_file(path).map_err(|_| BackendError::StorageUnavailable)?;
        let metadata = fs::metadata(path).map_err(|_| BackendError::StorageUnavailable)?;
        if metadata.len() > MAX_STORE_BYTES {
            return Err(BackendError::InvalidData);
        }
        serde_json::from_slice(&fs::read(path).map_err(|_| BackendError::StorageUnavailable)?)
            .map_err(|_| BackendError::InvalidData)
    }

    fn records(&self) -> Result<Vec<ManagedRecord>, BackendError> {
        let mut paths = fs::read_dir(&self.root)
            .map_err(|_| BackendError::StorageUnavailable)?
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|_| BackendError::StorageUnavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;
        paths.retain(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".intent.json"))
        });
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let record: ManagedRecord = Self::read_document(&path)?;
                if record.schema != INTENT_SCHEMA
                    || self.intent_path(record.operation.as_str()) != path
                    || SwarmId::new(record.operation.clone()).as_ref() != Ok(&record.swarm_id)
                    || !pack_is_supported(&record.pack)
                    || !request_is_valid(&record.request)
                {
                    return Err(BackendError::InvalidData);
                }
                Ok(record)
            })
            .collect()
    }

    fn room_receipt(&self, record: &ManagedRecord) -> Result<Option<RoomReceipt>, BackendError> {
        let path = self.receipt_path(&record.operation);
        match fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(BackendError::StorageUnavailable),
            Ok(_) => {
                let receipt: RoomReceipt = Self::read_document(&path)?;
                if receipt.schema != RECEIPT_SCHEMA
                    || receipt.operation != record.operation
                    || receipt.room_id.parse::<UlidString>().is_err()
                {
                    return Err(BackendError::InvalidData);
                }
                Ok(Some(receipt))
            }
        }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), BackendError> {
        self.control.request(method, path, body)
    }

    fn record(&self, swarm_id: &SwarmId) -> Result<ManagedRecord, BackendError> {
        self.records()?
            .into_iter()
            .find(|record| &record.swarm_id == swarm_id)
            .ok_or(BackendError::NotFound)
    }

    fn credentials_for_seat(
        &self,
        record: &ManagedRecord,
        room_id: &str,
        seat: &str,
        expected_role: &str,
    ) -> Result<MembershipCredentialsV1, BackendError> {
        let credentials_path = format!(
            "/api/v1/room-setup-operations/{}/seats/{seat}/membership-credentials",
            record.operation
        );
        let (code, body) = self.request("POST", &credentials_path, &[])?;
        if code != 200 {
            return Err(if code >= 500 || code == 429 {
                BackendError::StorageUnavailable
            } else {
                BackendError::AuthorityUnavailable
            });
        }
        let credentials: MembershipCredentialsV1 =
            serde_json::from_slice(&body).map_err(|_| BackendError::InvalidData)?;
        let expected_scopes: &[&str] = match expected_role {
            "human_coordinator" => &[
                "room:attach",
                "room:act",
                "room:observe_member",
                "room:replay",
            ],
            "worker" => &["room:attach", "room:act", "room:observe_member"],
            _ => return Err(BackendError::InvalidData),
        };
        if credentials.schema != MEMBERSHIP_CREDENTIALS_SCHEMA
            || credentials.operation != record.operation
            || credentials.seat != *seat
            || credentials.room_id != room_id
            || credentials.role != expected_role
            || credentials.pack != record.pack
            || credentials.runtime_url != self.runtime_url
            || credentials.member_id.parse::<UlidString>().is_err()
            || credentials.principal_id.parse::<UlidString>().is_err()
            || !credentials
                .scopes
                .iter()
                .map(String::as_str)
                .eq(expected_scopes.iter().copied())
        {
            return Err(BackendError::InvalidData);
        }
        Ok(credentials)
    }

    fn membership_credentials(
        &self,
        record: &ManagedRecord,
        room_id: &str,
        actor: &SwarmActor,
    ) -> Result<MembershipCredentialsV1, BackendError> {
        match actor {
            SwarmActor::HumanCoordinator => {
                self.credentials_for_seat(record, room_id, HUMAN_SEAT, "human_coordinator")
            }
            SwarmActor::Worker { member_key } => {
                let roster_index = record
                    .request
                    .roster
                    .iter()
                    .position(|member| &member.member_key == member_key)
                    .ok_or(BackendError::AuthorityUnavailable)?;

                // The Pack binds configuration roster entries to sorted
                // enabled worker Member IDs. Setup seat order is an
                // application concern, so reproduce that exact binding here.
                let mut credentials = (0..record.request.roster.len())
                    .map(|index| {
                        self.credentials_for_seat(
                            record,
                            room_id,
                            &format!("worker-{}", index + 1),
                            "worker",
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                credentials.sort_by(|left, right| left.member_id.cmp(&right.member_id));
                if credentials
                    .windows(2)
                    .any(|pair| pair[0].member_id == pair[1].member_id)
                {
                    return Err(BackendError::InvalidData);
                }
                credentials
                    .into_iter()
                    .nth(roster_index)
                    .ok_or(BackendError::AuthorityUnavailable)
            }
        }
    }

    fn observe_record(
        &self,
        record: &ManagedRecord,
        room_id: &str,
        actor: &SwarmActor,
    ) -> Result<(Value, String), BackendError> {
        let credentials = self.membership_credentials(record, room_id, actor)?;
        let member_id = credentials.member_id.clone();
        let observed = self.control.observe(credentials, actor)?;
        let observed_pack: PackReference = serde_json::from_value(
            observed
                .get("pack")
                .cloned()
                .ok_or(BackendError::InvalidData)?,
        )
        .map_err(|_| BackendError::InvalidData)?;
        if observed_pack != record.pack {
            return Err(BackendError::InvalidData);
        }
        Ok((observed, member_id))
    }

    fn reconcile_record(&self, record: &ManagedRecord) -> Result<SwarmView, BackendError> {
        let path = format!("/api/v1/room-setup-operations/{}", record.operation);
        let (status_code, body) = self.request("GET", &path, &[])?;
        let mut status = if status_code == 404 {
            let request = Self::setup_request(record)?;
            let bytes = serde_json::to_vec(&request).map_err(|_| BackendError::InvalidData)?;
            let created = self.request("POST", &path, &bytes)?;
            if created.0 == 409 {
                Self::decode_status(&self.request("GET", &path, &[])?, &record.operation)?
            } else {
                Self::decode_status(&created, &record.operation)?
            }
        } else {
            Self::decode_status(&(status_code, body), &record.operation)?
        };
        for _ in 0..MAX_RESUME_ATTEMPTS {
            if status.complete {
                break;
            }
            let resume = format!("{path}/resume");
            status = Self::decode_status(&self.request("POST", &resume, &[])?, &record.operation)?;
        }
        if !status.complete {
            return Err(BackendError::StorageUnavailable);
        }
        let room_id = status.room_id.clone().ok_or(BackendError::InvalidData)?;
        if let Some(existing) = self.room_receipt(record)? {
            if existing.room_id != room_id {
                return Err(BackendError::InvalidData);
            }
        } else {
            self.create_document(
                &self.receipt_path(&record.operation),
                &RoomReceipt {
                    schema: RECEIPT_SCHEMA.to_owned(),
                    operation: record.operation.clone(),
                    room_id: room_id.clone(),
                },
            )?;
        }
        let (observed, _) = self.observe_record(record, &room_id, &SwarmActor::HumanCoordinator)?;
        let view = view_from_observation(&record.swarm_id, &room_id, &observed)?;
        if view.goal != record.request.goal
            || view.constraints != record.request.constraints
            || view.acceptance_criteria != record.request.acceptance_criteria
            || view.working_area != record.request.working_area
            || !same_roster_identity(&view.roster, &record.request.roster)
        {
            return Err(BackendError::InvalidData);
        }
        Ok(view)
    }

    fn decode_status(
        response: &(u16, Vec<u8>),
        expected_operation: &str,
    ) -> Result<RoomSetupOperationStatusV1, BackendError> {
        if !matches!(response.0, 200 | 202) {
            return Err(if response.0 >= 500 || response.0 == 429 {
                BackendError::StorageUnavailable
            } else {
                BackendError::InvalidData
            });
        }
        let status: RoomSetupOperationStatusV1 =
            serde_json::from_slice(&response.1).map_err(|_| BackendError::InvalidData)?;
        if status.version != SETUP_STATUS_VERSION
            || status.operation != expected_operation
            || status
                .room_id
                .as_deref()
                .is_some_and(|room_id| room_id.parse::<UlidString>().is_err())
            || status.complete != matches!(status.stage, RoomSetupOperationStageV1::Complete)
            || (status.complete && status.room_id.is_none())
        {
            return Err(BackendError::InvalidData);
        }
        Ok(status)
    }

    fn setup_request(record: &ManagedRecord) -> Result<RoomSetupCreateRequestV1, BackendError> {
        if !pack_is_supported(&record.pack)
            || record.request.goal.len() > MAX_PACK_TEXT_BYTES
            || record.request.roster.len() > MAX_PACK_WORKERS
        {
            return Err(BackendError::InvalidData);
        }
        let configuration = configuration_value(&record.request)?;
        let mut seats = Vec::with_capacity(record.request.roster.len() + 1);
        seats.push(SetupSeatV1 {
            label: HUMAN_SEAT.to_owned(),
            role: "human_coordinator".to_owned(),
            required: true,
            display_name: "Human coordinator".to_owned(),
            principal: Some(SetupPrincipalV1 {
                reference: "human-coordinator-principal".to_owned(),
                kind: PrincipalKind::Human,
            }),
            assignment: None,
        });
        for (index, member) in record.request.roster.iter().enumerate() {
            let ordinal = index + 1;
            seats.push(SetupSeatV1 {
                label: format!("worker-{ordinal}"),
                role: "worker".to_owned(),
                required: true,
                display_name: member.label.clone(),
                principal: Some(SetupPrincipalV1 {
                    reference: format!("worker-{ordinal}-principal"),
                    kind: PrincipalKind::Agent,
                }),
                assignment: Some(SetupAssignmentV1 {
                    mode: AgentAssignmentModeV1::External,
                    agent_profile: None,
                    runner_template: None,
                }),
            });
        }
        Ok(RoomSetupCreateRequestV1 {
            acknowledge_start: true,
            specification: RoomSetupSpecificationV1 {
                schema: "worldstream/room-setup/v1".to_owned(),
                pack: record.pack.clone(),
                configuration,
                seats,
                spectators: Vec::new(),
                operator_view: false,
            },
        })
    }
}

impl SwarmBackend for ManagedLocalWorldStreamBackend {
    fn create(&mut self, request: ValidatedCreateSwarm) -> Result<SwarmView, BackendError> {
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(|_| BackendError::IdentityUnavailable)?;
        let suffix = lowercase_hex(&random);
        let operation = format!("swarm-{suffix}");
        let swarm_id =
            SwarmId::new(operation.clone()).map_err(|_| BackendError::IdentityUnavailable)?;
        let record = ManagedRecord {
            schema: INTENT_SCHEMA.to_owned(),
            swarm_id: swarm_id.clone(),
            operation,
            pack: self.creation_pack.clone(),
            request,
        };
        // Reject unsupported or non-UTF-8 configuration before retaining an
        // intent that this backend could never reconcile.
        let _ = Self::setup_request(&record)?;
        self.create_document(&self.intent_path(&record.operation), &record)?;
        self.reconcile_record(&record)
    }

    fn list(&self) -> Result<Vec<SwarmSummary>, BackendError> {
        let records = self.records()?;
        let mut summaries = Vec::with_capacity(records.len());
        for record in &records {
            if let Some(receipt) = self.room_receipt(record)? {
                summaries.push(SwarmSummary {
                    swarm_id: record.swarm_id.clone(),
                    room_id: receipt.room_id,
                    goal: record.request.goal.clone(),
                    member_count: record.request.roster.len(),
                });
            } else {
                summaries.push(self.reconcile_record(record)?.summary());
            }
        }
        Ok(summaries)
    }

    fn open(&self, swarm_id: &SwarmId) -> Result<SwarmView, BackendError> {
        self.reconcile_record(&self.record(swarm_id)?)
    }

    fn observe(
        &self,
        swarm_id: &SwarmId,
        actor: &SwarmActor,
    ) -> Result<SwarmObservation, BackendError> {
        let record = self.record(swarm_id)?;
        let swarm = self.reconcile_record(&record)?;
        let (observed, member_id) = self.observe_record(&record, &swarm.room_id, actor)?;
        observation_from_value(swarm, actor.clone(), member_id, &observed)
    }

    fn action_payload_schema(
        &self,
        swarm_id: &SwarmId,
        offer: &SwarmActionOffer,
    ) -> Result<Value, BackendError> {
        let record = self.record(swarm_id)?;
        let path = format!("/api/v1/activity-packs/{}", record.pack.digest);
        let (status, bytes) = self.request("GET", &path, &[])?;
        if status != 200 || bytes.len() > 1024 * 1024 {
            return Err(BackendError::AuthorityUnavailable);
        }
        let response: ActivityPackCatalogRevisionResponse =
            serde_json::from_slice(&bytes).map_err(|_| BackendError::InvalidData)?;
        schema_from_catalog(response, &record.pack, offer)
    }

    fn submit(
        &mut self,
        swarm_id: &SwarmId,
        request: &ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, BackendError> {
        if request.action_id.parse::<UlidString>().is_err()
            || request.action_type.is_empty()
            || request.action_type.len() > 128
            || request.offer_id.is_empty()
            || request.offer_id.len() > 256
            || !digest_is_valid(&request.payload_schema_digest)
            || serde_json::to_vec(&request.payload)
                .map_err(|_| BackendError::InvalidData)?
                .len()
                > 64 * 1024
        {
            return Err(BackendError::InvalidData);
        }
        let record = self.record(swarm_id)?;
        let swarm = self.reconcile_record(&record)?;
        let credentials = self.membership_credentials(&record, &swarm.room_id, &request.actor)?;
        let value = self.control.act(
            credentials,
            &request.actor,
            &ParticipantActionRequestV1 {
                action_id: request.action_id.clone(),
                based_on_room_seq: request.based_on_room_seq,
                offer_id: request.offer_id.clone(),
                schema_digest: request.payload_schema_digest.clone(),
                action_type: request.action_type.clone(),
                payload: request.payload.clone(),
            },
        )?;
        action_receipt(&value, &request.action_id)
    }
}

fn schema_from_catalog(
    response: ActivityPackCatalogRevisionResponse,
    pack: &PackReference,
    offer: &SwarmActionOffer,
) -> Result<Value, BackendError> {
    if response.version != ACTIVITY_PACK_CATALOG_VERSION || &response.revision.summary.pack != pack
    {
        return Err(BackendError::InvalidData);
    }
    let mut matching = response
        .revision
        .actions
        .into_iter()
        .filter(|action| action.action_type == offer.action_type);
    let action = matching.next().ok_or(BackendError::InvalidData)?;
    if matching.next().is_some()
        || action.payload_schema.schema_digest != offer.payload_schema_digest
    {
        return Err(BackendError::InvalidData);
    }
    let schema_bytes =
        serde_json::to_vec(&action.payload_schema.schema).map_err(|_| BackendError::InvalidData)?;
    let canonical = CanonicalJsonV1::parse(&schema_bytes)
        .and_then(|value| value.to_bytes())
        .map_err(|_| BackendError::InvalidData)?;
    if Blake3DigestV1::hash(&canonical).to_string() != offer.payload_schema_digest {
        return Err(BackendError::InvalidData);
    }
    validate_pack_schema_document(&action.payload_schema.schema)
        .map_err(|_| BackendError::InvalidData)?;
    Ok(action.payload_schema.schema)
}

fn configuration_value(request: &ValidatedCreateSwarm) -> Result<Value, BackendError> {
    let working_area = request
        .working_area
        .to_str()
        .filter(|value| value.len() <= MAX_PACK_TEXT_BYTES)
        .ok_or(BackendError::InvalidData)?;
    let roster = request
        .roster
        .iter()
        .map(|member| {
            let mut value = json!({
                "member_key": member.member_key,
                "label": member.label,
                "provider": member.provider,
                "requested_model": member.requested_model,
                "moving_alias_acknowledged": member.moving_alias_acknowledged,
                "configuration_state": member.configuration_state,
            });
            if let Some(effort) = &member.requested_effort {
                value["requested_effort"] = json!(effort);
            }
            value
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "goal": request.goal,
        "constraints": request.constraints,
        "acceptance_criteria": request.acceptance_criteria.iter().map(|criterion| criterion.text.as_str()).collect::<Vec<_>>(),
        "approved_working_area": { "root": working_area, "resource_paths": [] },
        "roster": roster,
        "progress_review_interval_seconds": request.progress_review_interval_seconds,
        "correction_failure_limit": request.correction_failure_limit,
    }))
}

fn same_roster_identity(current: &[MemberConfiguration], created: &[MemberConfiguration]) -> bool {
    current.len() == created.len()
        && current.iter().zip(created).all(|(current, created)| {
            current.member_key == created.member_key && current.label == created.label
        })
}

fn request_is_valid(request: &ValidatedCreateSwarm) -> bool {
    let candidate = CreateSwarm {
        goal: request.goal.clone(),
        constraints: request.constraints.clone(),
        acceptance_criteria: request.acceptance_criteria.clone(),
        working_area: request.working_area.clone(),
        roster: request.roster.clone(),
        progress_review_interval_seconds: request.progress_review_interval_seconds,
        correction_failure_limit: request.correction_failure_limit,
    };
    ValidatedCreateSwarm::try_from(candidate).is_ok_and(|validated| &validated == request)
}

fn pack_is_supported(pack: &PackReference) -> bool {
    pack.id == SUPPORTED_PACK_ID
        && SUPPORTED_PACK_VERSIONS.contains(&pack.version.as_str())
        && digest_is_valid(&pack.digest)
}

fn digest_is_valid(digest: &str) -> bool {
    digest.len() == 71
        && digest.starts_with("blake3:")
        && digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn lowercase_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn view_from_observation(
    swarm_id: &SwarmId,
    room_id: &str,
    observed: &Value,
) -> Result<SwarmView, BackendError> {
    let reset = projection_reset(observed)?;
    let activity = reset
        .pointer("/body/projection/activity")
        .and_then(Value::as_object)
        .ok_or(BackendError::InvalidData)?;
    view_from_activity(swarm_id, room_id, activity)
}

fn projection_reset(observed: &Value) -> Result<&Value, BackendError> {
    observed
        .get("delivery")
        .and_then(Value::as_array)
        .ok_or(BackendError::InvalidData)?
        .iter()
        .find(|delivery| delivery.get("kind").and_then(Value::as_str) == Some("projection_reset"))
        .ok_or(BackendError::InvalidData)
}

fn view_from_activity(
    swarm_id: &SwarmId,
    room_id: &str,
    activity: &serde_json::Map<String, Value>,
) -> Result<SwarmView, BackendError> {
    let goal = required_string(activity.get("goal"))?;
    let constraints = string_array(activity.get("constraints"))?;
    let criteria = string_array(activity.get("acceptance_criteria"))?
        .into_iter()
        .map(|text| AcceptanceCriterion { text })
        .collect();
    let root = activity
        .get("approved_working_area")
        .and_then(|area| area.get("root"))
        .and_then(Value::as_str)
        .ok_or(BackendError::InvalidData)?;
    let roster_values = activity
        .get("roster")
        .and_then(Value::as_array)
        .ok_or(BackendError::InvalidData)?;
    let roster = roster_values
        .iter()
        .map(|value| {
            let mut value = value.clone();
            value
                .as_object_mut()
                .ok_or(BackendError::InvalidData)?
                .remove("member_id");
            serde_json::from_value(value).map_err(|_| BackendError::InvalidData)
        })
        .collect::<Result<Vec<MemberConfiguration>, BackendError>>()?;
    let progress_review_interval_seconds = activity
        .get("progress_review_interval_seconds")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(BackendError::InvalidData)?;
    let correction_failure_limit = activity
        .get("correction_failure_limit")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(BackendError::InvalidData)?;
    Ok(SwarmView {
        swarm_id: swarm_id.clone(),
        room_id: room_id.to_owned(),
        goal,
        constraints,
        acceptance_criteria: criteria,
        working_area: PathBuf::from(root),
        roster,
        progress_review_interval_seconds,
        correction_failure_limit,
        source_label: SOURCE_LABEL.to_owned(),
    })
}

fn observation_from_value(
    swarm: SwarmView,
    actor: SwarmActor,
    member_id: String,
    observed: &Value,
) -> Result<SwarmObservation, BackendError> {
    let reset = projection_reset(observed)?;
    let activity = reset
        .pointer("/body/projection/activity")
        .cloned()
        .ok_or(BackendError::InvalidData)?;
    if !activity.is_object() {
        return Err(BackendError::InvalidData);
    }
    let head = observed
        .get("room_head")
        .and_then(Value::as_object)
        .or_else(|| reset.pointer("/body/room_head").and_then(Value::as_object))
        .ok_or(BackendError::InvalidData)?;
    let room_seq = head
        .get("room_seq")
        .and_then(Value::as_u64)
        .ok_or(BackendError::InvalidData)?;
    let authoritative_state_hash = required_string(head.get("authoritative_state_hash"))?;
    let offers = reset
        .pointer("/body/projection/action_offers")
        .and_then(Value::as_array)
        .ok_or(BackendError::InvalidData)?
        .iter()
        .enumerate()
        .map(|(index, offer)| {
            let offer = offer.as_object().ok_or(BackendError::InvalidData)?;
            if offer.get("domain").and_then(Value::as_str) != Some(ACTION_OFFER_DOMAIN) {
                return Err(BackendError::InvalidData);
            }
            let payload_schema_digest = required_string(offer.get("payload_schema_digest"))?;
            Ok(SwarmActionOffer {
                // Core's participant projection carries ordered Action offers,
                // not a separate opaque identity. Reproduce the exact offer
                // identity used by the managed Action gateway.
                offer_id: format!("{room_seq}:{index}:{payload_schema_digest}"),
                action_type: required_string(offer.get("action_type"))?,
                payload_schema_digest,
            })
        })
        .collect::<Result<Vec<_>, BackendError>>()?;
    Ok(SwarmObservation {
        swarm,
        actor,
        member_id,
        room_seq,
        authoritative_state_hash,
        action_offers: offers,
        activity,
    })
}

fn action_receipt(
    value: &Value,
    expected_action_id: &str,
) -> Result<SwarmActionReceipt, BackendError> {
    let status = value
        .get("state")
        .and_then(Value::as_str)
        .ok_or(BackendError::InvalidData)?;
    let receipt = value
        .get("receipt")
        .and_then(Value::as_object)
        .ok_or(BackendError::InvalidData)?;
    let action_id = required_string(receipt.get("action_id"))?;
    if action_id != expected_action_id {
        return Err(BackendError::InvalidData);
    }
    let duplicate = receipt
        .get("duplicate")
        .and_then(Value::as_bool)
        .ok_or(BackendError::InvalidData)?;
    match status {
        "accepted" => Ok(SwarmActionReceipt::Accepted {
            action_id,
            room_seq: receipt
                .get("room_head")
                .and_then(Value::as_object)
                .and_then(|head| head.get("room_seq"))
                .and_then(Value::as_u64)
                .ok_or(BackendError::InvalidData)?,
            duplicate,
        }),
        "rejected" => {
            let outer_code = required_string(receipt.get("code"))?;
            let code = if outer_code == "activity_domain_rejection" {
                required_string(
                    receipt
                        .get("details")
                        .and_then(|details| details.get("declared_code")),
                )?
            } else {
                outer_code
            };
            Ok(SwarmActionReceipt::Rejected {
                action_id,
                code,
                current_room_seq: receipt
                    .get("current_room_seq")
                    .and_then(Value::as_u64)
                    .ok_or(BackendError::InvalidData)?,
                retryable_with_same_action_id: receipt
                    .get("retryable_with_same_action_id")
                    .and_then(Value::as_bool)
                    .ok_or(BackendError::InvalidData)?,
                may_submit_revised_action: receipt
                    .get("may_submit_revised_action")
                    .and_then(Value::as_bool)
                    .ok_or(BackendError::InvalidData)?,
                duplicate,
            })
        }
        _ => Err(BackendError::InvalidData),
    }
}

fn required_string(value: Option<&Value>) -> Result<String, BackendError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(BackendError::InvalidData)
}
fn string_array(value: Option<&Value>) -> Result<Vec<String>, BackendError> {
    value
        .and_then(Value::as_array)
        .ok_or(BackendError::InvalidData)?
        .iter()
        .map(|value| required_string(Some(value)))
        .collect()
}

#[cfg(unix)]
fn publish_create_new(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::hard_link(source, target)?;
    fs::File::open(
        target
            .parent()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?,
    )?
    .sync_all()
}

#[cfg(windows)]
fn publish_create_new(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source
        .to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let target = target
        .to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    atomicwrites::move_atomic(source, target)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedRecord {
    schema: String,
    swarm_id: SwarmId,
    operation: String,
    pack: PackReference,
    request: ValidatedCreateSwarm,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RoomReceipt {
    schema: String,
    operation: String,
    room_id: String,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex, PoisonError},
    };

    use super::*;
    use crate::domain::ProviderConfigurationState;

    const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
    const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
    const WORKER_ONE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC2";
    const WORKER_TWO: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC1";

    #[test]
    fn exact_catalog_schema_rejects_digest_drift_and_invalid_shape() {
        let schema = json!({"type":"object","properties":{"refs":{"type":"array","minItems":1}},"required":["refs"]});
        let digest = format!(
            "blake3:{}",
            blake3::hash(schema.to_string().as_bytes()).to_hex()
        );
        let offer = SwarmActionOffer {
            offer_id: "offer".to_owned(),
            action_type: "submit_candidate".to_owned(),
            payload_schema_digest: digest.clone(),
        };
        let response = |schema: Value, schema_digest: &str| {
            serde_json::from_value::<ActivityPackCatalogRevisionResponse>(json!({
                "version":ACTIVITY_PACK_CATALOG_VERSION,
                "revision":{
                    "summary":{"pack":pack(),"name":"fixture","selectable_for_new_rooms":true,"runnable_for_retained_rooms":true},
                    "roles":[],
                    "configuration_schema":{"schema_id":"configuration","schema_digest":digest,"schema":{"type":"object"}},
                    "actions":[{"action_type":"submit_candidate","payload_schema":{"schema_id":"candidate","schema_digest":schema_digest,"schema":schema}}]
                }
            })).expect("typed catalog response")
        };
        assert_eq!(
            schema_from_catalog(response(schema.clone(), &digest), &pack(), &offer),
            Ok(schema.clone())
        );
        let wrong_digest = format!("blake3:{}", "f".repeat(64));
        assert_eq!(
            schema_from_catalog(response(schema.clone(), &wrong_digest), &pack(), &offer),
            Err(BackendError::InvalidData)
        );
        let invalid_schema = json!({"type":"unrecognized"});
        let invalid_digest = format!(
            "blake3:{}",
            blake3::hash(invalid_schema.to_string().as_bytes()).to_hex()
        );
        let invalid_offer = SwarmActionOffer {
            payload_schema_digest: invalid_digest.clone(),
            ..offer
        };
        assert_eq!(
            schema_from_catalog(
                response(invalid_schema, &invalid_digest),
                &pack(),
                &invalid_offer
            ),
            Err(BackendError::InvalidData)
        );
    }

    struct FakeControl {
        pack: PackReference,
        rooms: Arc<Mutex<BTreeMap<String, (String, Value)>>>,
        accepted_actions: Arc<Mutex<BTreeMap<String, u64>>>,
        room_head: Arc<Mutex<u64>>,
        wrong_seat: bool,
    }

    impl LocalControlPlane for FakeControl {
        fn request(
            &self,
            method: &str,
            path: &str,
            body: &[u8],
        ) -> Result<(u16, Vec<u8>), BackendError> {
            let operation = path
                .split('/')
                .nth(4)
                .ok_or(BackendError::InvalidData)?
                .to_owned();
            if path.ends_with("membership-credentials") {
                let requested_seat = path.split('/').nth(6).ok_or(BackendError::InvalidData)?;
                let worker = requested_seat.starts_with("worker-");
                let member_id = match requested_seat {
                    "worker-1" => WORKER_ONE,
                    "worker-2" => WORKER_TWO,
                    _ => MEMBER,
                };
                let room_id = self
                    .rooms
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&operation)
                    .map(|(room_id, _)| room_id.clone())
                    .ok_or(BackendError::InvalidData)?;
                let scopes = if worker {
                    json!(["room:attach", "room:act", "room:observe_member"])
                } else {
                    json!([
                        "room:attach",
                        "room:act",
                        "room:observe_member",
                        "room:replay"
                    ])
                };
                let document = json!({
                    "schema":"worldstream/membership-credentials/v1", "operation":operation,
                    "seat":if self.wrong_seat { "wrong-seat" } else { requested_seat },
                    "runtime_url":"http://127.0.0.1:1", "room_id":room_id, "member_id":member_id,
                    "principal_id":PRINCIPAL, "pack":self.pack,
                    "role":if worker { "worker" } else { "human_coordinator" },
                    "scopes":scopes,
                    "bearer":"wsb1:abababababababababababababababababababababababababababababababab"
                });
                return Ok((
                    200,
                    serde_json::to_vec(&document).map_err(|_| BackendError::InvalidData)?,
                ));
            }
            let mut rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
            if method == "GET" && !rooms.contains_key(&operation) {
                return Ok((404, Vec::new()));
            }
            if method == "POST" && !path.ends_with("/resume") {
                let request: Value =
                    serde_json::from_slice(body).map_err(|_| BackendError::InvalidData)?;
                let room_id = if rooms.is_empty() {
                    ROOM.to_owned()
                } else {
                    "01ARZ3NDEKTSV4RRFFQ69G5FBW".to_owned()
                };
                rooms.insert(
                    operation.clone(),
                    (room_id, request["specification"]["configuration"].clone()),
                );
            }
            let room_id = rooms
                .get(&operation)
                .map(|(room_id, _)| room_id)
                .ok_or(BackendError::InvalidData)?;
            let status = json!({"version":"room_setup_operation.v1","operation":operation,"room_id":room_id,
                "complete":true,"stage":"complete","active_stage":null,"next_action":"inspect_room"});
            Ok((
                200,
                serde_json::to_vec(&status).map_err(|_| BackendError::InvalidData)?,
            ))
        }

        fn observe(
            &self,
            credentials: MembershipCredentialsV1,
            actor: &SwarmActor,
        ) -> Result<Value, BackendError> {
            let configuration = self
                .rooms
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
                .find(|(room_id, _)| room_id == &credentials.room_id)
                .map(|(_, configuration)| configuration.clone())
                .ok_or(BackendError::InvalidData)?;
            let mut activity = configuration;
            for entry in activity["roster"]
                .as_array_mut()
                .ok_or(BackendError::InvalidData)?
            {
                entry
                    .as_object_mut()
                    .ok_or(BackendError::InvalidData)?
                    .insert("member_id".to_owned(), Value::String(MEMBER.to_owned()));
            }
            Ok(
                json!({"room_head":{"room_seq":*self.room_head.lock().unwrap_or_else(PoisonError::into_inner),"authoritative_state_hash":format!("blake3:{}", "c".repeat(64))},
                "delivery":[{"kind":"projection_reset","body":{"projection":{"activity":activity,"action_offers":[{
                    "domain":ACTION_OFFER_DOMAIN,
                    "action_type":if matches!(actor, SwarmActor::HumanCoordinator) { "confirm_initial_setup" } else { "propose_work_item" },
                    "payload_schema_digest":format!("blake3:{}", "d".repeat(64)),
                    "eligibility_window":null
                }]}}}],
                "credential_room":credentials.room_id, "pack":self.pack}),
            )
        }

        fn act(
            &self,
            _credentials: MembershipCredentialsV1,
            _actor: &SwarmActor,
            request: &ParticipantActionRequestV1,
        ) -> Result<Value, BackendError> {
            if let Some(room_seq) = self
                .accepted_actions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&request.action_id)
                .copied()
            {
                return Ok(json!({
                    "state":"accepted",
                    "receipt":{
                        "action_id":request.action_id,
                        "transition_id":"01ARZ3NDEKTSV4RRFFQ69G5FBZ",
                        "admitted_at":"2026-09-15T12:00:00Z",
                        "room_head":{"room_seq":room_seq},
                        "duplicate":true
                    }
                }));
            }
            let mut room_head = self
                .room_head
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if request.based_on_room_seq != *room_head {
                return Ok(json!({
                    "state":"rejected",
                    "receipt":{
                        "action_id":request.action_id,
                        "code":"stale_room_state",
                        "current_room_seq":*room_head,
                        "retryable_with_same_action_id":false,
                        "may_submit_revised_action":true,
                        "duplicate":false
                    }
                }));
            }
            *room_head += 1;
            let admitted_room_seq = *room_head;
            self.accepted_actions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(request.action_id.clone(), admitted_room_seq);
            Ok(json!({
                "state":"accepted",
                "receipt":{
                    "action_id":request.action_id,
                    "transition_id":"01ARZ3NDEKTSV4RRFFQ69G5FBZ",
                    "admitted_at":"2026-09-15T12:00:00Z",
                    "room_head":{"room_seq":admitted_room_seq},
                    "duplicate":false
                }
            }))
        }
    }

    fn pack() -> PackReference {
        PackReference {
            id: "worldstream.agent-swarm".to_owned(),
            version: "0.1.0".to_owned(),
            digest: format!("blake3:{}", "a".repeat(64)),
        }
    }
    fn request(area: &Path, goal: &str) -> ValidatedCreateSwarm {
        crate::domain::CreateSwarm {
            goal: goal.to_owned(),
            constraints: vec!["bounded".to_owned()],
            acceptance_criteria: vec![AcceptanceCriterion {
                text: "visible".to_owned(),
            }],
            working_area: area.to_owned(),
            roster: vec![MemberConfiguration {
                member_key: "worker-a".to_owned(),
                label: "Worker A".to_owned(),
                provider: "codex".to_owned(),
                requested_model: "fixture".to_owned(),
                requested_effort: None,
                configuration_revision: 1,
                moving_alias_acknowledged: true,
                configuration_state: ProviderConfigurationState::FixtureUnavailable,
            }],
            progress_review_interval_seconds: 300,
            correction_failure_limit: 3,
        }
        .try_into()
        .unwrap_or_else(|error| unreachable!("valid request: {error}"))
    }
    fn backend(root: &Path, wrong_seat: bool) -> ManagedLocalWorldStreamBackend {
        let root =
            prepare_data_directory(root).unwrap_or_else(|error| unreachable!("root: {error}"));
        ManagedLocalWorldStreamBackend {
            root,
            creation_pack: pack(),
            runtime_url: "http://127.0.0.1:1".to_owned(),
            control: Box::new(FakeControl {
                pack: pack(),
                rooms: Arc::new(Mutex::new(BTreeMap::new())),
                accepted_actions: Arc::new(Mutex::new(BTreeMap::new())),
                room_head: Arc::new(Mutex::new(0)),
                wrong_seat,
            }),
        }
    }

    #[test]
    fn creates_reopens_and_keeps_two_goals_in_two_operation_intents() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let mut backend = backend(&directory.path().join("index"), false);
        let mut first_request = request(directory.path(), "First goal");
        first_request.progress_review_interval_seconds = 37;
        first_request.correction_failure_limit = 5;
        let first = backend
            .create(first_request)
            .unwrap_or_else(|error| unreachable!("first: {error}"));
        let reopened = backend
            .open(&first.swarm_id)
            .unwrap_or_else(|error| unreachable!("open: {error}"));
        assert_eq!(reopened.goal, "First goal");
        assert_eq!(reopened.room_id, first.room_id);
        assert_eq!(reopened.progress_review_interval_seconds, 37);
        assert_eq!(reopened.correction_failure_limit, 5);
        let second = backend
            .create(request(directory.path(), "Second goal"))
            .unwrap_or_else(|error| unreachable!("second: {error}"));
        assert_ne!(first.swarm_id, second.swarm_id);
        assert_ne!(first.room_id, second.room_id);
        assert_eq!(
            backend
                .records()
                .unwrap_or_else(|error| unreachable!("records: {error}"))
                .len(),
            2
        );
    }

    #[test]
    fn observes_exact_actor_and_submits_only_against_the_observed_head() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let mut backend = backend(&directory.path().join("actions"), false);
        let created = backend
            .create(request(directory.path(), "Coordinate work"))
            .unwrap_or_else(|error| unreachable!("create: {error}"));

        let human = backend
            .observe(&created.swarm_id, &SwarmActor::HumanCoordinator)
            .unwrap_or_else(|error| unreachable!("observe human: {error}"));
        assert_eq!(human.room_seq, 0);
        assert_eq!(human.actor, SwarmActor::HumanCoordinator);
        assert_eq!(human.action_offers[0].action_type, "confirm_initial_setup");
        let expected_offer = format!("0:0:blake3:{}", "d".repeat(64));
        assert_eq!(human.action_offers[0].offer_id, expected_offer);

        let accepted = backend
            .submit(
                &created.swarm_id,
                &ExactSwarmAction {
                    actor: SwarmActor::HumanCoordinator,
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_owned(),
                    based_on_room_seq: human.room_seq,
                    offer_id: human.action_offers[0].offer_id.clone(),
                    action_type: "confirm_initial_setup".to_owned(),
                    payload_schema_digest: format!("blake3:{}", "d".repeat(64)),
                    payload: json!({}),
                },
            )
            .unwrap_or_else(|error| unreachable!("act: {error}"));
        assert_eq!(
            accepted,
            SwarmActionReceipt::Accepted {
                action_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_owned(),
                room_seq: 1,
                duplicate: false,
            }
        );

        // A caller that lost the first reply can resend the exact retained
        // Action identity after the Room Head advanced and receive the
        // authoritative duplicate receipt.
        let duplicate = backend
            .submit(
                &created.swarm_id,
                &ExactSwarmAction {
                    actor: SwarmActor::HumanCoordinator,
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_owned(),
                    based_on_room_seq: human.room_seq,
                    offer_id: human.action_offers[0].offer_id.clone(),
                    action_type: "confirm_initial_setup".to_owned(),
                    payload_schema_digest: format!("blake3:{}", "d".repeat(64)),
                    payload: json!({}),
                },
            )
            .unwrap_or_else(|error| unreachable!("retry exact Action: {error}"));
        assert_eq!(
            duplicate,
            SwarmActionReceipt::Accepted {
                action_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_owned(),
                room_seq: 1,
                duplicate: true,
            }
        );

        let worker = backend
            .observe(
                &created.swarm_id,
                &SwarmActor::Worker {
                    member_key: "worker-a".to_owned(),
                },
            )
            .unwrap_or_else(|error| unreachable!("observe worker: {error}"));
        assert_eq!(worker.action_offers[0].action_type, "propose_work_item");

        assert_eq!(
            backend.submit(
                &created.swarm_id,
                &ExactSwarmAction {
                    actor: SwarmActor::HumanCoordinator,
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FC1".to_owned(),
                    based_on_room_seq: 9,
                    offer_id: human.action_offers[0].offer_id.clone(),
                    action_type: "confirm_initial_setup".to_owned(),
                    payload_schema_digest: format!("blake3:{}", "d".repeat(64)),
                    payload: json!({}),
                },
            ),
            Ok(SwarmActionReceipt::Rejected {
                action_id: "01ARZ3NDEKTSV4RRFFQ69G5FC1".to_owned(),
                code: "stale_room_state".to_owned(),
                current_room_seq: 1,
                retryable_with_same_action_id: false,
                may_submit_revised_action: true,
                duplicate: false,
            })
        );
    }

    #[test]
    fn maps_member_keys_by_the_packs_sorted_worker_member_ids() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let mut backend = backend(&directory.path().join("sorted-members"), false);
        let mut create = request(directory.path(), "Stable worker mapping");
        create.roster.push(MemberConfiguration {
            member_key: "worker-b".to_owned(),
            label: "Worker B".to_owned(),
            provider: "claude".to_owned(),
            requested_model: "fixture".to_owned(),
            requested_effort: None,
            configuration_revision: 1,
            moving_alias_acknowledged: true,
            configuration_state: ProviderConfigurationState::FixtureUnavailable,
        });
        let created = backend
            .create(create)
            .unwrap_or_else(|error| unreachable!("create: {error}"));
        let record = backend
            .record(&created.swarm_id)
            .unwrap_or_else(|error| unreachable!("record: {error}"));

        let first = backend
            .membership_credentials(
                &record,
                &created.room_id,
                &SwarmActor::Worker {
                    member_key: "worker-a".to_owned(),
                },
            )
            .unwrap_or_else(|error| unreachable!("first: {error}"));
        let second = backend
            .membership_credentials(
                &record,
                &created.room_id,
                &SwarmActor::Worker {
                    member_key: "worker-b".to_owned(),
                },
            )
            .unwrap_or_else(|error| unreachable!("second: {error}"));

        assert_eq!(first.member_id, WORKER_TWO);
        assert_eq!(first.seat, "worker-2");
        assert_eq!(second.member_id, WORKER_ONE);
        assert_eq!(second.seat, "worker-1");
    }

    #[test]
    fn rejects_wrong_human_seat_credentials_and_malformed_projection() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let mut backend = backend(&directory.path().join("wrong-seat"), true);
        assert_eq!(
            backend.create(request(directory.path(), "Denied")),
            Err(BackendError::InvalidData)
        );
        let discoverable = backend
            .list()
            .unwrap_or_else(|error| unreachable!("post-Room list: {error}"));
        assert_eq!(discoverable.len(), 1);
        assert_eq!(discoverable[0].goal, "Denied");

        let swarm_id = SwarmId::new("swarm-shape".to_owned())
            .unwrap_or_else(|error| unreachable!("id: {error}"));
        assert_eq!(
            view_from_observation(&swarm_id, ROOM, &json!({"delivery":[]})),
            Err(BackendError::InvalidData)
        );
        assert_eq!(
            view_from_observation(
                &swarm_id,
                ROOM,
                &json!({"delivery":[{"kind":"projection_reset","body":{"projection":{"activity":{"goal":"x"}}}}]})
            ),
            Err(BackendError::InvalidData)
        );
    }

    #[test]
    fn retains_unreported_provider_selection_without_launching_a_worker() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let mut backend = backend(&directory.path().join("unsupported"), false);
        let mut unresolved = request(directory.path(), "Not configured yet");
        unresolved.roster[0].configuration_state = ProviderConfigurationState::ResolutionUnreported;

        let created = backend
            .create(unresolved)
            .unwrap_or_else(|error| unreachable!("create: {error}"));
        assert_eq!(
            created.roster[0].configuration_state,
            ProviderConfigurationState::ResolutionUnreported
        );
        assert_eq!(
            backend
                .records()
                .unwrap_or_else(|error| unreachable!("records: {error}"))
                .len(),
            1
        );
    }

    #[test]
    fn rejects_setup_status_for_a_different_operation() {
        let response = (
            200,
            serde_json::to_vec(&json!({
                "version": SETUP_STATUS_VERSION,
                "operation": "swarm-different",
                "room_id": ROOM,
                "complete": true,
                "stage": "complete",
                "active_stage": null,
                "next_action": "inspect_room"
            }))
            .unwrap_or_else(|error| unreachable!("status: {error}")),
        );

        assert_eq!(
            ManagedLocalWorldStreamBackend::decode_status(&response, "swarm-expected"),
            Err(BackendError::InvalidData)
        );
    }

    #[test]
    fn omits_an_unselected_effort_from_pack_configuration() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let mut create = request(directory.path(), "No effort selection");
        create.roster[0].requested_effort = None;
        let record = ManagedRecord {
            schema: INTENT_SCHEMA.to_owned(),
            swarm_id: SwarmId::new("swarm-effort".to_owned())
                .unwrap_or_else(|error| unreachable!("id: {error}")),
            operation: "swarm-effort".to_owned(),
            pack: pack(),
            request: create,
        };

        let setup = ManagedLocalWorldStreamBackend::setup_request(&record)
            .unwrap_or_else(|error| unreachable!("setup: {error}"));
        let member = setup.specification.configuration["roster"][0]
            .as_object()
            .unwrap_or_else(|| unreachable!("member configuration"));
        assert!(!member.contains_key("requested_effort"));
    }

    #[test]
    fn retains_historical_pack_support_without_accepting_unknown_revisions() {
        let mut retained = pack();
        assert!(pack_is_supported(&retained));
        retained.version = "0.2.0".to_owned();
        assert!(pack_is_supported(&retained));
        retained.version = "0.3.0".to_owned();
        assert!(!pack_is_supported(&retained));
    }

    #[test]
    fn exposes_the_packs_declared_rejection_code() {
        let action_id = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
        let receipt = action_receipt(
            &json!({
                "state": "rejected",
                "receipt": {
                    "action_id": action_id,
                    "code": "activity_domain_rejection",
                    "current_room_seq": 4,
                    "retryable_with_same_action_id": false,
                    "may_submit_revised_action": true,
                    "duplicate": false,
                    "details": {
                        "declared_code": "work_ineligible",
                        "safe_details": {}
                    }
                }
            }),
            action_id,
        );

        assert_eq!(
            receipt,
            Ok(SwarmActionReceipt::Rejected {
                action_id: action_id.to_owned(),
                code: "work_ineligible".to_owned(),
                current_room_seq: 4,
                retryable_with_same_action_id: false,
                may_submit_revised_action: true,
                duplicate: false,
            })
        );
    }
}
