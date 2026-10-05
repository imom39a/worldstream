//! Paired exact-executor Core preparation. The adapter below is NOT durable storage.
//! Build with --features conformance-tracer. No database or recovery is measured.
#[cfg(not(feature = "conformance-tracer"))]
fn main() {
    eprintln!("build this example with --features conformance-tracer");
    std::process::exit(2);
}

#[cfg(feature = "conformance-tracer")]
mod measurement {
    use anyhow::{Context, Result, bail, ensure};
    use serde_json::{Value, json};
    use std::{collections::BTreeMap, env, fs, str::FromStr, time::Instant};
    use worldstream_core::*;

    const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
    const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    const TIME: &str = "2026-08-15T12:00:00Z";

    // This adapter returns the sealed Core result. It never writes a database.
    struct InMemoryResultAdapter;
    impl CanonicalRoomCommitStorage for InMemoryResultAdapter {
        fn commit(&self, write: &PreparedCanonicalRoomWrite) -> RoomCommitResolutionV1 {
            RoomCommitResolutionV1::resolved(
                ResolutionStatusV1::New,
                write.semantic_result().clone(),
            )
        }
        fn resolve(&self, _: &OperationIdentityV1, _: &CanonicalRequestHashV1) -> ResolveOutcomeV1 {
            ResolveOutcomeV1::KnownAbsent
        }
    }

    fn parsed<T: FromStr>(value: &str) -> Result<T>
    where
        T::Err: std::fmt::Display,
    {
        value.parse().map_err(|error| anyhow::anyhow!("{error}"))
    }
    fn canonical(value: Value) -> Result<CanonicalJsonV1> {
        Ok(CanonicalJsonV1::parse(&serde_json::to_vec(&value)?)?)
    }
    fn authority() -> Result<PreparedAuthorityWitnessV1> {
        Ok(PreparedAuthorityWitnessV1::mint_for_conformance(
            "benchmark-in-memory-authority",
            parsed(PRINCIPAL)?,
            1,
            &canonical(json!({"scope":"benchmark","revoked":false}))?,
        )?)
    }
    fn argument(flag: &str) -> Result<String> {
        let args: Vec<_> = env::args().collect();
        let index = args
            .iter()
            .position(|arg| arg == flag)
            .with_context(|| format!("missing {flag}"))?;
        args.get(index + 1)
            .cloned()
            .with_context(|| format!("missing value for {flag}"))
    }
    fn create(
        registry: &PackRegistryV1,
        digest: &PackDigestV1,
        count: u64,
        size: usize,
        format: CanonicalHistoryFormat,
    ) -> Result<CanonicalRoomTrace> {
        let configuration = canonical(json!({"state_bytes":size,"maximum_counter":count}))?;
        let core = CoreRoomStateV1::active([MembershipV1::new(
            parsed(MEMBER)?,
            parsed(PRINCIPAL)?,
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )?])?;
        let genesis = registry.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
            room_id: parsed(ROOM)?,
            pack_digest: digest.clone(),
            configuration: configuration.clone(),
            room_seed: parsed(
                "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            )?,
            created_at: parsed(TIME)?,
            initial_core_state: core,
        })?;
        let request = RoomCreationRequestWithFormat::new(
            RoomCreationRequestV1::new(
                digest.clone(),
                configuration,
                vec![InitialMembershipProposalV1::new(
                    parsed(PRINCIPAL)?,
                    PrincipalKindV1::Human,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Participant,
                    Some("counter".to_owned()),
                )?],
            ),
            format,
        );
        let plan = PreparedCanonicalRoomCreation::from_registry_genesis_for_conformance(
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL)?,
                versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
                idempotency_key: "stream-benchmark".to_owned(),
            },
            &request,
            authority()?,
            genesis,
        )?;
        let outcome = commit_canonical_room_creation(&InMemoryResultAdapter, plan);
        ensure!(
            outcome.actor_installation() == ActorInstallationV1::Installed,
            "creation installation failed"
        );
        outcome.into_parts().2.context("new in-memory trace")
    }

    struct Cell {
        trace: CanonicalRoomTrace,
        format: CanonicalHistoryFormat,
        bytes: u64,
        genesis_bytes: usize,
        audit: blake3::Hasher,
        prepare_ns: u128,
        seal_install_ns: u128,
        encode_verify_ns: u128,
        samples: Vec<u128>,
        vectors: Vec<Value>,
        state_min: usize,
        state_max: usize,
        record_min: usize,
        record_max: usize,
    }
    impl Cell {
        fn new(trace: CanonicalRoomTrace, format: CanonicalHistoryFormat) -> Result<Self> {
            let genesis = trace.genesis_bytes()?;
            GenesisRecord::from_canonical_bytes(&genesis)?.verify()?;
            let mut audit = blake3::Hasher::new();
            audit.update(&(genesis.len() as u64).to_be_bytes());
            audit.update(&genesis);
            let state = trace.activity_state().to_bytes()?.len();
            Ok(Self {
                trace,
                format,
                bytes: 0,
                genesis_bytes: genesis.len(),
                audit,
                prepare_ns: 0,
                seal_install_ns: 0,
                encode_verify_ns: 0,
                samples: Vec::new(),
                vectors: Vec::new(),
                state_min: state,
                state_max: state,
                record_min: usize::MAX,
                record_max: 0,
            })
        }
        fn step(&mut self, seq: u64, sample: bool) -> Result<TransitionRecord> {
            let begin = Instant::now();
            let action_id = format!("{seq:026X}");
            let basis = self.trace.head().clone();
            let action = self
                .trace
                .retained_pack()
                .descriptor()
                .actions
                .iter()
                .find(|action| action.action_type == "increment")
                .context("increment definition")?;
            let stimulus = RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
                member_id: parsed(MEMBER)?,
                action_id: parsed(&action_id)?,
                action_type: "increment".to_owned(),
                payload_schema_digest: action.payload_schema.schema_digest.clone(),
                canonical_payload: canonical(json!({}))?,
                exact_basis_head: basis.clone(),
                admitted_at: parsed("2026-08-15T12:00:01Z")?,
            });
            let at = Instant::now();
            let prepared = self.trace.prepare(stimulus)?;
            self.prepare_ns += at.elapsed().as_nanos();
            let CanonicalAdvanceDisposition::TransitionAccepted {
                existing: false,
                transition,
            } = prepared.disposition()
            else {
                bail!("benchmark Action did not create a Transition")
            };
            let record = (**transition).clone();
            let at = Instant::now();
            let encoded = record.canonical_bytes()?;
            let decoded = TransitionRecord::from_canonical_bytes(self.format, &encoded)?;
            decoded.verify_successor(&basis)?;
            ensure!(decoded == record, "record changed on canonical round trip");
            self.audit.update(&(encoded.len() as u64).to_be_bytes());
            self.audit.update(&encoded);
            self.bytes += encoded.len() as u64;
            self.record_min = self.record_min.min(encoded.len());
            self.record_max = self.record_max.max(encoded.len());
            self.encode_verify_ns += at.elapsed().as_nanos();
            let at = Instant::now();
            let request = ParticipantActionRequestV1::new(
                parsed(ROOM)?,
                parsed(MEMBER)?,
                parsed(&action_id)?,
                basis.room_seq(),
                "increment",
                canonical(json!({}))?,
            );
            let plan = PreparedCanonicalRoomCommit::for_action_for_conformance(
                &self.trace,
                &request,
                prepared,
                parsed(&format!("{:026X}", seq + 1000000))?,
                IntegrityGenerationV1::new(1)?,
                authority()?,
                &BTreeMap::from([(parsed(MEMBER)?, seq - 1)]),
            )?;
            let outcome =
                commit_canonical_existing_room(&InMemoryResultAdapter, &mut self.trace, plan);
            ensure!(
                outcome.actor_installation() == ActorInstallationV1::Installed,
                "in-memory installation failed"
            );
            self.trace.discard_persisted_history();
            self.seal_install_ns += at.elapsed().as_nanos();
            let state_bytes = self.trace.activity_state().to_bytes()?;
            self.state_min = self.state_min.min(state_bytes.len());
            self.state_max = self.state_max.max(state_bytes.len());
            let state: Value = serde_json::from_slice(&state_bytes)?;
            ensure!(
                state["counter"].as_u64() == Some(seq),
                "counter does not match accepted sequence"
            );
            if [1, 100, 10000, 100000].contains(&seq) {
                let canonical_bytes_blake3 = blake3::hash(&encoded).to_hex().to_string();
                self.vectors.push(json!({"room_seq":seq,"canonical_record":String::from_utf8(encoded)?,"transition_hash":record.transition_hash().to_string(),"canonical_bytes_blake3":canonical_bytes_blake3,"state_bytes":state_bytes.len()}));
            }
            if sample {
                self.samples.push(begin.elapsed().as_nanos());
            }
            Ok(record)
        }
        fn report(mut self) -> Value {
            self.samples.sort_unstable();
            let quantile = |p: usize| self.samples[self.samples.len().saturating_sub(1) * p / 100];
            json!({"format":format!("{:?}",self.format),"genesis_bytes":self.genesis_bytes,
                "transition_bytes_exact_sum":self.bytes,"record_min_bytes":self.record_min,"record_max_bytes":self.record_max,
                "stream_audit_digest":self.audit.finalize().to_hex().to_string(),"audit_framing":"u64 big-endian byte length followed by exact Genesis then each Transition",
                "activity_state_min_bytes":self.state_min,"activity_state_max_bytes":self.state_max,
                "actual_reduce_callbacks":self.trace.activity_callback_count(),
                "timing_ns":{"core_prepare_total":self.prepare_ns,"conformance_seal_and_actor_install_total":self.seal_install_ns,"canonical_encode_decode_verify_and_audit_total":self.encode_verify_ns},
                "sampled_step_latency_ns":{"p50":quantile(50),"p95":quantile(95),"p99":quantile(99),"max":self.samples.last(),"sample_count":self.samples.len(),"method":"systematic fixed stride, at most 2048; nearest lower sample index"},
                "sample_vectors":self.vectors,"final_head":self.trace.head(),"retained_history_records":self.trace.transitions().len()})
        }
    }
    pub fn run() -> Result<()> {
        let count = argument("--transitions")?.parse::<u64>()?;
        let size = argument("--state-bytes")?.parse::<usize>()?;
        ensure!(
            [100, 10000, 100000].contains(&count),
            "supported counts: 100,10000,100000"
        );
        ensure!(
            [1024, 16384, 65536, 262144].contains(&size),
            "unsupported state target"
        );
        let registry = benchmark_state_registry_for_conformance()?;
        let digest = registry
            .catalog_revisions()
            .next()
            .context("benchmark revision")?
            .revision_digest;
        let mut v1 = Cell::new(
            create(&registry, &digest, count, size, CanonicalHistoryFormat::V1)?,
            CanonicalHistoryFormat::V1,
        )?;
        let mut v2 = Cell::new(
            create(&registry, &digest, count, size, CanonicalHistoryFormat::V2)?,
            CanonicalHistoryFormat::V2,
        )?;
        ensure!(
            v1.trace.activity_state() == v2.trace.activity_state()
                && v1.trace.core_state() == v2.trace.core_state(),
            "initial state mismatch"
        );
        let begin = Instant::now();
        let stride = count.div_ceil(2048);
        for seq in 1..=count {
            let sample = (seq - 1) % stride == 0;
            let a = v1.step(seq, sample)?;
            let b = v2.step(seq, sample)?;
            ensure!(
                a.ordered_domain_events() == b.ordered_domain_events()
                    && a.ordered_timer_changes() == b.ordered_timer_changes()
                    && a.ordered_attention_signals() == b.ordered_attention_signals(),
                "ordered effects differ"
            );
            ensure!(
                a.resulting_core_state_hash() == b.resulting_core_state_hash()
                    && a.resulting_activity_state_hash() == b.resulting_activity_state_hash()
                    && a.resulting_authoritative_state_hash()
                        == b.resulting_authoritative_state_hash(),
                "state commitments differ"
            );
            ensure!(
                v1.trace.core_state() == v2.trace.core_state()
                    && v1.trace.activity_state() == v2.trace.activity_state(),
                "logical state differs"
            );
        }
        let report = json!({"schema":"worldstream/core-stream-scaling/v1","execution":"checked embedded exact executor","commit_adapter":"in-memory result echo; no durability", "pack_digest":digest,
            "transitions_per_format":count,"state_target_bytes":size,"paired_wall_ns":begin.elapsed().as_nanos(),"pair_checks":"state, ordered effects, commitments after every accepted step; per-format canonical round trip and successor",
            "cells":[v1.report(),v2.report()],"not_measured":["database commits","I/O","backup","transfer","checkpoints","recovery","full Replay","publication","portable Component callbacks","portable callback bytes"]});
        fs::write(
            argument("--output")?,
            format!("{}\n", serde_json::to_string_pretty(&report)?),
        )?;
        Ok(())
    }
}
#[cfg(feature = "conformance-tracer")]
fn main() -> anyhow::Result<()> {
    measurement::run()
}
