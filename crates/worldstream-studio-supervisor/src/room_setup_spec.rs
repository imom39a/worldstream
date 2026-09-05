//! Reusable public Room creation intent, separate from retained setup operations.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use worldstream_core::CanonicalJsonV1;
use worldstream_protocol::{
    ActivityPackCatalogRevisionResponse, HostedSpectatorPurposeV2, PackReference, PrincipalKind,
};

use crate::{
    configuration_resolution::{ConfigurationResolutionError, resolve_configuration},
    room_drafts::{
        AgentAssignmentModeV1, AgentProfileRevisionReferenceV1, RunnerTemplateRevisionReferenceV1,
    },
};

/// A reference identifies a new Principal within this specification only.
/// Creation resolves each reference once per setup operation; it is not an ID lookup.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupPrincipalV1 {
    pub reference: String,
    pub kind: PrincipalKind,
}

/// Closed non-seat purpose supported by Room Setup Specification v2.
/// Each purpose derives its access mode, Role absence, and capability scopes
/// inside the Host; callers cannot widen them in setup input.
pub type SetupSpectatorPurposeV2 = HostedSpectatorPurposeV2;

/// One required, run-scoped non-seat Principal requested before Genesis.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupSpectatorV2 {
    pub purpose: SetupSpectatorPurposeV2,
    pub principal: SetupPrincipalV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupAssignmentV1 {
    pub mode: AgentAssignmentModeV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_profile: Option<AgentProfileRevisionReferenceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_template: Option<RunnerTemplateRevisionReferenceV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupSeatV1 {
    pub label: String,
    pub role: String,
    pub required: bool,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<SetupPrincipalV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<SetupAssignmentV1>,
}

/// Non-authoritative input. It carries no generated credentials or progress state.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomSetupSpecificationV1 {
    pub schema: String,
    pub pack: PackReference,
    pub configuration: Value,
    pub seats: Vec<SetupSeatV1>,
    /// v2-only non-seat spectators. v1 documents must omit this field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spectators: Vec<SetupSpectatorV2>,
    #[serde(default)]
    pub operator_view: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomSetupIssueCode {
    InvalidSpecification,
    ExactPackMismatch,
    SeatIntent,
    Required,
    DependencyMissing,
    DependencyIncompatible,
    OutputExists,
    OutputUnavailable,
    ExampleChoicesRequired,
}

/// Closed field diagnostics never include supplied values or parser messages.
#[derive(Debug, Deserialize, Error, Serialize)]
#[serde(untagged)]
pub enum RoomSetupError {
    #[error("Room setup does not satisfy the public specification")]
    Specification {
        path: String,
        code: RoomSetupIssueCode,
    },
    #[error("Room configuration does not satisfy the installed schema")]
    Configuration(#[from] ConfigurationResolutionError),
}

/// Resolves one reviewed input against the exact installed catalog revision.
/// This function performs no I/O, allocates no identities, and grants no approval.
///
/// # Errors
/// Rejects malformed input, mismatched catalog identity, and invalid configuration.
pub fn resolve_setup_specification(
    bytes: &[u8],
    catalog: &ActivityPackCatalogRevisionResponse,
) -> Result<RoomSetupSpecificationV1, RoomSetupError> {
    let mut specification = parse_setup_specification(bytes)?;
    if catalog.version != "activity_pack_catalog.v1"
        || specification.pack != catalog.revision.summary.pack
        || !catalog.revision.summary.selectable_for_new_rooms
    {
        return Err(RoomSetupError::Specification {
            path: "/pack".to_owned(),
            code: RoomSetupIssueCode::ExactPackMismatch,
        });
    }
    specification.configuration = resolve_configuration(
        &catalog.revision.configuration_schema.schema,
        &specification.configuration,
    )?;
    validate_seats(&specification, catalog)?;
    Ok(specification)
}

/// Checks bounded public input before any local controller connection is opened.
///
/// # Errors
/// Rejects malformed JSON, secret-bearing data, and unsupported specification fields.
pub fn parse_setup_specification(bytes: &[u8]) -> Result<RoomSetupSpecificationV1, RoomSetupError> {
    let invalid = || RoomSetupError::Specification {
        path: String::new(),
        code: RoomSetupIssueCode::InvalidSpecification,
    };
    if bytes.len() > 1_048_576 {
        return Err(invalid());
    }
    CanonicalJsonV1::parse(bytes).map_err(|_| invalid())?;
    let input: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if !safe_payload(&input, 0, &mut 4096) {
        return Err(invalid());
    }
    let spectators_were_supplied = input
        .as_object()
        .is_some_and(|object| object.contains_key("spectators"));
    let specification: RoomSetupSpecificationV1 =
        serde_json::from_value(input).map_err(|_| invalid())?;
    if !matches!(
        specification.schema.as_str(),
        "worldstream/room-setup/v1" | "worldstream/room-setup/v2"
    ) {
        return Err(invalid());
    }
    validate_spectators(&specification, spectators_were_supplied)?;
    Ok(specification)
}

/// Resolves reviewed example data through the same generic exact-Pack validator.
/// Examples are input documents, not Pack-specific field logic or approvals.
///
/// # Errors
/// Reports missing reviewed choices or rejects an invalid exact example.
pub fn generate_setup_example(
    catalog: &ActivityPackCatalogRevisionResponse,
) -> Result<RoomSetupSpecificationV1, RoomSetupError> {
    const REVIEWED: &[&[u8]] = &[
        include_bytes!("../../../examples/room-setup/agent-heist-0.2.0.json"),
        include_bytes!("../../../examples/room-setup/negotiate-0.2.0.json"),
    ];
    for bytes in REVIEWED {
        let example: RoomSetupSpecificationV1 =
            serde_json::from_slice(bytes).map_err(|_| RoomSetupError::Specification {
                path: String::new(),
                code: RoomSetupIssueCode::InvalidSpecification,
            })?;
        if example.pack == catalog.revision.summary.pack {
            return resolve_setup_specification(bytes, catalog);
        }
    }
    Err(RoomSetupError::Specification {
        path: String::new(),
        code: RoomSetupIssueCode::ExampleChoicesRequired,
    })
}

fn validate_seats(
    specification: &RoomSetupSpecificationV1,
    catalog: &ActivityPackCatalogRevisionResponse,
) -> Result<(), RoomSetupError> {
    let mut labels = BTreeSet::new();
    let mut principals = BTreeSet::new();
    let mut role_counts = BTreeMap::new();
    let mut required_counts = BTreeMap::new();
    if specification.seats.len() > 64 {
        return Err(RoomSetupError::Specification {
            path: "/seats".to_owned(),
            code: RoomSetupIssueCode::SeatIntent,
        });
    }
    for (index, seat) in specification.seats.iter().enumerate() {
        let invalid = || RoomSetupError::Specification {
            path: format!("/seats/{index}"),
            code: RoomSetupIssueCode::SeatIntent,
        };
        if !seat_label(&seat.label)
            || !public_reference(&seat.role)
            || seat.display_name.is_empty()
            || seat.display_name.len() > 128
            || !labels.insert(seat.label.as_str())
            || !catalog
                .revision
                .roles
                .iter()
                .any(|role| role.role == seat.role)
            || (seat.required && seat.principal.is_none())
        {
            return Err(invalid());
        }
        *role_counts.entry(seat.role.as_str()).or_insert(0_u32) += 1;
        if seat.required {
            *required_counts.entry(seat.role.as_str()).or_insert(0_u32) += 1;
        }
        match &seat.principal {
            Some(principal) => {
                if !public_reference(&principal.reference)
                    || !principals.insert(principal.reference.as_str())
                {
                    return Err(invalid());
                }
                match principal.kind {
                    PrincipalKind::Human if seat.assignment.is_some() => return Err(invalid()),
                    PrincipalKind::Agent => {
                        let assignment = seat.assignment.as_ref().ok_or_else(invalid)?;
                        if matches!(assignment.mode, AgentAssignmentModeV1::Managed)
                            && assignment.agent_profile.is_none()
                        {
                            return Err(RoomSetupError::Specification {
                                path: format!("/seats/{index}/assignment/agent_profile"),
                                code: RoomSetupIssueCode::Required,
                            });
                        }
                        if matches!(assignment.mode, AgentAssignmentModeV1::Managed)
                            != assignment.runner_template.is_some()
                        {
                            return Err(invalid());
                        }
                    }
                    PrincipalKind::Human => {}
                }
            }
            None if seat.assignment.is_some() => return Err(invalid()),
            None => {}
        }
    }
    for role in &catalog.revision.roles {
        let count = role_counts.get(role.role.as_str()).copied().unwrap_or(0);
        let required = required_counts
            .get(role.role.as_str())
            .copied()
            .unwrap_or(0);
        if count < role.minimum || count > role.maximum || required < role.minimum {
            return Err(RoomSetupError::Specification {
                path: "/seats".to_owned(),
                code: RoomSetupIssueCode::SeatIntent,
            });
        }
    }
    Ok(())
}

fn validate_spectators(
    specification: &RoomSetupSpecificationV1,
    spectators_were_supplied: bool,
) -> Result<(), RoomSetupError> {
    let invalid = |path: String| RoomSetupError::Specification {
        path,
        code: RoomSetupIssueCode::InvalidSpecification,
    };
    if specification.schema == "worldstream/room-setup/v1" {
        return if spectators_were_supplied {
            Err(invalid("/spectators".to_owned()))
        } else {
            Ok(())
        };
    }
    if specification.operator_view {
        return Err(invalid("/operator_view".to_owned()));
    }
    if specification.spectators.is_empty() || specification.spectators.len() > 3 {
        return Err(invalid("/spectators".to_owned()));
    }
    let mut purposes = BTreeSet::new();
    let mut references = specification
        .seats
        .iter()
        .filter_map(|seat| seat.principal.as_ref())
        .map(|principal| principal.reference.as_str())
        .collect::<BTreeSet<_>>();
    for (index, spectator) in specification.spectators.iter().enumerate() {
        let path = format!("/spectators/{index}");
        let expected_kind = spectator.purpose.principal_kind();
        if spectator.principal.kind != expected_kind
            || !public_reference(&spectator.principal.reference)
            || !purposes.insert(spectator.purpose)
            || !references.insert(&spectator.principal.reference)
        {
            return Err(invalid(path));
        }
    }
    if !purposes.contains(&SetupSpectatorPurposeV2::ResultIndexer) {
        return Err(RoomSetupError::Specification {
            path: "/spectators".to_owned(),
            code: RoomSetupIssueCode::Required,
        });
    }
    Ok(())
}

fn seat_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn public_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn safe_payload(value: &Value, depth: usize, remaining: &mut usize) -> bool {
    if depth > 32 || *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    match value {
        Value::Object(fields) => fields.iter().all(|(key, value)| {
            let normalized = key.replace('-', "_").to_ascii_lowercase();
            !["secret", "bearer", "password", "api_key", "apikey", "token"]
                .iter()
                .any(|forbidden| normalized.contains(forbidden))
                && safe_payload(value, depth + 1, remaining)
        }),
        Value::Array(values) => values
            .iter()
            .all(|value| safe_payload(value, depth + 1, remaining)),
        Value::String(value) => {
            let lower = value.trim().to_ascii_lowercase();
            !value.chars().any(char::is_control)
                && !value.contains("${")
                && !value.contains("$(")
                && ![
                    "bearer ",
                    "sk-",
                    "sk_",
                    "ghp_",
                    "github_pat_",
                    "xoxb-",
                    "xoxp-",
                ]
                .iter()
                .any(|prefix| lower.starts_with(prefix))
        }
        _ => true,
    }
}
