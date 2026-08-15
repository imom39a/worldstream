//! Embedded retained registry rows for the two Counter conformance revisions.
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
    PackRegistryV1::try_new([v1, v2])
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

#[derive(Clone, Copy)]
enum CounterRevision {
    V1,
    V2,
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
    })
}

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
    let core = CoreRoomStateV1::active([participant, spectator, operator])
        .unwrap_or_else(|error| unreachable!("authored Counter Core fixture: {error}"));
    let configuration = match revision {
        CounterRevision::V1 => canonical(br#"{"initial_value":0,"maximum_value":1}"#),
        CounterRevision::V2 => canonical(br#"{"initial_value":0,"maximum_value":2}"#),
    };
    let descriptor = match revision {
        CounterRevision::V1 => counter_v1_revision().0,
        CounterRevision::V2 => counter_v2_revision().0,
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
    MembershipV1::new(
        parsed(member_id),
        parsed(principal_id),
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
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
        activity_pack::{
            author_golden_transcript_digest_for_test, author_golden_transcript_for_test,
        },
        counter::{CounterV1, CounterV2},
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
    }

    #[test]
    fn both_counter_revisions_are_retained_but_only_v2_is_conformance_selectable() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("reviewed Counter registry: {error}"));
        let v1 = registry
            .load_retained(&counter_v1_digest())
            .unwrap_or_else(|error| unreachable!("Counter v1 retained: {error}"));
        let v2 = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|error| unreachable!("Counter v2 retained: {error}"));
        assert!(!v1.status().selectable_for_new_rooms);
        assert!(v1.status().runnable_for_retained_rooms);
        assert!(v2.status().selectable_for_new_rooms);
        assert!(v2.status().runnable_for_retained_rooms);
        assert!(registry.select_for_new_room(&counter_v1_digest()).is_err());
        assert!(registry.select_for_new_room(&counter_v2_digest()).is_ok());
    }

    #[test]
    fn counter_transcripts_make_revision_privacy_and_offer_semantics_readable() {
        let v1 = transcript(CounterRevision::V1);
        let v2 = transcript(CounterRevision::V2);

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
}
