//! Actual canonical backend cells. Run through the subprocess measurement driver.
#![allow(clippy::wildcard_imports, clippy::too_many_lines)]
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use worldstream_core::*;
use worldstream_postgres::{
    PostgresAdmin, PostgresConnectionConfig, PostgresConnectionPath, PostgresRoomStore,
};
use worldstream_sqlite::SqliteRoomStore;
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const HOST: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH2";
const TIME: &str = "2026-08-15T12:00:00Z";
fn parsed<T: FromStr>(s: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    s.parse()
        .map_err(|e| format!("invalid deterministic fixture: {e}").into())
}
fn canonical(v: &Value) -> Result<CanonicalJsonV1> {
    Ok(CanonicalJsonV1::parse(&serde_json::to_vec(&v)?)?)
}
fn arg(name: &str) -> Result<String> {
    let args: Vec<_> = env::args().collect();
    let i = args
        .iter()
        .position(|a| a == name)
        .ok_or_else(|| format!("missing {name}"))?;
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing {name} value").into())
}
fn id(group: u8, n: u64) -> Result<TransitionId> {
    parsed(&format!("01ARZ3NDEKTSV4RR{group:02}{n:08}"))
}
enum Backend {
    Sqlite(Arc<SqliteRoomStore>),
    Postgres(Arc<PostgresRoomStore>),
}

// Transparent measurement controls select the public full capture or change
// only the final install fence. Verification and SQL install use the adapter.
struct RecoveryProbePort<'a> {
    backend: &'a Backend,
    name: &'a str,
    directory: &'a str,
    force_full: bool,
    change_install_fence: bool,
}
impl CanonicalRoomRecoveryStorage for RecoveryProbePort<'_> {
    fn inspect_recovery_candidate(
        &self,
        room: &RoomId,
    ) -> std::result::Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1> {
        if self.force_full {
            self.backend
                .recovery()
                .inspect_full_recovery_candidate(room)
        } else {
            self.backend.recovery().inspect_recovery_candidate(room)
        }
    }
    fn inspect_full_recovery_candidate(
        &self,
        room: &RoomId,
    ) -> std::result::Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1> {
        self.backend
            .recovery()
            .inspect_full_recovery_candidate(room)
    }
    fn guard_recovery_install(
        &self,
        room: &RoomId,
        head: &CompleteHeadV1,
        generation: IntegrityGenerationV1,
        recovered: &RecoveredRoomMaterializationsV1,
    ) -> std::result::Result<(), RoomRecoveryErrorV1> {
        if self.change_install_fence {
            probe(self.name, self.directory, "fence")
                .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        }
        self.backend
            .recovery()
            .guard_recovery_install(room, head, generation, recovered)
    }
    fn record_recovery_failure(
        &self,
        room: &RoomId,
        head: &CompleteHeadV1,
        generation: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
    ) -> std::result::Result<(), RoomRecoveryErrorV1> {
        self.backend
            .recovery()
            .record_recovery_failure(room, head, generation, disposition)
    }
}
impl Backend {
    fn commit(&self) -> &dyn CanonicalRoomCommitStorage {
        match self {
            Self::Sqlite(s) => s.as_ref(),
            Self::Postgres(s) => s.as_ref(),
        }
    }
    fn recovery(&self) -> &dyn CanonicalRoomRecoveryStorage {
        match self {
            Self::Sqlite(s) => s.as_ref(),
            Self::Postgres(s) => s.as_ref(),
        }
    }
    fn resolver(&self) -> &dyn AuthorizedReceiptResolverV1 {
        match self {
            Self::Sqlite(s) => s.as_ref(),
            Self::Postgres(s) => s.as_ref(),
        }
    }
    fn authority(&self) -> AuthorityV1 {
        match self {
            Self::Sqlite(s) => AuthorityV1::new(s.clone()),
            Self::Postgres(s) => AuthorityV1::new(s.clone()),
        }
    }
    fn frames(&self) -> Result<(RoomIntegrityStateV1, BTreeMap<MemberId, u64>)> {
        match self {
            Self::Sqlite(s) => {
                let f = s
                    .current_room_serving_fence(&parsed(ROOM)?)?
                    .ok_or("missing SQLite fence")?;
                Ok((f.integrity().clone(), f.frame_heads().clone()))
            }
            Self::Postgres(s) => {
                let f = s
                    .current_room_serving_fence(&parsed(ROOM)?)?
                    .ok_or("missing PG fence")?;
                Ok((f.integrity().clone(), f.frame_heads().clone()))
            }
        }
    }
    fn engine(&self) -> Result<String> {
        match self {
            Self::Sqlite(s) => Ok(format!(
                "{} {}",
                s.engine_identity().0,
                s.engine_identity().1
            )),
            Self::Postgres(s) => Ok(s.engine_identity()?.formatted().to_owned()),
        }
    }
}
fn probe(backend: &str, dir: &str, action: &str) -> Result<Value> {
    let o = Command::new("/usr/bin/python3")
        .args([
            "scripts/measure-backend-stream-scaling.py",
            "--probe",
            action,
            "--backend",
            backend,
            "--cell-dir",
            dir,
        ])
        .output()?;
    if !o.status.success() {
        return Err(format!("database probe failed: {action}").into());
    }
    Ok(serde_json::from_slice(&o.stdout)?)
}
fn create(
    b: &Backend,
    registry: &PackRegistryV1,
    digest: PackDigestV1,
    configuration: CanonicalJsonV1,
    format: CanonicalHistoryFormat,
) -> Result<CanonicalRoomTrace> {
    let authority = b.authority();
    let bearer = CapabilityBearerV1::from_bytes([0xA7; 32]);
    authority.bootstrap(
        AuthorityBootstrapV1::new(
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FK0")?,
            parsed(PRINCIPAL)?,
            PrincipalKindV1::Human,
            parsed(HOST)?,
            bearer.token_hash(),
            None,
        )?,
        parsed(TIME)?,
    )?;
    let presented = PresentedCapabilityV1::new(parsed(HOST)?, bearer);
    let identity = AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(PRINCIPAL)?,
        versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
        idempotency_key: "backend-scaling-create".to_owned(),
    };
    let request = RoomCreationRequestWithFormat::new(
        RoomCreationRequestV1::new(
            digest.clone(),
            configuration.clone(),
            vec![InitialMembershipProposalV1::new(
                parsed(PRINCIPAL)?,
                PrincipalKindV1::Human,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter".into()),
            )?],
        ),
        format,
    );
    let RoomCreationIngressV1::Authorized(grant) = authorize_canonical_room_creation_operation(
        &authority,
        b.resolver(),
        &presented,
        &identity,
        &request,
        parsed(TIME)?,
    )?
    else {
        return Err("creation not authorized".into());
    };
    let genesis = registry.prepare_genesis_for_new_room_with_format(
        &PackGenesisRequestV1 {
            room_id: parsed(ROOM)?,
            pack_digest: digest,
            configuration,
            room_seed: parsed(
                "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            )?,
            created_at: parsed(TIME)?,
            initial_core_state: CoreRoomStateV1::active([MembershipV1::new(
                parsed(MEMBER)?,
                parsed(PRINCIPAL)?,
                PrincipalKindV1::Human,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter".into()),
            )?])?,
        },
        format,
    )?;
    let prepared =
        PreparedCanonicalRoomCreation::from_registry_genesis(identity, &request, *grant, genesis)?;
    let out = commit_canonical_room_creation(b.commit(), prepared);
    if out.actor_installation() != ActorInstallationV1::Installed {
        return Err(format!("creation failed: {:?}", out.resolution()).into());
    }
    out.into_parts()
        .2
        .ok_or_else(|| "creation trace absent".into())
}
fn commit_one(
    b: &Backend,
    trace: &mut CanonicalRoomTrace,
    n: u64,
    benchmark: bool,
    integrity: &RoomIntegrityStateV1,
    frames: &BTreeMap<MemberId, u64>,
) -> Result<(Vec<u8>, Value)> {
    // The small retained Counter advances twice. Remaining accepted history
    // consists of legitimate host-admin suspend/resume stimuli, each reduced by
    // the same retained executor. Synthetic state cells use bounded increment.
    let prepared = if benchmark || n <= 2 {
        let action_id: ActionId = parsed(id(11, n)?.as_ref())?;
        let request = ParticipantActionRequestV1::new(
            parsed(ROOM)?,
            parsed(MEMBER)?,
            action_id.clone(),
            trace.head().room_seq(),
            "increment",
            canonical(&json!({}))?,
        );
        let schema = trace
            .retained_pack()
            .descriptor()
            .actions
            .iter()
            .find(|a| a.action_type == "increment")
            .ok_or("increment definition")?
            .payload_schema
            .schema_digest
            .clone();
        let authority = b.authority();
        let ParticipantActionIngressV1::Authorized(grant) = authorize_participant_action_operation(
            &authority,
            b.resolver(),
            &PresentedCapabilityV1::new(
                parsed("01ARZ3NDEKTSV4RRFFQ69G5FH3")?,
                CapabilityBearerV1::from_bytes([0xB8; 32]),
            ),
            &request,
            parsed(TIME)?,
        )?
        else {
            return Err("fresh Action not authorized".into());
        };
        let ParticipantActionAuthorityV1::EnabledParticipant(grant) = *grant else {
            return Err("member not enabled".into());
        };
        let transition =
            trace.prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
                member_id: parsed(MEMBER)?,
                action_id,
                action_type: "increment".into(),
                payload_schema_digest: schema,
                canonical_payload: canonical(&json!({}))?,
                exact_basis_head: trace.head().clone(),
                admitted_at: parsed(TIME)?,
            }))?;
        PreparedCanonicalRoomCommit::for_authorized_action(
            trace,
            &request,
            transition,
            id(12, n)?,
            integrity.generation(),
            grant,
            frames,
        )?
    } else {
        let before = trace
            .core_state()
            .membership(&parsed(MEMBER)?)
            .ok_or("member")?
            .clone();
        let enabled = before.standing() == MembershipStandingV1::Enabled;
        let request = CoreAdministrationRequestV1::new(
            parsed(ROOM)?,
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL)?,
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: format!("measure-toggle-{n}"),
            },
            if enabled {
                CoreProposedKindV1::Suspend
            } else {
                CoreProposedKindV1::Resume
            },
            trace.head().room_seq(),
            "measure_standing_toggle",
            CoreChangeSetV1::one(if enabled {
                MembershipChangeV1::suspend(before)
            } else {
                MembershipChangeV1::resume(before)
            }),
        )?;
        let authority = b.authority();
        let CoreAdministrationIngressV1::Authorized(grant) =
            authorize_core_administration_operation(
                &authority,
                b.resolver(),
                &PresentedCapabilityV1::new(
                    parsed(HOST)?,
                    CapabilityBearerV1::from_bytes([0xA7; 32]),
                ),
                &request,
                parsed(TIME)?,
            )?
        else {
            return Err("fresh Core operation not authorized".into());
        };
        PreparedCanonicalRoomCommit::for_authorized_core_administration(
            trace,
            &request,
            parsed(TIME)?,
            id(13, n)?,
            integrity.generation(),
            *grant,
            frames,
        )?
    };
    let PreparedCanonicalExistingIntent::Advance(advance) = prepared.intent() else {
        return Err("expected accepted Transition".into());
    };
    let bytes = advance.canonical_transition_bytes().to_vec();
    let effects = json!({"events":advance.transition().ordered_domain_events(),"timers":advance.transition().ordered_timer_changes(),"attention":advance.transition().ordered_attention_signals()});
    let out = commit_canonical_existing_room(b.commit(), trace, prepared);
    if out.actor_installation() != ActorInstallationV1::Installed {
        let class = match out.resolution() {
            RoomCommitResolutionV1::GenesisCreated { .. } => "GenesisCreated",
            RoomCommitResolutionV1::TransitionCommitted { .. } => "TransitionCommitted",
            RoomCommitResolutionV1::RejectionRecorded { .. } => "RejectionRecorded",
            RoomCommitResolutionV1::NoChangeRecorded { .. } => "NoChangeRecorded",
            RoomCommitResolutionV1::NotApplicable => "NotApplicable",
            RoomCommitResolutionV1::Reprepare => "Reprepare",
            RoomCommitResolutionV1::Fenced => "Fenced",
            RoomCommitResolutionV1::Conflict { .. } => "Conflict",
            RoomCommitResolutionV1::RetryableKnownAbsent => "RetryableKnownAbsent",
            RoomCommitResolutionV1::Indeterminate => "Indeterminate",
            RoomCommitResolutionV1::Fault => "Fault",
        };
        return Err(format!("commit {n} failed: {class}").into());
    }
    Ok((bytes, effects))
}
fn quantiles(samples: &mut [Duration]) -> Value {
    samples.sort_unstable();
    let q = |p: usize| samples[(samples.len() - 1) * p / 100].as_secs_f64() * 1e3;
    json!({"samples":samples.len(),"p50_ms":q(50),"p95_ms":q(95),"p99_ms":q(99),"max_ms":q(100),"precision":"exact at evenly spaced deterministic sample positions, at most 4096 samples"})
}
fn recovery(
    b: &Backend,
    registry: &PackRegistryV1,
    expected: &CanonicalRoomTrace,
) -> Result<Value> {
    let start = Instant::now();
    let recovered =
        recover_canonical_room_from_storage_with_receipt(b.recovery(), registry, &parsed(ROOM)?)?
            .ok_or("recovery missing")?;
    let elapsed = start.elapsed().as_secs_f64();
    if recovered.trace().head() != expected.head()
        || recovered.trace().activity_state() != expected.activity_state()
    {
        return Err("recovery diverged".into());
    }
    let r = recovered.receipt();
    Ok(
        json!({"elapsed_seconds":elapsed,"path":format!("{:?}",r.path()),"checkpoint_seq":r.checkpoint_room_seq().map(RoomSequenceV1::get),"prefix_records_delivered":r.prefix_transition_records_delivered(),"prefix_records_skipped":r.prefix_transitions_skipped(),"tail_records_delivered":r.tail_transition_records_delivered(),"reducer_callbacks":recovered.trace().activity_callback_count()}),
    )
}
fn stored_audit(
    backend: &str,
    dir: &str,
    genesis: &[u8],
    expected_hash: &str,
    count: u64,
) -> Result<Value> {
    let mut child = Command::new("/usr/bin/python3")
        .args([
            "scripts/measure-backend-stream-scaling.py",
            "--probe",
            "records",
            "--backend",
            backend,
            "--cell-dir",
            dir,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut hash = blake3::Hasher::new();
    hash.update(genesis);
    let mut seen = 0;
    let mut bytes = 0;
    let genesis = VerifiedCanonicalGenesis::from_canonical_bytes(genesis)?;
    let mut head = genesis.record().complete_head();
    for line in BufReader::new(child.stdout.take().ok_or("record stream")?).lines() {
        let line = line?;
        let decoded = (0..line.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&line[i..i + 2], 16))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let record = genesis.record().decode_transition(&decoded)?;
        let checked = VerifiedCanonicalLineageRecord::verify_for_storage(
            &genesis,
            &record.complete_head(),
            &decoded,
        )?;
        checked.verify_successor(&head)?;
        head = checked.head().clone();
        hash.update(&decoded);
        bytes += decoded.len();
        seen += 1;
    }
    if !child.wait()?.success()
        || seen != count
        || hash.finalize().to_hex().as_str() != expected_hash
    {
        return Err("stored record audit failed".into());
    }
    Ok(
        json!({"records":seen,"bytes":bytes,"blake3":expected_hash,"exact_original_bytes_verified":true}),
    )
}

fn existing_recovery(
    b: &Backend,
    registry: &PackRegistryV1,
    backend: &str,
    dir: &str,
    output: &str,
    count: u64,
    format: CanonicalHistoryFormat,
) -> Result<()> {
    let prior: Value = serde_json::from_slice(&fs::read(Path::new(dir).join("cell.json"))?)?;
    if prior["transitions"].as_u64() != Some(count) {
        return Err("existing history does not match original completed cell".into());
    }
    let started = Instant::now();
    let recovered = recover_canonical_room_from_storage_with_receipt(
        &RecoveryProbePort {
            backend: b,
            name: backend,
            directory: dir,
            force_full: true,
            change_install_fence: false,
        },
        registry,
        &parsed(ROOM)?,
    )?
    .ok_or("existing full recovery missing")?;
    let elapsed = started.elapsed().as_secs_f64();
    let receipt = recovered.receipt();
    let mut trace = recovered.into_trace();
    let callbacks = trace.activity_callback_count();
    let retained_records = trace.transitions().len();
    let genesis = trace.genesis_bytes()?;
    if receipt.path() != RoomRecoveryExecutionPathV1::Full
        || receipt.prefix_transition_records_delivered() != count
        || u64::try_from(callbacks)? != count
        || u64::try_from(retained_records)? != count
        || trace.head().core_state_hash().to_string() != prior["final_core_hash"]
        || trace.head().activity_state_hash().to_string() != prior["final_activity_hash"]
        || VerifiedCanonicalGenesis::from_canonical_bytes(&genesis)?
            .record()
            .format()
            != format
    {
        return Err("existing recovery changed logical results or full work accounting".into());
    }
    trace.discard_persisted_history();
    let original_hash = prior["original_record_audit"]["blake3"]
        .as_str()
        .ok_or("original stored digest")?;
    let stored = stored_audit(backend, dir, &genesis, original_hash, count)?;
    let hot_start = Instant::now();
    let before = trace.activity_callback_count();
    for _ in 0..100 {
        let _ = b.frames()?;
    }
    let report = json!({"schema":"worldstream/backend-existing-recovery/v1","backend":backend,"engine":b.engine()?,"original_completed_cell":prior,"full_recovery":{"elapsed_seconds":elapsed,"path":format!("{:?}",receipt.path()),"prefix_records_delivered":receipt.prefix_transition_records_delivered(),"tail_records_delivered":receipt.tail_transition_records_delivered(),"reducer_callbacks":callbacks,"retained_records_before_discard":retained_records,"exact_final_state_commitments_verified":true},"original_record_audit":stored,"hot_current_read":{"calls":100,"elapsed_seconds":hot_start.elapsed().as_secs_f64(),"registry_supplied":false,"owned_executor_callback_delta":trace.activity_callback_count()-before,"history_work_counter":"not_exposed"},"fault_setup":"none; delegates public full capture and guarded install on the completed original history","before_after":"Compare this separately labeled phase with the original cell fallback durations; no mutation throughput rerun is claimed"});
    fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    println!("completed existing full recovery {backend} transitions={count}");
    Ok(())
}
fn run() -> Result<()> {
    let backend = arg("--backend")?;
    let dir = arg("--cell-dir")?;
    let output = arg("--output")?;
    if env::args().any(|a| a == "--setup-postgres") {
        PostgresAdmin::new(PostgresConnectionConfig::direct_admin(env::var(
            "WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN",
        )?)?)?
        .migrate()?;
        return Ok(());
    }
    let count: u64 = arg("--transitions")?.parse()?;
    let size: usize = arg("--state-bytes")?.parse()?;
    let format_name = arg("--format")?;
    let format = if format_name == "v1" {
        CanonicalHistoryFormat::V1
    } else if format_name == "v2" {
        CanonicalHistoryFormat::V2
    } else {
        return Err("unknown format".into());
    };
    let benchmark = size > 0;
    let registry = if benchmark {
        benchmark_state_registry_for_conformance()?
    } else {
        builtin_counter_registry()?
    };
    let digest = if benchmark {
        registry
            .catalog_revisions()
            .next()
            .ok_or("benchmark revision")?
            .revision_digest
    } else {
        counter_v2_digest()
    };
    let b = if backend == "sqlite" {
        Backend::Sqlite(Arc::new(SqliteRoomStore::open(
            Path::new(&dir).join("room.db"),
        )?))
    } else {
        Backend::Postgres(Arc::new(PostgresRoomStore::new(
            PostgresConnectionConfig::runtime(
                env::var("WORLDSTREAM_POSTGRES_TEST_DSN")?,
                PostgresConnectionPath::Direct,
            )?,
        )?))
    };
    if env::args().any(|a| a == "--existing-recovery") {
        return existing_recovery(&b, &registry, &backend, &dir, &output, count, format);
    }
    let configuration = if benchmark {
        canonical(&json!({"state_bytes":size,"maximum_counter":count}))?
    } else {
        canonical(&json!({"initial_value":0,"maximum_value":16}))?
    };
    let mut trace = create(&b, &registry, digest, configuration, format)?;
    let authority = b.authority();
    authority.change(
        &PresentedCapabilityV1::new(parsed(HOST)?, CapabilityBearerV1::from_bytes([0xA7; 32])),
        AuthorityChangeV1::RegisterCapability {
            change_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FJ4")?,
            capability: NewCapabilityV1::new(
                parsed("01ARZ3NDEKTSV4RRFFQ69G5FH3")?,
                CapabilityBearerV1::from_bytes([0xB8; 32]).token_hash(),
                parsed(PRINCIPAL)?,
                CapabilityProfileV1::RoomMember {
                    room_id: parsed(ROOM)?,
                    member_id: parsed(MEMBER)?,
                },
                CapabilityScopeSetV1::new([CapabilityScopeV1::RoomAct])?,
                None,
            )?,
        },
        parsed(TIME)?,
    )?;
    let genesis = trace.genesis_bytes()?;
    let mut audit = blake3::Hasher::new();
    audit.update(&genesis);
    let mut effect_audit = blake3::Hasher::new();
    let mut canonical_bytes = 0u64;
    let mut samples = Vec::new();
    let stride = count.div_ceil(4096).max(1);
    let mut state_min = usize::MAX;
    let mut state_max = 0;
    let start = Instant::now();
    for n in 1..=count {
        if n == count && !count.is_multiple_of(250) {
            // A labeled operational fixture makes the final ordinary
            // postcommit checkpoint due. Its contents still come from the
            // real sealed transaction and production snapshot writer.
            probe(&backend, &dir, "checkpoint-due")?;
        }
        let (integrity, frames) = b.frames()?;
        let began = Instant::now();
        let (bytes, effects) = commit_one(&b, &mut trace, n, benchmark, &integrity, &frames)?;
        if n % stride == 0 || n == count {
            samples.push(began.elapsed());
        }
        audit.update(&bytes);
        effect_audit.update(&serde_json::to_vec(&effects)?);
        canonical_bytes += bytes.len() as u64;
        let len = trace.activity_state().to_bytes()?.len();
        state_min = state_min.min(len);
        state_max = state_max.max(len);
        trace.discard_persisted_history();
        if n % 1000 == 0 {
            eprintln!("committed {n}/{count}");
        }
    }
    let commit_seconds = start.elapsed().as_secs_f64();
    let expected_hash = audit.finalize().to_hex().to_string();
    let stored = stored_audit(&backend, &dir, &genesis, &expected_hash, count)?;
    let metrics = probe(&backend, &dir, "metrics")?;
    if metrics["transition_count"].as_u64() != Some(count)
        || metrics["transition_bytes"].as_u64() != Some(canonical_bytes)
    {
        return Err("durable transition inventory mismatch".into());
    }
    let hot_start = Instant::now();
    let callbacks_before = trace.activity_callback_count();
    for _ in 0..100 {
        let _ = b.frames()?;
    }
    let hot = hot_start.elapsed().as_secs_f64();
    let hot_callback_delta = trace.activity_callback_count() - callbacks_before;
    let ordinary = recovery(&b, &registry, &trace)?;
    if ordinary["path"] != "Checkpoint" || ordinary["tail_records_delivered"] != 0 {
        return Err("ordinary final checkpoint did not execute a zero-tail path".into());
    }
    let stale_started = Instant::now();
    let stale = recover_canonical_room_from_storage(
        &RecoveryProbePort {
            backend: &b,
            name: &backend,
            directory: &dir,
            force_full: false,
            change_install_fence: true,
        },
        &registry,
        &parsed(ROOM)?,
    );
    if !matches!(stale, Err(RoomRecoveryErrorV1::ConcurrentChange)) {
        return Err("changed install fence did not reject recovery".into());
    }
    let stale_seconds = stale_started.elapsed().as_secs_f64();
    probe(&backend, &dir, "tail")?;
    let tail = recovery(&b, &registry, &trace)?;
    if tail["path"] != "Checkpoint"
        || tail["tail_records_delivered"]
            .as_u64()
            .is_none_or(|n| n == 0 || n > 250)
    {
        return Err("nonzero checkpoint tail did not execute its bounded path".into());
    }
    probe(&backend, &dir, "invalid")?;
    let invalid = recovery(&b, &registry, &trace)?;
    probe(&backend, &dir, "absent")?;
    let absent = recovery(&b, &registry, &trace)?;
    for fallback in [&invalid, &absent] {
        if fallback["path"] != "Full" || fallback["prefix_records_delivered"] != count {
            return Err("checkpoint fallback did not replay exact full history".into());
        }
    }
    let report = json!({"schema":"worldstream/backend-stream-cell/v1","backend":backend,"format":format_name,"pack":if benchmark{"benchmark-state"}else{"retained-counter-v2"},"stimulus_mix":if benchmark{"all ParticipantAction increment"}else{"2 ParticipantAction increments then bounded Core Membership Standing suspend/resume"},"transitions":count,"requested_state_bytes":size,"engine":b.engine()?,"commit_wall_seconds":commit_seconds,"commit_latency":quantiles(&mut samples),"genesis_bytes":genesis.len(),"transition_bytes":canonical_bytes,"original_record_audit":stored,"effects_blake3":effect_audit.finalize().to_hex().to_string(),"final_core_hash":trace.head().core_state_hash().to_string(),"final_activity_hash":trace.head().activity_state_hash().to_string(),"activity_state_min_bytes":state_min,"activity_state_max_bytes":state_max,"core_bytes":trace.core_state().canonical_bytes()?.len(),"hot_current_read":{"calls":100,"elapsed_seconds":hot,"registry_supplied":false,"history_work_counter":"not_exposed","owned_executor_callback_delta":hot_callback_delta,"reason":"production current serving fence takes no registry or executable trace; SQL history work has no exposed counter"},"ordinary_recovery":ordinary,"stale_install_fence":{"elapsed_seconds":stale_seconds,"error":"ConcurrentChange","fixture":"public recovery port delegates capture and SQL install; changes only operational integrity generation immediately before install"},"nonzero_tail_recovery":tail,"invalid_checkpoint_fallback":invalid,"absent_checkpoint_full_replay":absent,"fault_injection":"driver SQL makes final checkpoint due for counts not divisible by250, then deletes/corrupts disposable snapshot witnesses; canonical history and operational proof rows remain unchanged","physical_inventory":metrics,"backup":"root-owned native qualification, not measured by this cell","transfer":"root-owned native qualification, not measured by this cell","device_write_traffic":"not_measured","materialization_write_count":"not_exposed","portable_component":"not_measured"});
    fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    println!("completed {backend} {format_name} transitions={count} state={size}");
    Ok(())
}
/// Entry shared by independently frozen measurement binaries.
pub fn main() {
    if let Err(e) = run() {
        eprintln!("measurement failed: {e}");
        std::process::exit(1);
    }
}
