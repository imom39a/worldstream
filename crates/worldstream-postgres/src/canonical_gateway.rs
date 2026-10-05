//! Format-neutral production serving entrypoints. SQL commits retain one shared persistence path.
use super::*;
use worldstream_core::{
    CoreRoomStateV1, PreparedCanonicalRoomCommit, ValidatedPackViewV1,
    commit_canonical_existing_room,
};

pub(super) trait PostgresServingTrace {
    fn head(&self) -> &CompleteHeadV1;
    fn core_state(&self) -> &CoreRoomStateV1;
    fn view(
        &self,
        registry: &PackRegistryV1,
        viewer: &PackViewerV1,
    ) -> Result<ValidatedPackViewV1, PostgresActivationError>;
}
impl PostgresServingTrace for CoreTraceV1 {
    fn head(&self) -> &CompleteHeadV1 {
        self.head()
    }
    fn core_state(&self) -> &CoreRoomStateV1 {
        self.core_state()
    }
    fn view(
        &self,
        registry: &PackRegistryV1,
        viewer: &PackViewerV1,
    ) -> Result<ValidatedPackViewV1, PostgresActivationError> {
        let retained = registry
            .load_retained(self.head().pack_digest())
            .map_err(|_| PostgresActivationError::Corrupt)?;
        if self
            .retained_pack()
            .is_some_and(|bound| bound.revision_lock() != retained.revision_lock())
        {
            return Err(PostgresActivationError::Corrupt);
        }
        retained
            .host()
            .view(&worldstream_core::ViewInputV1 {
                core: self.core_state(),
                activity_state: self.activity_state(),
                complete_head: self.head(),
                viewer,
            })
            .map_err(|_| PostgresActivationError::Corrupt)
    }
}
impl PostgresServingTrace for CanonicalRoomTrace {
    fn head(&self) -> &CompleteHeadV1 {
        self.head()
    }
    fn core_state(&self) -> &CoreRoomStateV1 {
        self.core_state()
    }
    fn view(
        &self,
        registry: &PackRegistryV1,
        viewer: &PackViewerV1,
    ) -> Result<ValidatedPackViewV1, PostgresActivationError> {
        let retained = registry
            .load_retained(self.head().pack_digest())
            .map_err(|_| PostgresActivationError::Corrupt)?;
        if self.retained_pack().revision_lock() != retained.revision_lock() {
            return Err(PostgresActivationError::Corrupt);
        }
        self.view(viewer)
            .map_err(|_| PostgresActivationError::Corrupt)
    }
}
impl PostgresRoomStore {
    pub(super) fn resolve_canonical_admission_receipt(
        &self,
        authority: worldstream_core::AuthorizedReceiptReadV1,
    ) -> Result<Option<RoomCommitResolutionV1>, PostgresRoomCommitError> {
        match AuthorizedReceiptResolverV1::resolve_authorized(self, authority) {
            Ok(ResolveOutcomeV1::StoredResolution(result)) => Ok(Some(
                RoomCommitResolutionV1::resolved(ResolutionStatusV1::Existing, *result),
            )),
            Ok(ResolveOutcomeV1::Conflict {
                existing_request_hash,
            }) => Ok(Some(RoomCommitResolutionV1::Conflict {
                existing_request_hash,
            })),
            Ok(ResolveOutcomeV1::KnownAbsent) => Ok(None),
            Ok(ResolveOutcomeV1::ResolutionUnavailable) => {
                Ok(Some(RoomCommitResolutionV1::Indeterminate))
            }
            Err(AuthorityErrorV1::Unavailable) => Err(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::StorageUnavailable,
            )),
            Err(_) => Ok(Some(RoomCommitResolutionV1::Fenced)),
        }
    }

    /// Recovers a uniquely owned executable trace using immutable Genesis selection.
    pub fn recover_canonical_room(
        &self,
        registry: &PackRegistryV1,
        room_id: &str,
    ) -> Result<Option<CanonicalRoomTrace>, RoomRecoveryErrorV1> {
        let room_id = room_id
            .parse::<RoomId>()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        worldstream_core::recover_canonical_room_from_storage(self, registry, &room_id)
    }
    /// Replays retained canonical history for conformance without exposing a production raw-write API.
    #[cfg(feature = "conformance-tracer")]
    pub fn recover_canonical_conformance_trace(
        &self,
        registry: &PackRegistryV1,
        room_id: &str,
    ) -> Result<CanonicalRoomTrace, PostgresConformanceError> {
        let verification = self
            .verify_room(room_id)
            .map_err(|error| PostgresConformanceError::Verification(error.to_string()))?;
        CanonicalRoomTrace::replay(
            registry,
            &verification.genesis_bytes,
            &verification.transition_bytes,
        )
        .map(worldstream_core::CanonicalReplayReport::into_trace)
        .map_err(|error| PostgresConformanceError::Replay(format!("{error:?}")))
    }
    /// Enables only the existing checked conformance capability-witness branch for this sealed canonical write.
    #[cfg(feature = "conformance-tracer")]
    pub fn commit_canonical_conformance_write_for_conformance(
        &self,
        prepared: &PreparedCanonicalRoomWrite,
    ) -> RoomCommitResolutionV1 {
        self.conformance_capability_commit
            .store(true, Ordering::SeqCst);
        let result = CanonicalRoomCommitStorage::commit(self, prepared);
        self.conformance_capability_commit
            .store(false, Ordering::SeqCst);
        result
    }
    /// Exercises the production keyset replay reader against one persisted Room.
    #[cfg(feature = "conformance-tracer")]
    pub fn verify_canonical_stream_room_for_conformance(
        &self,
        registry: &PackRegistryV1,
        room_id: &str,
    ) -> Result<(), PostgresRoomVerificationError> {
        let mut client = self
            .connect()
            .map_err(PostgresRoomVerificationError::Connection)?;
        let mut transaction = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .map_err(PostgresRoomVerificationError::Sql)?;
        Self::verify_stream_room_executable_replay_in_transaction(
            &mut transaction,
            room_id,
            registry,
        )?;
        transaction
            .commit()
            .map_err(PostgresRoomVerificationError::Sql)
    }
    /// Prepares a canonical cached claim through the shared operational transaction.
    pub fn prepare_activation_claim_from_canonical_serving_trace(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedRunnerControlV1,
        request: ActivationOperationRequestV1,
        trace: &CanonicalRoomTrace,
        integrity: &RoomIntegrityStateV1,
    ) -> Result<PostgresActivationClaimPreparationV1, PostgresActivationError> {
        self.prepare_activation_claim_from_verified_trace(
            registry, authority, request, trace, integrity,
        )
    }
    /// Resolves the exact durable receipt before fresh Action preparation.
    ///
    /// # Errors
    /// Returns a preparation or storage error when the request cannot be admitted.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_participant_action_from_canonical_serving_trace(
        &self,
        authority: ParticipantActionAuthorityV1,
        request: &ParticipantActionRequestV1,
        stimulus: ParticipantActionV1,
        transition_id: worldstream_core::TransitionId,
        trace: &mut CanonicalRoomTrace,
        integrity_generation: IntegrityGenerationV1,
        frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<RoomCommitResolutionV1, PostgresRoomCommitError> {
        let receipt_authority =
            worldstream_core::AuthorizedReceiptReadV1::from_participant_action_authority(
                &authority, request,
            )
            .map_err(|_| PostgresRoomCommitError::Preparation)?;
        if let Some(result) = self.resolve_canonical_admission_receipt(receipt_authority)? {
            return Ok(result);
        }
        if stimulus.exact_basis_head.room_id() != trace.head().room_id() {
            return Err(PostgresRoomCommitError::Preparation);
        }
        let admitted_at = stimulus.admitted_at.clone();
        let prepared = if request.based_on_room_seq() == trace.head().room_seq() {
            let prepared_transition =
                match trace.prepare(RecordedStimulusV1::ParticipantAction(stimulus)) {
                    Ok(prepared) => Some(prepared),
                    Err(
                        TraceErrorV1::ActionAdmission(_)
                        | TraceErrorV1::ArchivedStimulusForbidden
                        | TraceErrorV1::CompleteHeadMismatch,
                    ) => None,
                    Err(TraceErrorV1::PayloadBudget(_)) => {
                        return Err(PostgresRoomCommitError::Rejected);
                    }
                    Err(_) => return Err(PostgresRoomCommitError::Preparation),
                };
            match prepared_transition {
                Some(prepared) => match authority {
                    ParticipantActionAuthorityV1::EnabledParticipant(authority) => {
                        PreparedCanonicalRoomCommit::for_authorized_action(
                            trace,
                            request,
                            prepared,
                            transition_id,
                            integrity_generation,
                            authority,
                            frame_heads,
                        )
                        .map_err(canonical_preparation_error)?
                    }
                    ParticipantActionAuthorityV1::StableMembershipNotEnabled(authority) => {
                        PreparedCanonicalRoomCommit::for_authorized_stable_action_disposition(
                            trace,
                            request,
                            admitted_at.clone(),
                            integrity_generation,
                            ParticipantActionAuthorityV1::StableMembershipNotEnabled(authority),
                        )
                        .map_err(canonical_preparation_error)?
                    }
                },
                None => PreparedCanonicalRoomCommit::for_authorized_stable_action_disposition(
                    trace,
                    request,
                    admitted_at,
                    integrity_generation,
                    authority,
                )
                .map_err(canonical_preparation_error)?,
            }
        } else {
            PreparedCanonicalRoomCommit::for_authorized_stable_action_disposition(
                trace,
                request,
                admitted_at,
                integrity_generation,
                authority,
            )
            .map_err(canonical_preparation_error)?
        };
        Ok(commit_canonical_existing_room(self, trace, prepared)
            .into_parts()
            .0)
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Result::map_err transfers the closed error by value."
)]
fn canonical_preparation_error(
    error: worldstream_core::PrepareRoomWriteErrorV1,
) -> PostgresRoomCommitError {
    match error {
        worldstream_core::PrepareRoomWriteErrorV1::Trace(TraceErrorV1::PayloadBudget(_)) => {
            PostgresRoomCommitError::Rejected
        }
        _ => PostgresRoomCommitError::Preparation,
    }
}

pub(super) fn require_legacy_recovery_format(
    store: &PostgresRoomStore,
    room_id: &RoomId,
) -> Result<(), RoomRecoveryErrorV1> {
    let mut client = store
        .connect()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let row = client
        .query_opt(
            "SELECT genesis_bytes FROM worldstream_genesis WHERE room_id=$1",
            &[&room_id.as_str()],
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    if let Some(row) = row {
        let bytes: Vec<u8> = row.try_get(0).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if GenesisRecord::from_canonical_bytes(&bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            .format()
            != worldstream_core::CanonicalHistoryFormat::V1
        {
            return Err(RoomRecoveryErrorV1::RuntimeUnavailable);
        }
    }
    Ok(())
}
