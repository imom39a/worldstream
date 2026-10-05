//! Immutable byte ceilings for explicitly selected compact Room lineages.

use serde::Serialize;
use thiserror::Error;

use crate::{
    CanonicalHistoryFormat, CanonicalJsonError, CanonicalJsonV1, CoreRoomStateV1,
    PackGenesisErrorV1, PackGenesisRequestV1, PackRegistryV1, PreparedNewRoomGenesisV1,
    RecordedStimulusV1, TimerChangeV1,
};

/// Exact policy identity bound by V2 Genesis and creation requests.
pub const PAYLOAD_BUDGET_V1_ID: &str = "worldstream/payload-budget/v1";

/// Inclusive canonical UTF-8 byte ceilings. These values define admission
/// accounting, not state hash framing or physical compression limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PayloadBudgetV1 {
    /// Complete control metadata byte ceiling.
    pub control_metadata: usize,
    /// Complete action payload byte ceiling.
    pub action_payload: usize,
    /// Complete external input payload byte ceiling.
    pub external_input_payload: usize,
    /// Complete creation configuration byte ceiling.
    pub creation_configuration: usize,
    /// Complete domain event item byte ceiling.
    pub domain_event_item: usize,
    /// Complete domain events array byte ceiling.
    pub domain_events_array: usize,
    /// Complete timer change item byte ceiling.
    pub timer_change_item: usize,
    /// Complete timer changes array byte ceiling.
    pub timer_changes_array: usize,
    /// Complete attention signal item byte ceiling.
    pub attention_signal_item: usize,
    /// Complete attention signals array byte ceiling.
    pub attention_signals_array: usize,
    /// Complete effects byte ceiling.
    pub effects: usize,
    /// Complete transition byte ceiling.
    pub transition: usize,
    /// Complete activity state byte ceiling.
    pub activity_state: usize,
    /// Complete core state byte ceiling.
    pub core_state: usize,
    /// Complete authoritative state byte ceiling.
    pub authoritative_state: usize,
    /// Complete genesis byte ceiling.
    pub genesis: usize,
    /// Complete observation byte ceiling.
    pub observation: usize,
    /// Complete projection byte ceiling.
    pub projection: usize,
    /// Complete artifact reference byte ceiling.
    pub artifact_reference: usize,
}

/// Frozen initial compact-Room policy. Legacy Rooms do not select this policy.
pub const PAYLOAD_BUDGET_V1: PayloadBudgetV1 = PayloadBudgetV1 {
    control_metadata: 4_096,
    action_payload: 32_768,
    external_input_payload: 32_768,
    creation_configuration: 32_768,
    domain_event_item: 8_192,
    domain_events_array: 262_144,
    timer_change_item: 4_096,
    timer_changes_array: 65_536,
    attention_signal_item: 4_096,
    attention_signals_array: 65_536,
    effects: 393_216,
    transition: 524_288,
    activity_state: 262_144,
    core_state: 131_072,
    authoritative_state: 524_288,
    genesis: 786_432,
    observation: 32_768,
    projection: 262_144,
    artifact_reference: 2_048,
};

/// One precisely classified canonical value in the fixed Room policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PayloadKindV1 {
    /// Classified control fields.
    ControlMetadata,
    /// One Action payload.
    ActionPayload,
    /// One external-input payload.
    ExternalInputPayload,
    /// Complete creation configuration.
    CreationConfiguration,
    /// One Domain Event.
    DomainEventItem,
    /// Complete ordered Domain Event array.
    DomainEventsArray,
    /// One Timer change.
    TimerChangeItem,
    /// Complete ordered Timer-change array.
    TimerChangesArray,
    /// One Attention Signal.
    AttentionSignalItem,
    /// Complete ordered Attention Signal array.
    AttentionSignalsArray,
    /// Complete effects accounting object.
    Effects,
    /// Complete compact Transition.
    Transition,
    /// Complete Activity State.
    ActivityState,
    /// Complete Core Room State.
    CoreState,
    /// Complete Authoritative State accounting object.
    AuthoritativeState,
    /// Complete Genesis.
    Genesis,
    /// Complete authorized Observation.
    Observation,
    /// Complete authorized Projection.
    Projection,
    /// One declared application Artifact reference.
    ArtifactReference,
}

/// A byte count only. It contains no caller content or authority credential.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{kind:?} has {actual_bytes} canonical bytes; limit is {maximum_bytes}")]
pub struct PayloadBudgetViolationV1 {
    /// The classified value that exceeds its ceiling.
    pub kind: PayloadKindV1,
    /// Exact canonical UTF-8 byte count.
    pub actual_bytes: usize,
    /// Inclusive selected ceiling.
    pub maximum_bytes: usize,
}

/// Canonical encoding failure or an inclusive policy ceiling violation.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PayloadBudgetErrorV1 {
    /// The value cannot be encoded by the canonical codec.
    #[error(transparent)]
    Encoding(#[from] CanonicalJsonError),
    /// The complete encoded value exceeds its ceiling.
    #[error(transparent)]
    Exceeded(#[from] PayloadBudgetViolationV1),
}

/// Fresh creation input validation is distinct from a selected Pack failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CanonicalGenesisPreparationError {
    /// Caller configuration, initial Core, or control metadata exceeds policy.
    #[error(transparent)]
    Input(#[from] PayloadBudgetErrorV1),
    /// Exact registry selection or checked initialization failed.
    #[error(transparent)]
    Genesis(#[from] PackGenesisErrorV1),
}

impl PackRegistryV1 {
    /// Checks fresh selected creation input before Pack initialization.
    /// Callers must resolve authoritative creation receipts before this method.
    ///
    /// # Errors
    ///
    /// Returns an input ceiling, registry-selection, or initialization error.
    pub fn prepare_genesis_for_new_room_with_format(
        &self,
        request: &PackGenesisRequestV1,
        format: CanonicalHistoryFormat,
    ) -> Result<PreparedNewRoomGenesisV1, CanonicalGenesisPreparationError> {
        if format == CanonicalHistoryFormat::V2 {
            PAYLOAD_BUDGET_V1.check_genesis_request(request)?;
        }
        Ok(self.prepare_genesis_for_new_room(request)?)
    }
}

impl PayloadBudgetV1 {
    /// Returns the ceiling for an explicitly classified value.
    #[must_use]
    pub const fn maximum(self, kind: PayloadKindV1) -> usize {
        match kind {
            PayloadKindV1::ControlMetadata => self.control_metadata,
            PayloadKindV1::ActionPayload => self.action_payload,
            PayloadKindV1::ExternalInputPayload => self.external_input_payload,
            PayloadKindV1::CreationConfiguration => self.creation_configuration,
            PayloadKindV1::DomainEventItem => self.domain_event_item,
            PayloadKindV1::DomainEventsArray => self.domain_events_array,
            PayloadKindV1::TimerChangeItem => self.timer_change_item,
            PayloadKindV1::TimerChangesArray => self.timer_changes_array,
            PayloadKindV1::AttentionSignalItem => self.attention_signal_item,
            PayloadKindV1::AttentionSignalsArray => self.attention_signals_array,
            PayloadKindV1::Effects => self.effects,
            PayloadKindV1::Transition => self.transition,
            PayloadKindV1::ActivityState => self.activity_state,
            PayloadKindV1::CoreState => self.core_state,
            PayloadKindV1::AuthoritativeState => self.authoritative_state,
            PayloadKindV1::Genesis => self.genesis,
            PayloadKindV1::Observation => self.observation,
            PayloadKindV1::Projection => self.projection,
            PayloadKindV1::ArtifactReference => self.artifact_reference,
        }
    }

    /// Checks an already encoded canonical value. Equality is permitted.
    ///
    /// # Errors
    ///
    /// Returns the classified count when the ceiling is exceeded.
    pub fn check_bytes(
        self,
        kind: PayloadKindV1,
        bytes: &[u8],
    ) -> Result<(), PayloadBudgetViolationV1> {
        self.check_length(kind, bytes.len())
    }

    /// Uses the kernel canonical codec, including UTF-8 and container framing.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or a classified ceiling violation.
    pub fn check_value<T: Serialize>(
        self,
        kind: PayloadKindV1,
        value: &T,
    ) -> Result<(), PayloadBudgetErrorV1> {
        let bytes = CanonicalJsonV1::from_serialize(value)?.to_bytes()?;
        self.check_bytes(kind, &bytes)?;
        Ok(())
    }

    /// Accounts for the precisely classified creation fields before initialize.
    /// Configuration and complete initial Core have independent ceilings.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or a fresh-input ceiling violation.
    pub fn check_genesis_request(
        self,
        request: &PackGenesisRequestV1,
    ) -> Result<(), PayloadBudgetErrorV1> {
        self.check_value(PayloadKindV1::CreationConfiguration, &request.configuration)?;
        self.check_value(PayloadKindV1::CoreState, &request.initial_core_state)?;
        let mut metadata = serde_json::to_value(request)
            .map_err(|error| CanonicalJsonError::Serialization(error.to_string()))?;
        if let Some(fields) = metadata.as_object_mut() {
            fields.remove("configuration");
            fields.remove("initial_core_state");
        }
        self.check_value(PayloadKindV1::ControlMetadata, &metadata)
    }

    /// Checks caller payloads and their classified control metadata before any
    /// Pack callback. Proposed Core is checked separately after host reduction.
    /// This function has no authority or durable receipt lookup. Adapters must
    /// resolve accepted matching receipts before fresh admission calls it.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or an input or metadata ceiling violation.
    pub fn check_stimulus(self, stimulus: &RecordedStimulusV1) -> Result<(), PayloadBudgetErrorV1> {
        match stimulus {
            RecordedStimulusV1::ParticipantAction(action) => {
                self.check_value(PayloadKindV1::ActionPayload, &action.canonical_payload)?;
            }
            RecordedStimulusV1::ExternalInput(input) => {
                self.check_value(
                    PayloadKindV1::ExternalInputPayload,
                    &input.canonical_payload,
                )?;
            }
            RecordedStimulusV1::TimerFired(_) | RecordedStimulusV1::CoreProposed(_) => {}
        }
        // These fields belong to exact typed Stimuli. This does not classify
        // arbitrary application JSON or infer Artifact references from shape.
        let mut metadata = serde_json::to_value(stimulus)
            .map_err(|error| CanonicalJsonError::Serialization(error.to_string()))?;
        if let Some(object) = metadata.as_object_mut() {
            object.remove("canonical_payload");
            object.remove("canonical_changeset");
        }
        self.check_value(PayloadKindV1::ControlMetadata, &metadata)
    }

    /// Counts complete canonical Core and Activity and the fixed accounting
    /// object. This does not change any existing state-hash preimage.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or a state ceiling violation.
    pub fn check_authoritative_state(
        self,
        core: &CoreRoomStateV1,
        activity: &CanonicalJsonV1,
    ) -> Result<(), PayloadBudgetErrorV1> {
        let core_bytes = core.canonical_bytes()?;
        let activity_bytes = activity.to_bytes()?;
        self.check_bytes(PayloadKindV1::CoreState, &core_bytes)?;
        self.check_bytes(PayloadKindV1::ActivityState, &activity_bytes)?;
        let count = b"{\"activity_state\":,\"core_state\":}"
            .len()
            .saturating_add(core_bytes.len())
            .saturating_add(activity_bytes.len());
        self.check_length(PayloadKindV1::AuthoritativeState, count)?;
        Ok(())
    }

    /// Checks each item, each complete ordered array, and the combined effects
    /// object. Array brackets, commas, and object keys count toward the limits.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or an item or aggregate ceiling violation.
    pub fn check_effects(
        self,
        events: &[CanonicalJsonV1],
        timers: &[TimerChangeV1],
        attention: &[CanonicalJsonV1],
    ) -> Result<(), PayloadBudgetErrorV1> {
        let events_bytes = self.check_array(
            PayloadKindV1::DomainEventItem,
            PayloadKindV1::DomainEventsArray,
            events,
        )?;
        let timer_bytes = self.check_array(
            PayloadKindV1::TimerChangeItem,
            PayloadKindV1::TimerChangesArray,
            timers,
        )?;
        let attention_bytes = self.check_array(
            PayloadKindV1::AttentionSignalItem,
            PayloadKindV1::AttentionSignalsArray,
            attention,
        )?;
        let count = b"{\"ordered_attention_signals\":,\"ordered_domain_events\":,\"ordered_timer_changes\":}".len()
            .saturating_add(events_bytes)
            .saturating_add(timer_bytes)
            .saturating_add(attention_bytes);
        self.check_length(PayloadKindV1::Effects, count)?;
        Ok(())
    }

    /// Checks normalized initial Timer items and their complete ordered array.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or an item or array ceiling violation.
    pub fn check_initial_timers(
        self,
        timers: &[crate::ScheduledTimerV1],
    ) -> Result<(), PayloadBudgetErrorV1> {
        self.check_array(
            PayloadKindV1::TimerChangeItem,
            PayloadKindV1::TimerChangesArray,
            timers,
        )?;
        Ok(())
    }

    /// Applies the reference ceiling only when the caller already has an exact
    /// application-declared reference contract. No JSON shape scan occurs.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or a reference ceiling violation.
    pub fn check_declared_artifact_reference(
        self,
        reference: &CanonicalJsonV1,
    ) -> Result<(), PayloadBudgetErrorV1> {
        self.check_value(PayloadKindV1::ArtifactReference, reference)
    }

    fn check_length(
        self,
        kind: PayloadKindV1,
        actual_bytes: usize,
    ) -> Result<(), PayloadBudgetViolationV1> {
        let maximum_bytes = self.maximum(kind);
        if actual_bytes > maximum_bytes {
            Err(PayloadBudgetViolationV1 {
                kind,
                actual_bytes,
                maximum_bytes,
            })
        } else {
            Ok(())
        }
    }

    fn check_array<T: Serialize>(
        self,
        item_kind: PayloadKindV1,
        array_kind: PayloadKindV1,
        values: &[T],
    ) -> Result<usize, PayloadBudgetErrorV1> {
        let mut bytes = 2_usize;
        for (index, value) in values.iter().enumerate() {
            let item = CanonicalJsonV1::from_serialize(value)?.to_bytes()?;
            self.check_bytes(item_kind, &item)?;
            bytes = bytes
                .saturating_add(item.len())
                .saturating_add(usize::from(index > 0));
            self.check_length(array_kind, bytes)?;
        }
        self.check_length(array_kind, bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn canonical(value: &serde_json::Value) -> Result<CanonicalJsonV1, Box<dyn std::error::Error>> {
        Ok(CanonicalJsonV1::parse(&serde_json::to_vec(value)?)?)
    }

    #[test]
    fn policy_counts_canonical_utf8_at_inclusive_input_and_item_boundaries() -> TestResult {
        for (kind, bytes) in [
            (PayloadKindV1::ActionPayload, 32_768),
            (PayloadKindV1::ExternalInputPayload, 32_768),
            (PayloadKindV1::CreationConfiguration, 32_768),
            (PayloadKindV1::DomainEventItem, 8_192),
            (PayloadKindV1::AttentionSignalItem, 4_096),
        ] {
            let text = "é".repeat((bytes - 2) / 2);
            let exact = canonical(&serde_json::json!(text))?;
            assert_eq!(exact.to_bytes()?.len(), bytes);
            assert!(PAYLOAD_BUDGET_V1.check_value(kind, &exact).is_ok());
            let oversized = canonical(&serde_json::json!(format!("{text}x")))?;
            assert_eq!(
                PAYLOAD_BUDGET_V1.check_value(kind, &oversized),
                Err(PayloadBudgetErrorV1::Exceeded(PayloadBudgetViolationV1 {
                    kind,
                    actual_bytes: bytes + 1,
                    maximum_bytes: bytes,
                }))
            );
        }
        Ok(())
    }

    #[test]
    fn policy_counts_array_and_combined_effects_framing() -> TestResult {
        let events = vec![canonical(&serde_json::json!({"type":"é"}))?; 3];
        let attention = vec![canonical(&serde_json::json!({"reason":"注意"}))?; 2];
        let timers: Vec<TimerChangeV1> = Vec::new();
        let accounting = canonical(&serde_json::json!({
            "ordered_attention_signals": attention,
            "ordered_domain_events": events,
            "ordered_timer_changes": timers,
        }))?;
        let exact = accounting.to_bytes()?.len();
        let budget = PayloadBudgetV1 {
            effects: exact,
            ..PAYLOAD_BUDGET_V1
        };
        assert!(budget.check_effects(&events, &timers, &attention).is_ok());
        let exceeded = PayloadBudgetV1 {
            effects: exact - 1,
            ..budget
        }
        .check_effects(&events, &timers, &attention)
        .err()
        .ok_or("expected a ceiling violation")?;
        assert_eq!(
            exceeded,
            PayloadBudgetErrorV1::Exceeded(PayloadBudgetViolationV1 {
                kind: PayloadKindV1::Effects,
                actual_bytes: exact,
                maximum_bytes: exact - 1,
            })
        );
        let events_bytes = canonical(&serde_json::json!(events))?.to_bytes()?.len();
        let exceeded = PayloadBudgetV1 {
            domain_events_array: events_bytes - 1,
            ..budget
        }
        .check_effects(&events, &timers, &attention)
        .err()
        .ok_or("expected a ceiling violation")?;
        assert_eq!(
            exceeded,
            PayloadBudgetErrorV1::Exceeded(PayloadBudgetViolationV1 {
                kind: PayloadKindV1::DomainEventsArray,
                actual_bytes: events_bytes,
                maximum_bytes: events_bytes - 1,
            })
        );
        Ok(())
    }

    #[test]
    fn policy_does_not_classify_arbitrary_application_json_as_artifact_references() -> TestResult {
        let lookalike = canonical(&serde_json::json!({
            "artifact_id":"ordinary application text", "body":"a".repeat(3000)
        }))?;
        assert!(
            PAYLOAD_BUDGET_V1
                .check_value(PayloadKindV1::ActionPayload, &lookalike)
                .is_ok()
        );
        assert!(matches!(
            PAYLOAD_BUDGET_V1.check_declared_artifact_reference(&lookalike),
            Err(PayloadBudgetErrorV1::Exceeded(PayloadBudgetViolationV1 {
                kind: PayloadKindV1::ArtifactReference,
                ..
            }))
        ));
        Ok(())
    }
}
