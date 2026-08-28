//! Embedded retained registry rows for the Counter conformance revisions.
//!
//! This module intentionally lives outside `counter.rs`: the executor source
//! is itself a revision-lock input, while the reviewed transcript digests are
//! derived evidence. Keeping the derived literals here avoids a self-hashing
//! source cycle.

use std::str::FromStr;

use crate::{
    AccessModeV1, CanonicalJsonV1, CoreRoomStateV1, MembershipStandingV1, MembershipV1,
    PackDigestV1, PackGenesisRequestV1, PackGoldenActionV1, PackGoldenCorpusV1,
    PackGoldenViewerKindV1, PackGoldenViewerV1, PackRegistryErrorV1, PackRegistryStatusV1,
    PackRegistryV1, PrincipalKindV1,
    activity_pack::{CanonicalPackCodecV1, PackRegistryArtifactsV1, PackRegistryEntryV1},
    counter::{counter_v1_revision, counter_v2_revision},
    counter_attention::counter_v3_revision,
};

const PARTICIPANT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const SPECTATOR: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
const OPERATOR: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC2";

// Authored once from the checked multi-view transcript and then held fixed.
// A changed executor, codec, visibility rule, Action Offer, event, or lineage
// byte must disagree during registry construction.
const COUNTER_V1_TRANSCRIPT_DIGEST: &str =
    "blake3:af195b647116508de2210a5e8aec7d8558295ef8b270eedaeb70a0d624c8e9dd";
const COUNTER_V2_TRANSCRIPT_DIGEST: &str =
    "blake3:2f09686e4b8f78db7220c5f1b55061788753d11619832d28e137412207e7196a";
const COUNTER_V3_TRANSCRIPT_DIGEST: &str =
    "blake3:fec06f45b83bc5e8bc9b19f0dc64c94721e115389e2906a340ac2549b3bf6105";
#[cfg(test)]
const COUNTER_V3_CORPUS_DIGEST: &str =
    "blake3:e41e162eaadf33eee2f242d3736f2b678d3e24212fcde3528a8070ad8af01a18";

/// Constructs the complete embedded Counter conformance registry.
///
/// Counter v1 is retained-only. Counter v2 is selectable inside the
/// conformance runtime and retained-runnable. Release manifests keep both
/// Counter rows nonselectable because Counter is not the release Activity.
///
/// # Errors
///
/// Fails closed if any revision-lock, schema, codec, executor provenance, or
/// literal golden-transcript byte disagrees.
pub fn builtin_counter_registry() -> Result<PackRegistryV1, PackRegistryErrorV1> {
    let v1 = counter_entry(false, true, CounterRevision::V1)?;
    let v2 = counter_entry(true, true, CounterRevision::V2)?;
    let v3 = counter_entry(true, true, CounterRevision::V3)?;
    PackRegistryV1::try_new([v1, v2, v3])
}

/// Builds the retained-v1-only registry used to prove missing-runtime recovery.
///
/// # Errors
///
/// Fails closed under the same registry verification as the complete fixture.
#[cfg(any(test, feature = "conformance-tracer"))]
pub fn counter_v1_only_registry_for_conformance() -> Result<PackRegistryV1, PackRegistryErrorV1> {
    PackRegistryV1::try_new([counter_entry(false, true, CounterRevision::V1)?])
}

/// Builds the complete fixture with the v2 executor replaced by one that
/// panics at the selected pure host invocation.
///
/// # Errors
///
/// Fails if the checked fixture registry cannot be built or the replacement
/// does not retain the exact v2 descriptor identity.
#[cfg(any(test, feature = "conformance-tracer"))]
pub fn counter_v2_runtime_fault_registry_for_conformance(
    operation: crate::ActivityPackOperationV1,
) -> Result<PackRegistryV1, PackRegistryErrorV1> {
    let mut registry = builtin_counter_registry()?;
    registry.replace_executor_for_conformance(
        &counter_v2_digest(),
        std::sync::Arc::new(FaultingCounterV2 {
            operation,
            return_error: false,
        }),
    )?;
    Ok(registry)
}

/// Builds the complete fixture with the v2 executor returning a typed fault
/// at the selected host invocation.
///
/// # Errors
///
/// Fails if the checked fixture registry cannot be built or the replacement
/// does not retain the exact v2 descriptor identity.
#[cfg(any(test, feature = "conformance-tracer"))]
pub fn counter_v2_returned_fault_registry_for_conformance(
    operation: crate::ActivityPackOperationV1,
) -> Result<PackRegistryV1, PackRegistryErrorV1> {
    let mut registry = builtin_counter_registry()?;
    registry.replace_executor_for_conformance(
        &counter_v2_digest(),
        std::sync::Arc::new(FaultingCounterV2 {
            operation,
            return_error: true,
        }),
    )?;
    Ok(registry)
}

/// Builds the complete fixture with a successful v2-shaped executor whose
/// reduction deliberately returns v1 semantics.
///
/// # Errors
///
/// Fails if the checked fixture registry cannot be built or the replacement
/// does not retain the exact v2 descriptor identity.
#[cfg(any(test, feature = "conformance-tracer"))]
pub fn counter_v2_semantic_mismatch_registry_for_conformance()
-> Result<PackRegistryV1, PackRegistryErrorV1> {
    let mut registry = builtin_counter_registry()?;
    registry.replace_executor_for_conformance(
        &counter_v2_digest(),
        std::sync::Arc::new(SemanticMismatchCounterV2),
    )?;
    Ok(registry)
}

/// Builds the complete fixture with a v2 executor that returns a malformed
/// successful value from the selected operation.
///
/// # Errors
///
/// Fails if the checked fixture registry cannot be built or the replacement
/// does not retain the exact v2 descriptor identity.
#[cfg(any(test, feature = "conformance-tracer"))]
pub fn counter_v2_malformed_output_registry_for_conformance(
    operation: crate::ActivityPackOperationV1,
) -> Result<PackRegistryV1, PackRegistryErrorV1> {
    let mut registry = builtin_counter_registry()?;
    registry.replace_executor_for_conformance(
        &counter_v2_digest(),
        std::sync::Arc::new(MalformedCounterV2 { operation }),
    )?;
    Ok(registry)
}

/// Builds the complete fixture with a v2 reducer that emits a host-invalid
/// Timer request after otherwise successful reduction.
///
/// # Errors
///
/// Fails if the checked fixture registry cannot be built or the replacement
/// does not retain the exact v2 descriptor identity.
#[cfg(any(test, feature = "conformance-tracer"))]
pub fn counter_v2_invalid_timer_output_registry_for_conformance()
-> Result<PackRegistryV1, PackRegistryErrorV1> {
    let mut registry = builtin_counter_registry()?;
    registry.replace_executor_for_conformance(
        &counter_v2_digest(),
        std::sync::Arc::new(InvalidTimerOutputCounterV2),
    )?;
    Ok(registry)
}

/// Exact retained v1 semantic digest.
#[must_use]
pub fn counter_v1_digest() -> PackDigestV1 {
    counter_v1_revision().0.revision_digest.clone()
}

/// Exact current conformance v2 semantic digest.
#[must_use]
pub fn counter_v2_digest() -> PackDigestV1 {
    counter_v2_revision().0.revision_digest.clone()
}

/// Exact attention-bearing Counter v3 semantic digest.
#[must_use]
pub fn counter_v3_digest() -> PackDigestV1 {
    counter_v3_revision().0.revision_digest.clone()
}

#[derive(Clone, Copy)]
enum CounterRevision {
    V1,
    V2,
    V3,
}

#[cfg(any(test, feature = "conformance-tracer"))]
struct FaultingCounterV2 {
    operation: crate::ActivityPackOperationV1,
    return_error: bool,
}

#[cfg(any(test, feature = "conformance-tracer"))]
#[allow(clippy::panic)]
impl crate::ActivityPackV1 for FaultingCounterV2 {
    fn descriptor(&self) -> &'static crate::PackRevisionDescriptorV1 {
        crate::ActivityPackV1::descriptor(&crate::counter::CounterV2)
    }

    fn initialize(
        &self,
        input: &crate::ActivityGenesisInputV1<'_>,
        cx: &crate::DeterministicContextV1<'_>,
    ) -> Result<crate::InitialOutputV1, crate::PackFaultV1> {
        if self.operation == crate::ActivityPackOperationV1::Initialize {
            if self.return_error {
                return Err(crate::PackFaultV1::Callback(
                    "conformance initialize fault".to_owned(),
                ));
            }
            panic!("conformance initialize panic");
        }
        crate::ActivityPackV1::initialize(&crate::counter::CounterV2, input, cx)
    }

    fn reduce(
        &self,
        input: &crate::ActivityReduceInputV1<'_>,
        cx: &crate::DeterministicContextV1<'_>,
    ) -> Result<crate::ActivityDispositionV1, crate::PackFaultV1> {
        if self.operation == crate::ActivityPackOperationV1::Reduce {
            if self.return_error {
                return Err(crate::PackFaultV1::Callback(
                    "conformance reduce fault".to_owned(),
                ));
            }
            panic!("conformance reduce panic");
        }
        crate::ActivityPackV1::reduce(&crate::counter::CounterV2, input, cx)
    }

    fn view(
        &self,
        input: &crate::ViewInputV1<'_>,
    ) -> Result<crate::PackViewV1, crate::PackFaultV1> {
        if self.operation == crate::ActivityPackOperationV1::View {
            if self.return_error {
                return Err(crate::PackFaultV1::Callback(
                    "conformance view fault".to_owned(),
                ));
            }
            panic!("conformance view panic");
        }
        crate::ActivityPackV1::view(&crate::counter::CounterV2, input)
    }

    fn observe(
        &self,
        input: &crate::ObserveInputV1<'_>,
    ) -> Result<Option<crate::PackObservationV1>, crate::PackFaultV1> {
        if self.operation == crate::ActivityPackOperationV1::Observe {
            if self.return_error {
                return Err(crate::PackFaultV1::Callback(
                    "conformance observe fault".to_owned(),
                ));
            }
            panic!("conformance observe panic");
        }
        crate::ActivityPackV1::observe(&crate::counter::CounterV2, input)
    }
}

#[cfg(any(test, feature = "conformance-tracer"))]
struct SemanticMismatchCounterV2;

#[cfg(any(test, feature = "conformance-tracer"))]
impl crate::ActivityPackV1 for SemanticMismatchCounterV2 {
    fn descriptor(&self) -> &'static crate::PackRevisionDescriptorV1 {
        crate::ActivityPackV1::descriptor(&crate::counter::CounterV2)
    }

    fn initialize(
        &self,
        input: &crate::ActivityGenesisInputV1<'_>,
        cx: &crate::DeterministicContextV1<'_>,
    ) -> Result<crate::InitialOutputV1, crate::PackFaultV1> {
        crate::ActivityPackV1::initialize(&crate::counter::CounterV2, input, cx)
    }

    fn reduce(
        &self,
        input: &crate::ActivityReduceInputV1<'_>,
        cx: &crate::DeterministicContextV1<'_>,
    ) -> Result<crate::ActivityDispositionV1, crate::PackFaultV1> {
        let disposition = crate::ActivityPackV1::reduce(&crate::counter::CounterV2, input, cx)?;
        Ok(match disposition {
            crate::ActivityDispositionV1::Apply(mut apply) => {
                apply.next_activity_state =
                    canonical(br#"{"maximum_value":2,"private_ack_count":0,"value":1}"#);
                crate::ActivityDispositionV1::Apply(apply)
            }
            rejection @ crate::ActivityDispositionV1::Reject(_) => rejection,
        })
    }

    fn view(
        &self,
        input: &crate::ViewInputV1<'_>,
    ) -> Result<crate::PackViewV1, crate::PackFaultV1> {
        crate::ActivityPackV1::view(&crate::counter::CounterV2, input)
    }

    fn observe(
        &self,
        input: &crate::ObserveInputV1<'_>,
    ) -> Result<Option<crate::PackObservationV1>, crate::PackFaultV1> {
        crate::ActivityPackV1::observe(&crate::counter::CounterV2, input)
    }
}

#[cfg(any(test, feature = "conformance-tracer"))]
struct MalformedCounterV2 {
    operation: crate::ActivityPackOperationV1,
}

#[cfg(any(test, feature = "conformance-tracer"))]
impl crate::ActivityPackV1 for MalformedCounterV2 {
    fn descriptor(&self) -> &'static crate::PackRevisionDescriptorV1 {
        crate::ActivityPackV1::descriptor(&crate::counter::CounterV2)
    }

    fn initialize(
        &self,
        input: &crate::ActivityGenesisInputV1<'_>,
        cx: &crate::DeterministicContextV1<'_>,
    ) -> Result<crate::InitialOutputV1, crate::PackFaultV1> {
        if self.operation == crate::ActivityPackOperationV1::Initialize {
            return Ok(crate::InitialOutputV1 {
                initial_activity_state: canonical(br"{}"),
                timer_requests: Vec::new(),
            });
        }
        crate::ActivityPackV1::initialize(&crate::counter::CounterV2, input, cx)
    }

    fn reduce(
        &self,
        input: &crate::ActivityReduceInputV1<'_>,
        cx: &crate::DeterministicContextV1<'_>,
    ) -> Result<crate::ActivityDispositionV1, crate::PackFaultV1> {
        if self.operation == crate::ActivityPackOperationV1::Reduce {
            return Ok(crate::ActivityDispositionV1::Apply(
                crate::ActivityApplyV1 {
                    next_activity_state: canonical(br"{}"),
                    ordered_domain_events: Vec::new(),
                    timer_requests: Vec::new(),
                    ordered_attention_signals: Vec::new(),
                },
            ));
        }
        crate::ActivityPackV1::reduce(&crate::counter::CounterV2, input, cx)
    }

    fn view(
        &self,
        input: &crate::ViewInputV1<'_>,
    ) -> Result<crate::PackViewV1, crate::PackFaultV1> {
        if self.operation == crate::ActivityPackOperationV1::View {
            return Ok(crate::PackViewV1 {
                projection_schema: "worldstream/invalid-view/v1".to_owned(),
                projection: canonical(br"{}"),
                action_offers: Vec::new(),
            });
        }
        crate::ActivityPackV1::view(&crate::counter::CounterV2, input)
    }

    fn observe(
        &self,
        input: &crate::ObserveInputV1<'_>,
    ) -> Result<Option<crate::PackObservationV1>, crate::PackFaultV1> {
        if self.operation == crate::ActivityPackOperationV1::Observe {
            return Ok(Some(crate::PackObservationV1::new(
                "worldstream/invalid-observation/v1",
                canonical(br"{}"),
            )));
        }
        crate::ActivityPackV1::observe(&crate::counter::CounterV2, input)
    }
}

#[cfg(any(test, feature = "conformance-tracer"))]
struct InvalidTimerOutputCounterV2;

#[cfg(any(test, feature = "conformance-tracer"))]
impl crate::ActivityPackV1 for InvalidTimerOutputCounterV2 {
    fn descriptor(&self) -> &'static crate::PackRevisionDescriptorV1 {
        crate::ActivityPackV1::descriptor(&crate::counter::CounterV2)
    }

    fn initialize(
        &self,
        input: &crate::ActivityGenesisInputV1<'_>,
        cx: &crate::DeterministicContextV1<'_>,
    ) -> Result<crate::InitialOutputV1, crate::PackFaultV1> {
        crate::ActivityPackV1::initialize(&crate::counter::CounterV2, input, cx)
    }

    fn reduce(
        &self,
        input: &crate::ActivityReduceInputV1<'_>,
        cx: &crate::DeterministicContextV1<'_>,
    ) -> Result<crate::ActivityDispositionV1, crate::PackFaultV1> {
        let mut disposition = crate::ActivityPackV1::reduce(&crate::counter::CounterV2, input, cx)?;
        if let crate::ActivityDispositionV1::Apply(apply) = &mut disposition {
            apply
                .timer_requests
                .push(crate::TimerRequestV1::CancelCurrent {
                    timer_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FG0"),
                    expected_generation: crate::TimerGenerationV1::new(1)
                        .unwrap_or_else(|error| unreachable!("Timer generation: {error}")),
                });
        }
        Ok(disposition)
    }

    fn view(
        &self,
        input: &crate::ViewInputV1<'_>,
    ) -> Result<crate::PackViewV1, crate::PackFaultV1> {
        crate::ActivityPackV1::view(&crate::counter::CounterV2, input)
    }

    fn observe(
        &self,
        input: &crate::ObserveInputV1<'_>,
    ) -> Result<Option<crate::PackObservationV1>, crate::PackFaultV1> {
        crate::ActivityPackV1::observe(&crate::counter::CounterV2, input)
    }
}

impl CounterRevision {
    const fn transcript_digest(self) -> &'static str {
        match self {
            Self::V1 => COUNTER_V1_TRANSCRIPT_DIGEST,
            Self::V2 => COUNTER_V2_TRANSCRIPT_DIGEST,
            Self::V3 => COUNTER_V3_TRANSCRIPT_DIGEST,
        }
    }
}

fn counter_entry(
    selectable_for_new_rooms: bool,
    runnable_for_retained_rooms: bool,
    revision: CounterRevision,
) -> Result<PackRegistryEntryV1, PackRegistryErrorV1> {
    let (descriptor, lock, schemas, codecs, artifact_digest) = match revision {
        CounterRevision::V1 => counter_v1_revision(),
        CounterRevision::V2 => counter_v2_revision(),
        CounterRevision::V3 => counter_v3_revision(),
    };
    let corpus = counter_corpus(revision, descriptor.revision_digest.clone());
    let artifacts = PackRegistryArtifactsV1 {
        expected_revision_digest: descriptor.revision_digest.clone(),
        schemas: Some(schemas.clone()),
        codecs: Some(codecs.clone()),
        codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
        executor_artifact_digest: artifact_digest.clone(),
        golden_corpus_digest: corpus.digest()?,
        golden_corpus: Some(corpus),
    };
    let status = PackRegistryStatusV1 {
        selectable_for_new_rooms,
        runnable_for_retained_rooms,
    };
    Ok(match revision {
        CounterRevision::V1 => {
            PackRegistryEntryV1::counter_v1(lock.clone(), descriptor, artifacts, status)
        }
        CounterRevision::V2 => {
            PackRegistryEntryV1::counter_v2(lock.clone(), descriptor, artifacts, status)
        }
        CounterRevision::V3 => {
            PackRegistryEntryV1::counter_v3(lock.clone(), descriptor, artifacts, status)
        }
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "The frozen multi-revision corpus is audited as one exact fixture."
)]
fn counter_corpus(revision: CounterRevision, pack_digest: PackDigestV1) -> PackGoldenCorpusV1 {
    let participant = membership(
        PARTICIPANT,
        "01ARZ3NDEKTSV4RRFFQ69G5FD0",
        AccessModeV1::Participant,
        Some("counter"),
    );
    let spectator = membership(
        SPECTATOR,
        "01ARZ3NDEKTSV4RRFFQ69G5FD1",
        AccessModeV1::Spectator,
        None,
    );
    let operator = membership(
        OPERATOR,
        "01ARZ3NDEKTSV4RRFFQ69G5FD2",
        AccessModeV1::Operator,
        None,
    );
    let mut memberships = vec![participant, spectator, operator];
    if matches!(revision, CounterRevision::V3) {
        memberships.extend([
            membership_with(
                "01ARZ3NDEKTSV4RRFFQ69G5FC5",
                "01ARZ3NDEKTSV4RRFFQ69G5FD5",
                PrincipalKindV1::Agent,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter"),
            ),
            membership_with(
                "01ARZ3NDEKTSV4RRFFQ69G5FC6",
                "01ARZ3NDEKTSV4RRFFQ69G5FD6",
                PrincipalKindV1::Agent,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter"),
            ),
            membership_with(
                "01ARZ3NDEKTSV4RRFFQ69G5FC7",
                "01ARZ3NDEKTSV4RRFFQ69G5FD7",
                PrincipalKindV1::Agent,
                MembershipStandingV1::Suspended,
                AccessModeV1::Participant,
                Some("counter"),
            ),
            membership_with(
                "01ARZ3NDEKTSV4RRFFQ69G5FC8",
                "01ARZ3NDEKTSV4RRFFQ69G5FD8",
                PrincipalKindV1::Agent,
                MembershipStandingV1::Departed,
                AccessModeV1::Participant,
                Some("counter"),
            ),
        ]);
    }
    let core = CoreRoomStateV1::active(memberships)
        .unwrap_or_else(|error| unreachable!("authored Counter Core fixture: {error}"));
    let configuration = match revision {
        CounterRevision::V1 => canonical(br#"{"initial_value":0,"maximum_value":1}"#),
        CounterRevision::V2 | CounterRevision::V3 => {
            canonical(br#"{"initial_value":0,"maximum_value":2}"#)
        }
    };
    let descriptor = match revision {
        CounterRevision::V1 => counter_v1_revision().0,
        CounterRevision::V2 => counter_v2_revision().0,
        CounterRevision::V3 => counter_v3_revision().0,
    };
    let empty_payload = canonical(br"{}");
    PackGoldenCorpusV1 {
        corpus_id: "worldstream/pack-golden-corpus/v1".to_owned(),
        genesis: PackGenesisRequestV1 {
            room_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            pack_digest,
            configuration,
            room_seed: parsed(
                "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            ),
            created_at: parsed("2026-08-15T12:00:00Z"),
            initial_core_state: core,
        },
        viewers: vec![
            viewer(PackGoldenViewerKindV1::Public, SPECTATOR, 0, None),
            viewer(PackGoldenViewerKindV1::Participant, PARTICIPANT, 0, None),
            viewer(PackGoldenViewerKindV1::Operator, OPERATOR, 0, None),
            viewer(PackGoldenViewerKindV1::Historical, SPECTATOR, 0, None),
            viewer(PackGoldenViewerKindV1::Historical, PARTICIPANT, 0, None),
            viewer(PackGoldenViewerKindV1::Historical, OPERATOR, 0, None),
            viewer(
                PackGoldenViewerKindV1::FinalReveal,
                PARTICIPANT,
                2,
                Some("Counter final reveal is unavailable before completion"),
            ),
        ],
        actions: vec![
            PackGoldenActionV1 {
                member_id: parsed(PARTICIPANT),
                action_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC3"),
                action_type: "private_ack".to_owned(),
                payload_schema_digest: descriptor.actions[1].payload_schema.schema_digest.clone(),
                canonical_payload: empty_payload.clone(),
                admitted_at: parsed("2026-08-15T12:00:01Z"),
            },
            PackGoldenActionV1 {
                member_id: parsed(PARTICIPANT),
                action_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC4"),
                action_type: "increment".to_owned(),
                payload_schema_digest: descriptor.actions[0].payload_schema.schema_digest.clone(),
                canonical_payload: empty_payload,
                admitted_at: parsed("2026-08-15T12:00:02Z"),
            },
        ],
        external_inputs: Vec::new(),
        expected_transcript_digest: parsed(revision.transcript_digest()),
    }
}

fn viewer(
    kind: PackGoldenViewerKindV1,
    member_id: &str,
    available_after_action: u32,
    denied_before_detail: Option<&str>,
) -> PackGoldenViewerV1 {
    PackGoldenViewerV1 {
        kind,
        member_id: parsed(member_id),
        available_after_action,
        denied_before_detail: denied_before_detail.map(str::to_owned),
    }
}

fn membership(
    member_id: &str,
    principal_id: &str,
    access_mode: AccessModeV1,
    role: Option<&str>,
) -> MembershipV1 {
    membership_with(
        member_id,
        principal_id,
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        access_mode,
        role,
    )
}

fn membership_with(
    member_id: &str,
    principal_id: &str,
    principal_kind: PrincipalKindV1,
    standing: MembershipStandingV1,
    access_mode: AccessModeV1,
    role: Option<&str>,
) -> MembershipV1 {
    MembershipV1::new(
        parsed(member_id),
        parsed(principal_id),
        principal_kind,
        standing,
        access_mode,
        role.map(str::to_owned),
    )
    .unwrap_or_else(|error| unreachable!("authored Counter Membership fixture: {error}"))
}

fn canonical(bytes: &[u8]) -> CanonicalJsonV1 {
    CanonicalJsonV1::parse(bytes)
        .unwrap_or_else(|error| unreachable!("authored Counter canonical fixture: {error}"))
}

fn parsed<T>(value: &str) -> T
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("authored Counter fixture value {value}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CoreTraceV1, ParticipantActionV1, RecordedStimulusV1,
        activity_pack::{
            author_golden_transcript_digest_for_test, author_golden_transcript_for_test,
        },
        counter::{CounterV1, CounterV2},
        counter_attention::CounterV3,
    };

    fn transcript(revision: CounterRevision) -> serde_json::Value {
        let canonical = match revision {
            CounterRevision::V1 => {
                let (descriptor, lock, schemas, codecs, artifact) = counter_v1_revision();
                let corpus =
                    counter_corpus(CounterRevision::V1, descriptor.revision_digest.clone());
                author_golden_transcript_for_test(
                    lock.clone(),
                    descriptor,
                    schemas.clone(),
                    codecs.clone(),
                    artifact.clone(),
                    &corpus,
                    CounterV1,
                )
            }
            CounterRevision::V2 => {
                let (descriptor, lock, schemas, codecs, artifact) = counter_v2_revision();
                let corpus =
                    counter_corpus(CounterRevision::V2, descriptor.revision_digest.clone());
                author_golden_transcript_for_test(
                    lock.clone(),
                    descriptor,
                    schemas.clone(),
                    codecs.clone(),
                    artifact.clone(),
                    &corpus,
                    CounterV2,
                )
            }
            CounterRevision::V3 => {
                let (descriptor, lock, schemas, codecs, artifact) = counter_v3_revision();
                let corpus =
                    counter_corpus(CounterRevision::V3, descriptor.revision_digest.clone());
                author_golden_transcript_for_test(
                    lock.clone(),
                    descriptor,
                    schemas.clone(),
                    codecs.clone(),
                    artifact.clone(),
                    &corpus,
                    CounterV3,
                )
            }
        }
        .unwrap_or_else(|()| unreachable!("authored Counter transcript"));
        serde_json::from_slice(
            &canonical
                .to_bytes()
                .unwrap_or_else(|error| unreachable!("canonical Counter transcript: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("Counter transcript JSON: {error}"))
    }

    #[test]
    fn reviewed_transcript_digests_are_literal_and_current() {
        let (descriptor, lock, schemas, codecs, artifact) = counter_v1_revision();
        let corpus = counter_corpus(CounterRevision::V1, descriptor.revision_digest.clone());
        let v1 = author_golden_transcript_digest_for_test(
            lock.clone(),
            descriptor,
            schemas.clone(),
            codecs.clone(),
            artifact.clone(),
            &corpus,
            CounterV1,
        )
        .unwrap_or_else(|()| unreachable!("authored Counter v1 transcript"));
        let (descriptor, lock, schemas, codecs, artifact) = counter_v2_revision();
        let corpus = counter_corpus(CounterRevision::V2, descriptor.revision_digest.clone());
        let v2 = author_golden_transcript_digest_for_test(
            lock.clone(),
            descriptor,
            schemas.clone(),
            codecs.clone(),
            artifact.clone(),
            &corpus,
            CounterV2,
        )
        .unwrap_or_else(|()| unreachable!("authored Counter v2 transcript"));
        assert_eq!(v1, parsed(COUNTER_V1_TRANSCRIPT_DIGEST));
        assert_eq!(v2, parsed(COUNTER_V2_TRANSCRIPT_DIGEST));
        let (descriptor, lock, schemas, codecs, artifact) = counter_v3_revision();
        let corpus = counter_corpus(CounterRevision::V3, descriptor.revision_digest.clone());
        let v3 = author_golden_transcript_digest_for_test(
            lock.clone(),
            descriptor,
            schemas.clone(),
            codecs.clone(),
            artifact.clone(),
            &corpus,
            CounterV3,
        )
        .unwrap_or_else(|()| unreachable!("authored Counter v3 transcript"));
        assert_eq!(v3, parsed(COUNTER_V3_TRANSCRIPT_DIGEST));
    }

    #[test]
    fn counter_revisions_remain_retained_while_v2_and_v3_are_conformance_selectable() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("reviewed Counter registry: {error}"));
        let v1 = registry
            .load_retained(&counter_v1_digest())
            .unwrap_or_else(|error| unreachable!("Counter v1 retained: {error}"));
        let v2 = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|error| unreachable!("Counter v2 retained: {error}"));
        let v3 = registry
            .load_retained(&counter_v3_digest())
            .unwrap_or_else(|error| unreachable!("Counter v3 retained: {error}"));
        assert!(!v1.status().selectable_for_new_rooms);
        assert!(v1.status().runnable_for_retained_rooms);
        assert!(v2.status().selectable_for_new_rooms);
        assert!(v2.status().runnable_for_retained_rooms);
        assert!(v3.status().selectable_for_new_rooms);
        assert!(v3.status().runnable_for_retained_rooms);
        assert!(registry.select_for_new_room(&counter_v1_digest()).is_err());
        assert!(registry.select_for_new_room(&counter_v2_digest()).is_ok());
        assert!(registry.select_for_new_room(&counter_v3_digest()).is_ok());
    }

    #[test]
    fn counter_transcripts_make_revision_privacy_and_offer_semantics_readable() {
        let v1 = transcript(CounterRevision::V1);
        let v2 = transcript(CounterRevision::V2);
        let v3 = transcript(CounterRevision::V3);

        assert_eq!(
            v1["steps"][1]["disposition"]["next_activity_state"]["value"],
            1
        );
        assert_eq!(
            v2["steps"][1]["disposition"]["next_activity_state"]["value"],
            2
        );
        assert_eq!(
            v1["steps"][1]["disposition"]["ordered_domain_events"][0]["delta"],
            1
        );
        assert_eq!(
            v2["steps"][1]["disposition"]["ordered_domain_events"][0]["delta"],
            2
        );
        assert_eq!(
            v3["steps"][0]["disposition"]["ordered_attention_signals"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            v3["steps"][0]["disposition"]["ordered_attention_signals"][0]["target_member_id"],
            "01ARZ3NDEKTSV4RRFFQ69G5FC5"
        );
        assert_eq!(
            v3["steps"][0]["disposition"]["ordered_attention_signals"][1]["target_member_id"],
            "01ARZ3NDEKTSV4RRFFQ69G5FC6"
        );

        for transcript in [&v1, &v2] {
            let private_ack_observations = &transcript["steps"][0]["observations"];
            assert_eq!(
                private_ack_observations[0]["observation"]["outcome"],
                "hidden"
            );
            assert_eq!(
                private_ack_observations[1]["observation"]["outcome"],
                "observation"
            );
            assert_eq!(
                private_ack_observations[2]["observation"]["outcome"],
                "hidden"
            );

            assert_eq!(transcript["initial_views"][6]["outcome"], "unavailable");
            assert_eq!(
                transcript["steps"][1]["after_views"][6]["outcome"],
                "available"
            );

            let observation_offers =
                &transcript["steps"][1]["observations"][1]["observation"]["value"]["action_offers"];
            let after_view_offers =
                &transcript["steps"][1]["after_views"][1]["view"]["action_offers"];
            assert_eq!(observation_offers, after_view_offers);
            assert_eq!(after_view_offers.as_array().map(Vec::len), Some(1));
            assert_eq!(after_view_offers[0]["action_type"], "private_ack");
        }
    }

    #[test]
    fn counter_v3_human_ack_emits_eligible_agent_attention_and_replays_without_effects() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("reviewed Counter registry: {error}"));
        let corpus = counter_corpus(CounterRevision::V3, counter_v3_digest());
        let prepared = registry
            .prepare_genesis_for_new_room(&corpus.genesis)
            .unwrap_or_else(|error| unreachable!("Counter v3 genesis: {error}"));
        let mut trace = CoreTraceV1::create_uncommitted(prepared)
            .unwrap_or_else(|error| unreachable!("Counter v3 trace: {error}"));
        let private_ack = trace
            .retained_pack()
            .unwrap_or_else(|| unreachable!("Counter v3 retained pack"))
            .descriptor()
            .actions
            .iter()
            .find(|action| action.action_type == "private_ack")
            .unwrap_or_else(|| unreachable!("Counter v3 private acknowledgement"));
        trace
            .advance(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
                member_id: parsed(PARTICIPANT),
                action_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC3"),
                action_type: "private_ack".to_owned(),
                payload_schema_digest: private_ack.payload_schema.schema_digest.clone(),
                canonical_payload: canonical(br"{}"),
                exact_basis_head: trace.head().clone(),
                admitted_at: parsed("2026-08-15T12:00:01Z"),
            }))
            .unwrap_or_else(|error| unreachable!("Counter v3 acknowledgement: {error}"));

        let transition = trace
            .transitions()
            .first()
            .unwrap_or_else(|| unreachable!("Counter v3 transition"));
        let targets = transition
            .ordered_attention_signals()
            .iter()
            .map(|signal| {
                serde_json::from_slice::<serde_json::Value>(
                    &signal
                        .to_bytes()
                        .unwrap_or_else(|error| unreachable!("Counter v3 Attention bytes: {error}")),
                )
                .unwrap_or_else(|error| unreachable!("Counter v3 Attention JSON: {error}"))
                ["target_member_id"]
                    .as_str()
                    .unwrap_or_else(|| unreachable!("Counter v3 Attention target"))
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            targets,
            [
                "01ARZ3NDEKTSV4RRFFQ69G5FC5".to_owned(),
                "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
            ]
        );
        assert!(transition.ordered_attention_signals().iter().all(|signal| {
            signal.to_bytes().is_ok_and(|bytes| {
                bytes.windows(b"FC7".len()).all(|window| window != b"FC7")
                    && bytes.windows(b"FC8".len()).all(|window| window != b"FC8")
            })
        }));

        let replay = CoreTraceV1::replay(
            &registry,
            &trace
                .genesis_bytes()
                .unwrap_or_else(|error| unreachable!("Counter v3 Genesis bytes: {error}")),
            &trace
                .transition_bytes()
                .unwrap_or_else(|error| unreachable!("Counter v3 Transition bytes: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("Counter v3 replay: {error:?}"));
        assert_eq!(replay.final_head, *trace.head());
        assert_eq!(replay.activity_callback_count, 1);
        assert_eq!(replay.external_effect_count, 0);
        assert_eq!(replay.receipt_count, 0);

        let prepared = registry
            .prepare_genesis_for_new_room(&corpus.genesis)
            .unwrap_or_else(|error| unreachable!("Counter v3 agent genesis: {error}"));
        let mut agent_trace = CoreTraceV1::create_uncommitted(prepared)
            .unwrap_or_else(|error| unreachable!("Counter v3 agent trace: {error}"));
        agent_trace
            .advance(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
                member_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC5"),
                action_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC4"),
                action_type: "private_ack".to_owned(),
                payload_schema_digest: private_ack.payload_schema.schema_digest.clone(),
                canonical_payload: canonical(br"{}"),
                exact_basis_head: agent_trace.head().clone(),
                admitted_at: parsed("2026-08-15T12:00:01Z"),
            }))
            .unwrap_or_else(|error| unreachable!("Counter v3 agent acknowledgement: {error}"));
        assert!(
            agent_trace.transitions()[0]
                .ordered_attention_signals()
                .is_empty()
        );
    }

    #[test]
    fn counter_v3_compatibility_metadata_is_fixed() {
        let (descriptor, lock, schemas, codecs, artifact) = counter_v3_revision();
        assert_eq!(
            descriptor.revision_digest,
            parsed("blake3:7572a62b364fb9c88c02d79c85efba9e5b9cef22211da4a66827f64704970a55")
        );
        assert_eq!(
            lock.descriptor_digest,
            parsed("blake3:ac40f0ca14fe3c16884cc7548efdc8b456296f6956759bb9411503896415d06f")
        );
        assert_eq!(
            *artifact,
            parsed("blake3:2f8203ebeae8948d6c4c9113c26850c1e7d95c79301e7ae2161d040563626e20")
        );
        assert_eq!(
            schemas
                .digest()
                .unwrap_or_else(|error| unreachable!("Counter v3 schema digest: {error}")),
            parsed("blake3:bfb839aeaabf089a318d0b14ef403bfdee0f49883111c7a60c7f8158af177e8f")
        );
        assert_eq!(
            codecs
                .digest()
                .unwrap_or_else(|error| unreachable!("Counter v3 codec digest: {error}")),
            parsed("blake3:67b814baf1511b6c29ffe9862eeeb2b130988972c9c3a2c2ab6ff1f7018a6b30")
        );
        assert_eq!(
            counter_corpus(CounterRevision::V3, descriptor.revision_digest.clone())
                .digest()
                .unwrap_or_else(|error| unreachable!("Counter v3 corpus digest: {error}")),
            parsed(COUNTER_V3_CORPUS_DIGEST)
        );
    }
}
