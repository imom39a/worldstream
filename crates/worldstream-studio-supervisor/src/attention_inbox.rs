//! Derived Home attention inbox with stable condition identity and minimal history.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::State,
    response::{IntoResponse as _, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_renameable_file, prepare_data_directory, validate_owner_only_file,
};

use crate::{
    backups::{
        BackupFreshnessV1, BackupOperationPhaseV1, BackupOperationStatusV1, BackupOperationsV1,
    },
    lifecycle::{DaemonLifecycleControl, DaemonLifecycleStateV1, DaemonLifecycleV1},
    managed_agent_host::{
        ManagedAgentHostFreshnessV1, ManagedAgentHostStateV1, ManagedAgentHostStatusV1,
    },
    room_creation::{RoomCreationStateV1, RoomCreationStatusV1, RoomCreationSupervisorV1},
    runner_attention::{
        ActivationAttentionStateV1, ApprovedRunnerRestartV1, RunnerAttentionErrorV1,
        RunnerAttentionFreshnessV1, RunnerAttentionOperationsResponseV1, RunnerAttentionSourceV1,
        RunnerAttentionSupervisorV1, RunnerRestartOperationStateV1, TaskAgentAttentionResponseV1,
    },
    task_setup::{
        TaskLaunchApplicabilityV1, TaskLaunchStateV1, TaskSetupStateV1, TaskSetupStatusV1,
        TaskSetupSupervisorV1,
    },
};

const INBOX_SCHEMA_V1: &str = "worldstream/studio-attention-inbox/v1";
const HISTORY_SCHEMA_V1: &str = "worldstream/studio-attention-history/v1";
const MAX_ACTIVE_ITEMS: usize = 256;
const MAX_RESOLVED_ITEMS: usize = 16;
const MAX_HISTORY_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionTargetKindV1 {
    Task,
    Process,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionConditionV1 {
    DaemonLifecycle,
    TaskSetup,
    TaskReadiness,
    TaskLaunch,
    RunnerPresence,
    RunnerCompatibility,
    RunnerCapacity,
    ActivationBacklog,
    ActivationLease,
    RunnerRestart,
    ManagedAgentHost,
    RoomCreation,
    BackupOperation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionItemV1 {
    pub attention_id: String,
    pub target_kind: AttentionTargetKindV1,
    pub condition: AttentionConditionV1,
    pub target_id: String,
    pub title: String,
    pub reason: String,
    pub freshness: RunnerAttentionFreshnessV1,
    pub deep_link: String,
    pub next_action: String,
    pub first_seen_at_unix_ms: u64,
    pub last_seen_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedAttentionItemV1 {
    pub attention_id: String,
    pub target_kind: AttentionTargetKindV1,
    pub condition: AttentionConditionV1,
    pub target_id: String,
    pub resolved_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionInboxResponseV1 {
    pub schema: String,
    pub observed_at_unix_ms: u64,
    pub items: Vec<AttentionItemV1>,
    pub recently_resolved: Vec<ResolvedAttentionItemV1>,
}

#[derive(Clone, Debug)]
pub struct AttentionInboxSnapshotV1 {
    pub observed_at_unix_ms: u64,
    pub lifecycle: DaemonLifecycleV1,
    pub task_setups: Vec<TaskSetupStatusV1>,
    pub room_creations: Vec<RoomCreationStatusV1>,
    pub backup_operations: Vec<BackupOperationStatusV1>,
    pub runner_operations: RunnerAttentionOperationsResponseV1,
    pub task_attention: Vec<(String, TaskAgentAttentionResponseV1)>,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AttentionInboxErrorV1 {
    #[error("attention sources are unavailable")]
    Unavailable,
    #[error("attention state is invalid")]
    InvalidData,
}

pub trait AttentionInboxSourceV1: Send + Sync + 'static {
    /// Reads the existing safe lifecycle, setup, Runner, and Activation projections.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable or invalid-data result.
    fn snapshot(&self) -> Result<AttentionInboxSnapshotV1, AttentionInboxErrorV1>;
}

#[derive(Clone)]
pub struct LiveAttentionInboxSourceV1<L, S, R> {
    lifecycle: L,
    setups: TaskSetupSupervisorV1,
    room_creations: RoomCreationSupervisorV1,
    backups: BackupOperationsV1,
    runners: RunnerAttentionSupervisorV1<S, R>,
}

impl<L, S, R> LiveAttentionInboxSourceV1<L, S, R> {
    #[must_use]
    pub const fn new(
        lifecycle: L,
        setups: TaskSetupSupervisorV1,
        room_creations: RoomCreationSupervisorV1,
        backups: BackupOperationsV1,
        runners: RunnerAttentionSupervisorV1<S, R>,
    ) -> Self {
        Self {
            lifecycle,
            setups,
            room_creations,
            backups,
            runners,
        }
    }
}

impl<L, S, R> AttentionInboxSourceV1 for LiveAttentionInboxSourceV1<L, S, R>
where
    L: DaemonLifecycleControl,
    S: RunnerAttentionSourceV1,
    R: ApprovedRunnerRestartV1,
{
    fn snapshot(&self) -> Result<AttentionInboxSnapshotV1, AttentionInboxErrorV1> {
        let observed_at_unix_ms = now_unix_ms()?;
        let lifecycle = self.lifecycle.lifecycle();
        let task_setups = self
            .setups
            .statuses()
            .map_err(|_| AttentionInboxErrorV1::Unavailable)?;
        let room_creations = self
            .room_creations
            .statuses()
            .map_err(|_| AttentionInboxErrorV1::Unavailable)?;
        let backup_operations = self
            .backups
            .statuses()
            .map_err(|_| AttentionInboxErrorV1::Unavailable)?;
        let runner_operations = self.runners.operations().map_err(map_runner_error)?;
        let room_ids = task_setups
            .iter()
            .map(|setup| setup.room_id.clone())
            .collect::<BTreeSet<_>>();
        let mut task_attention = Vec::new();
        for room_id in room_ids {
            match self.runners.task(&room_id) {
                Ok(attention) => task_attention.push((room_id, attention)),
                Err(RunnerAttentionErrorV1::NotFound) => {}
                Err(error) => return Err(map_runner_error(error)),
            }
        }
        Ok(AttentionInboxSnapshotV1 {
            observed_at_unix_ms,
            lifecycle,
            task_setups,
            room_creations,
            backup_operations,
            runner_operations,
            task_attention,
        })
    }
}

#[derive(Clone)]
pub struct AttentionInboxV1<S> {
    source: S,
    history: FileAttentionHistoryV1,
}

impl<S> AttentionInboxV1<S>
where
    S: AttentionInboxSourceV1,
{
    #[must_use]
    pub const fn new(source: S, history: FileAttentionHistoryV1) -> Self {
        Self { source, history }
    }

    /// Refreshes active conditions and reconciles minimal resolved history.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable or invalid-data result.
    pub fn refresh(&self) -> Result<AttentionInboxResponseV1, AttentionInboxErrorV1> {
        let snapshot = self.source.snapshot()?;
        if snapshot.observed_at_unix_ms == 0 {
            return Err(AttentionInboxErrorV1::InvalidData);
        }
        let candidates = derive_items(&snapshot)?;
        self.history
            .reconcile(snapshot.observed_at_unix_ms, candidates)
    }
}

#[derive(Clone)]
pub struct FileAttentionHistoryV1 {
    root: Arc<PathBuf>,
    mutation: Arc<Mutex<()>>,
}

impl FileAttentionHistoryV1 {
    /// Opens owner-only attention transition storage.
    ///
    /// # Errors
    ///
    /// Returns unavailable when protected local storage cannot be prepared.
    pub fn open(root: &Path) -> Result<Self, AttentionInboxErrorV1> {
        Ok(Self {
            root: Arc::new(
                prepare_data_directory(root).map_err(|_| AttentionInboxErrorV1::Unavailable)?,
            ),
            mutation: Arc::new(Mutex::new(())),
        })
    }

    fn reconcile(
        &self,
        observed_at: u64,
        candidates: Vec<AttentionCandidateV1>,
    ) -> Result<AttentionInboxResponseV1, AttentionInboxErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let mut history = self.load()?;
        let active_ids = candidates
            .iter()
            .map(|candidate| candidate.attention_id.clone())
            .collect::<BTreeSet<_>>();
        for record in history.records.values_mut() {
            if record.resolved_at_unix_ms.is_none() && !active_ids.contains(&record.attention_id) {
                record.resolved_at_unix_ms = Some(observed_at);
                record.item = None;
            }
        }
        for candidate in candidates {
            if let Some(record) = history.records.get_mut(&candidate.attention_id) {
                if record.target_kind != candidate.target_kind
                    || record.condition != candidate.condition
                    || record.target_id != candidate.target_id
                {
                    return Err(AttentionInboxErrorV1::InvalidData);
                }
                if record.resolved_at_unix_ms.is_some() {
                    record.first_seen_at_unix_ms = observed_at;
                }
                record.last_seen_at_unix_ms = candidate.observed_at_unix_ms;
                record.resolved_at_unix_ms = None;
                record.item = Some(candidate.into_item(record.first_seen_at_unix_ms));
            } else {
                let item = candidate.clone().into_item(observed_at);
                history.records.insert(
                    candidate.attention_id.clone(),
                    AttentionHistoryRecordV1 {
                        attention_id: candidate.attention_id,
                        target_kind: candidate.target_kind,
                        condition: candidate.condition,
                        target_id: candidate.target_id,
                        first_seen_at_unix_ms: observed_at,
                        last_seen_at_unix_ms: candidate.observed_at_unix_ms,
                        resolved_at_unix_ms: None,
                        item: Some(item),
                    },
                );
            }
        }
        let mut resolved = history
            .records
            .values()
            .filter_map(|record| {
                record
                    .resolved_at_unix_ms
                    .map(|resolved_at_unix_ms| ResolvedAttentionItemV1 {
                        attention_id: record.attention_id.clone(),
                        target_kind: record.target_kind,
                        condition: record.condition,
                        target_id: record.target_id.clone(),
                        resolved_at_unix_ms,
                    })
            })
            .collect::<Vec<_>>();
        resolved.sort_by(|left, right| {
            right
                .resolved_at_unix_ms
                .cmp(&left.resolved_at_unix_ms)
                .then_with(|| left.attention_id.cmp(&right.attention_id))
        });
        resolved.truncate(MAX_RESOLVED_ITEMS);
        let retained_resolved = resolved
            .iter()
            .map(|item| item.attention_id.as_str())
            .collect::<BTreeSet<_>>();
        history.records.retain(|_, record| {
            record.resolved_at_unix_ms.is_none()
                || retained_resolved.contains(record.attention_id.as_str())
        });
        self.persist(&history)?;
        let mut items = history
            .records
            .values()
            .filter_map(|record| record.item.clone())
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            left.target_kind
                .cmp(&right.target_kind)
                .then_with(|| left.target_id.cmp(&right.target_id))
                .then_with(|| left.condition.cmp(&right.condition))
        });
        Ok(AttentionInboxResponseV1 {
            schema: INBOX_SCHEMA_V1.to_owned(),
            observed_at_unix_ms: observed_at,
            items,
            recently_resolved: resolved,
        })
    }

    fn load(&self) -> Result<AttentionHistoryFileV1, AttentionInboxErrorV1> {
        let path = self.root.join("history.json");
        if !path.exists() {
            return Ok(AttentionHistoryFileV1 {
                schema: HISTORY_SCHEMA_V1.to_owned(),
                records: BTreeMap::new(),
            });
        }
        validate_owner_only_file(&path).map_err(|_| AttentionInboxErrorV1::Unavailable)?;
        let metadata = fs::metadata(&path).map_err(|_| AttentionInboxErrorV1::Unavailable)?;
        if metadata.len() == 0 || metadata.len() > MAX_HISTORY_BYTES {
            return Err(AttentionInboxErrorV1::InvalidData);
        }
        let history: AttentionHistoryFileV1 = serde_json::from_slice(
            &fs::read(path).map_err(|_| AttentionInboxErrorV1::Unavailable)?,
        )
        .map_err(|_| AttentionInboxErrorV1::InvalidData)?;
        validate_history(&history)?;
        Ok(history)
    }

    fn persist(&self, history: &AttentionHistoryFileV1) -> Result<(), AttentionInboxErrorV1> {
        validate_history(history)?;
        let bytes = serde_json::to_vec(history).map_err(|_| AttentionInboxErrorV1::InvalidData)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_HISTORY_BYTES {
            return Err(AttentionInboxErrorV1::InvalidData);
        }
        let temporary = self.root.join(".history.tmp");
        let target = self.root.join("history.json");
        let _ = fs::remove_file(&temporary);
        let mut file = create_owner_only_renameable_file(&temporary)
            .map_err(|_| AttentionInboxErrorV1::Unavailable)?;
        let result = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| replace_file(&temporary, &target))
            .and_then(|()| sync_directory(self.root.as_ref()));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
            return Err(AttentionInboxErrorV1::Unavailable);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttentionHistoryFileV1 {
    schema: String,
    records: BTreeMap<String, AttentionHistoryRecordV1>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttentionHistoryRecordV1 {
    attention_id: String,
    target_kind: AttentionTargetKindV1,
    condition: AttentionConditionV1,
    target_id: String,
    first_seen_at_unix_ms: u64,
    last_seen_at_unix_ms: u64,
    resolved_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    item: Option<AttentionItemV1>,
}

#[derive(Clone)]
struct AttentionCandidateV1 {
    attention_id: String,
    target_kind: AttentionTargetKindV1,
    condition: AttentionConditionV1,
    target_id: String,
    title: String,
    reason: String,
    freshness: RunnerAttentionFreshnessV1,
    deep_link: &'static str,
    next_action: String,
    observed_at_unix_ms: u64,
}

impl AttentionCandidateV1 {
    fn into_item(self, first_seen: u64) -> AttentionItemV1 {
        AttentionItemV1 {
            attention_id: self.attention_id,
            target_kind: self.target_kind,
            condition: self.condition,
            target_id: self.target_id,
            title: self.title,
            reason: self.reason,
            freshness: self.freshness,
            deep_link: self.deep_link.to_owned(),
            next_action: self.next_action,
            first_seen_at_unix_ms: first_seen,
            last_seen_at_unix_ms: self.observed_at_unix_ms,
        }
    }
}

fn derive_items(
    snapshot: &AttentionInboxSnapshotV1,
) -> Result<Vec<AttentionCandidateV1>, AttentionInboxErrorV1> {
    let mut items = BTreeMap::new();
    derive_lifecycle(
        &snapshot.lifecycle,
        snapshot.observed_at_unix_ms,
        &mut items,
    )?;
    for setup in &snapshot.task_setups {
        derive_setup(setup, snapshot.observed_at_unix_ms, &mut items)?;
    }
    for creation in &snapshot.room_creations {
        derive_room_creation(creation, snapshot.observed_at_unix_ms, &mut items)?;
    }
    for backup in &snapshot.backup_operations {
        derive_backup_operation(backup, snapshot.observed_at_unix_ms, &mut items)?;
    }
    derive_runner_operations(
        &snapshot.runner_operations,
        snapshot.observed_at_unix_ms,
        &mut items,
    )?;
    for (room_id, task) in &snapshot.task_attention {
        derive_task_attention(room_id, task, snapshot.observed_at_unix_ms, &mut items)?;
    }
    if items.len() > MAX_ACTIVE_ITEMS
        || items.values().any(|candidate| {
            candidate.observed_at_unix_ms == 0
                || candidate.observed_at_unix_ms > snapshot.observed_at_unix_ms
        })
    {
        return Err(AttentionInboxErrorV1::InvalidData);
    }
    Ok(items.into_values().collect())
}

fn derive_lifecycle(
    lifecycle: &DaemonLifecycleV1,
    observed_at_unix_ms: u64,
    items: &mut BTreeMap<String, AttentionCandidateV1>,
) -> Result<(), AttentionInboxErrorV1> {
    if !matches!(
        lifecycle.state,
        DaemonLifecycleStateV1::Failed | DaemonLifecycleStateV1::Unavailable
    ) {
        return Ok(());
    }
    let (reason, next_action) = lifecycle.failure.as_ref().map_or(
        (
            "The configured daemon lifecycle is unavailable.",
            "Retry status, then inspect bounded daemon diagnostics.",
        ),
        |failure| {
            (
                safe_text(
                    &failure.explanation,
                    "The configured daemon needs attention.",
                ),
                safe_text(
                    &failure.next_action,
                    "Retry status, then inspect bounded daemon diagnostics.",
                ),
            )
        },
    );
    insert_candidate(
        items,
        AttentionTargetKindV1::Process,
        AttentionConditionV1::DaemonLifecycle,
        "worldstreamd",
        "worldstreamd needs attention",
        reason,
        if lifecycle.state == DaemonLifecycleStateV1::Unavailable {
            RunnerAttentionFreshnessV1::Unavailable
        } else {
            RunnerAttentionFreshnessV1::Live
        },
        observed_at_unix_ms,
        "#operations",
        next_action,
    )
}

fn derive_setup(
    setup: &TaskSetupStatusV1,
    observed_at_unix_ms: u64,
    items: &mut BTreeMap<String, AttentionCandidateV1>,
) -> Result<(), AttentionInboxErrorV1> {
    if setup.state == TaskSetupStateV1::NeedsAttention {
        let (reason, next_action) = setup.attention.as_ref().map_or(
            (
                "Task setup requires operator attention.",
                "Open Task setup and retry the bounded setup operation.",
            ),
            |attention| {
                (
                    safe_text(
                        &attention.message,
                        "Task setup requires operator attention.",
                    ),
                    if attention.retryable {
                        "Open Task setup and retry the bounded setup operation."
                    } else {
                        "Review the Task setup requirement before continuing."
                    },
                )
            },
        );
        insert_candidate(
            items,
            AttentionTargetKindV1::Task,
            AttentionConditionV1::TaskSetup,
            &setup.draft_id,
            "Task setup needs attention",
            reason,
            RunnerAttentionFreshnessV1::Live,
            observed_at_unix_ms,
            "#task-setup",
            next_action,
        )?;
    }
    if setup.state == TaskSetupStateV1::Ready
        && setup.launch_applicability != TaskLaunchApplicabilityV1::ActiveAtGenesis
        && !setup.readiness.ready_to_launch
    {
        insert_candidate(
            items,
            AttentionTargetKindV1::Task,
            AttentionConditionV1::TaskReadiness,
            &setup.draft_id,
            "Task is not ready to launch",
            "One or more required seats do not currently satisfy readiness.",
            RunnerAttentionFreshnessV1::Live,
            observed_at_unix_ms,
            "#task-setup",
            "Open Task readiness and restore the reported seat requirement.",
        )?;
    }
    if setup
        .launch
        .as_ref()
        .is_some_and(|launch| launch.state == TaskLaunchStateV1::NeedsAttention)
    {
        insert_candidate(
            items,
            AttentionTargetKindV1::Task,
            AttentionConditionV1::TaskLaunch,
            &setup.draft_id,
            "Task launch needs attention",
            "The retained Task launch operation could not finish.",
            RunnerAttentionFreshnessV1::Live,
            observed_at_unix_ms,
            "#task-setup",
            "Open Task launch status and retry only the retained operation.",
        )?;
    }
    Ok(())
}

fn derive_room_creation(
    creation: &RoomCreationStatusV1,
    observed_at_unix_ms: u64,
    items: &mut BTreeMap<String, AttentionCandidateV1>,
) -> Result<(), AttentionInboxErrorV1> {
    if !matches!(
        creation.state,
        RoomCreationStateV1::Retrying | RoomCreationStateV1::NeedsAttention
    ) {
        return Ok(());
    }
    let (reason, next_action) = creation.attention.as_ref().map_or(
        (
            "The retained Room creation operation requires reconciliation.",
            "Open Room creation and retry only the retained operation.",
        ),
        |attention| {
            (
                safe_text(
                    &attention.message,
                    "The retained Room creation operation needs attention.",
                ),
                if attention.retryable {
                    "Open Room creation and retry only the retained operation."
                } else {
                    "Open Room creation and review the rejected operation."
                },
            )
        },
    );
    insert_candidate(
        items,
        AttentionTargetKindV1::Task,
        AttentionConditionV1::RoomCreation,
        &creation.draft_id,
        "Room creation needs attention",
        reason,
        if creation.state == RoomCreationStateV1::Retrying {
            RunnerAttentionFreshnessV1::Unavailable
        } else {
            RunnerAttentionFreshnessV1::Live
        },
        observed_at_unix_ms,
        "#room-creation",
        next_action,
    )
}

fn derive_backup_operation(
    backup: &BackupOperationStatusV1,
    observed_at_unix_ms: u64,
    items: &mut BTreeMap<String, AttentionCandidateV1>,
) -> Result<(), AttentionInboxErrorV1> {
    if !matches!(
        backup.phase,
        BackupOperationPhaseV1::Failed | BackupOperationPhaseV1::Retrying
    ) {
        return Ok(());
    }
    let reason = backup.failure_reason.as_deref().map_or(
        "The retained backup operation requires reconciliation.",
        |reason| safe_text(reason, "The retained backup operation needs attention."),
    );
    let freshness = match backup.freshness {
        BackupFreshnessV1::Fresh { .. } => RunnerAttentionFreshnessV1::Live,
        BackupFreshnessV1::Stale { .. } => RunnerAttentionFreshnessV1::Stale,
        BackupFreshnessV1::Unavailable { .. } => RunnerAttentionFreshnessV1::Unavailable,
    };
    insert_candidate(
        items,
        AttentionTargetKindV1::Process,
        AttentionConditionV1::BackupOperation,
        &backup.operation_id,
        "Backup operation needs attention",
        reason,
        freshness,
        observed_at_unix_ms,
        "#backups",
        if backup.phase == BackupOperationPhaseV1::Retrying {
            "Open Backups and retry only the retained operation."
        } else {
            "Open Backups and review the bounded verification failure."
        },
    )
}

fn derive_runner_operations(
    operations: &RunnerAttentionOperationsResponseV1,
    polled_at_unix_ms: u64,
    items: &mut BTreeMap<String, AttentionCandidateV1>,
) -> Result<(), AttentionInboxErrorV1> {
    for runner in &operations.runners {
        let observed_at_unix_ms = runner
            .observed_at_unix_ms
            .or(operations.observed_at_unix_ms)
            .unwrap_or(polled_at_unix_ms);
        if runner.freshness != RunnerAttentionFreshnessV1::Live {
            insert_candidate(
                items,
                AttentionTargetKindV1::Process,
                AttentionConditionV1::RunnerPresence,
                &runner.runner_id,
                "Runner presence needs attention",
                "The last authenticated Runner presence is not live.",
                runner.freshness,
                observed_at_unix_ms,
                "#runner-attention",
                &runner.next_action,
            )?;
        }
        if runner.incompatible_assignments > 0 {
            insert_candidate(
                items,
                AttentionTargetKindV1::Process,
                AttentionConditionV1::RunnerCompatibility,
                &runner.runner_id,
                "Runner compatibility needs attention",
                "At least one assignment is not compatible with this exact Runner revision.",
                runner.freshness,
                observed_at_unix_ms,
                "#runner-attention",
                &runner.next_action,
            )?;
        }
        if runner.capacity.available == 0 && runner.compatible_assignments > 0 {
            insert_candidate(
                items,
                AttentionTargetKindV1::Process,
                AttentionConditionV1::RunnerCapacity,
                &runner.runner_id,
                "Runner capacity is exhausted",
                "All advertised Activation capacity is currently in use.",
                runner.freshness,
                observed_at_unix_ms,
                "#runner-attention",
                &runner.next_action,
            )?;
        }
    }
    for restart in &operations.restart_attempts {
        if restart.state == RunnerRestartOperationStateV1::Failed {
            insert_candidate(
                items,
                AttentionTargetKindV1::Process,
                AttentionConditionV1::RunnerRestart,
                &restart.instance_id,
                "Runner restart failed",
                &restart.explanation,
                operations.freshness,
                operations.observed_at_unix_ms.unwrap_or(polled_at_unix_ms),
                "#runner-attention",
                &restart.next_action,
            )?;
        }
    }
    derive_managed_hosts(
        &operations.managed_hosts,
        operations.observed_at_unix_ms.unwrap_or(polled_at_unix_ms),
        items,
    )?;
    Ok(())
}

fn derive_managed_hosts(
    hosts: &[ManagedAgentHostStatusV1],
    observed_at_unix_ms: u64,
    items: &mut BTreeMap<String, AttentionCandidateV1>,
) -> Result<(), AttentionInboxErrorV1> {
    for host in hosts {
        if host.state == ManagedAgentHostStateV1::NeedsAttention
            || host.freshness == ManagedAgentHostFreshnessV1::Stale
            || host.active_invocations >= host.capacity
        {
            let (reason, next_action) = host.failure.as_ref().map_or(
                (
                    "The managed Agent Host is stale or has no available capacity.",
                    "Open bounded Runner attention and refresh the managed host.",
                ),
                |failure| {
                    (
                        safe_text(&failure.message, "The managed Agent Host needs attention."),
                        safe_text(
                            &failure.safe_action,
                            "Open bounded Runner attention and refresh the managed host.",
                        ),
                    )
                },
            );
            insert_candidate(
                items,
                AttentionTargetKindV1::Process,
                AttentionConditionV1::ManagedAgentHost,
                &host.assignment_id,
                "Managed Agent Host needs attention",
                reason,
                if host.freshness == ManagedAgentHostFreshnessV1::Stale {
                    RunnerAttentionFreshnessV1::Stale
                } else {
                    RunnerAttentionFreshnessV1::Live
                },
                observed_at_unix_ms,
                "#runner-attention",
                next_action,
            )?;
        }
    }
    Ok(())
}

fn derive_task_attention(
    room_id: &str,
    task: &TaskAgentAttentionResponseV1,
    polled_at_unix_ms: u64,
    items: &mut BTreeMap<String, AttentionCandidateV1>,
) -> Result<(), AttentionInboxErrorV1> {
    let observed_at_unix_ms = task.observed_at_unix_ms.unwrap_or(polled_at_unix_ms);
    for seat in &task.seats {
        let target = format!("{room_id}:{}", seat.seat_id);
        match seat.activation.state {
            ActivationAttentionStateV1::Attention | ActivationAttentionStateV1::Unavailable => {
                insert_candidate(
                    items,
                    AttentionTargetKindV1::Task,
                    AttentionConditionV1::ActivationBacklog,
                    &target,
                    "Agent work needs attention",
                    "Waiting Activation work cannot currently make progress.",
                    seat.freshness,
                    observed_at_unix_ms,
                    "#tasks",
                    &seat.next_action,
                )?;
            }
            ActivationAttentionStateV1::Delayed => {
                insert_candidate(
                    items,
                    AttentionTargetKindV1::Task,
                    AttentionConditionV1::ActivationLease,
                    &target,
                    "Activation lease is delayed",
                    "A retained Activation lease is waiting for reconciliation or expiry.",
                    seat.freshness,
                    observed_at_unix_ms,
                    "#tasks",
                    &seat.next_action,
                )?;
            }
            ActivationAttentionStateV1::Idle
            | ActivationAttentionStateV1::Waiting
            | ActivationAttentionStateV1::Leased => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_candidate(
    items: &mut BTreeMap<String, AttentionCandidateV1>,
    target_kind: AttentionTargetKindV1,
    condition: AttentionConditionV1,
    target_id: &str,
    title: &str,
    reason: &str,
    freshness: RunnerAttentionFreshnessV1,
    observed_at_unix_ms: u64,
    deep_link: &'static str,
    next_action: &str,
) -> Result<(), AttentionInboxErrorV1> {
    if target_id.is_empty() || target_id.len() > 128 || !valid_deep_link(deep_link) {
        return Err(AttentionInboxErrorV1::InvalidData);
    }
    let title = safe_text(title, "Operational condition needs attention.");
    let reason = safe_text(reason, "An operational condition needs attention.");
    let next_action = safe_text(next_action, "Open the bounded detail and retry status.");
    let attention_id = stable_attention_id(target_kind, condition, target_id);
    let candidate = AttentionCandidateV1 {
        attention_id: attention_id.clone(),
        target_kind,
        condition,
        target_id: target_id.to_owned(),
        title: title.to_owned(),
        reason: reason.to_owned(),
        freshness,
        deep_link,
        next_action: next_action.to_owned(),
        observed_at_unix_ms,
    };
    if items.insert(attention_id, candidate).is_some() {
        return Err(AttentionInboxErrorV1::InvalidData);
    }
    Ok(())
}

fn stable_attention_id(
    target_kind: AttentionTargetKindV1,
    condition: AttentionConditionV1,
    target_id: &str,
) -> String {
    let identity = format!("{target_kind:?}:{condition:?}:{target_id}");
    format!("blake3:{}", blake3::hash(identity.as_bytes()).to_hex())
}

fn safe_text<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let lower = value.to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 512
        || value.contains(['\0', '\r', '\n'])
        || [
            "bearer",
            "wsb1:",
            "secret",
            "token",
            "password",
            "prompt",
            "response",
            "memory",
            "payload",
            "invocation",
            "context",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        fallback
    } else {
        value
    }
}

fn valid_deep_link(value: &str) -> bool {
    matches!(
        value,
        "#operations"
            | "#task-setup"
            | "#runner-attention"
            | "#tasks"
            | "#room-creation"
            | "#backups"
    )
}

fn validate_history(history: &AttentionHistoryFileV1) -> Result<(), AttentionInboxErrorV1> {
    if history.schema != HISTORY_SCHEMA_V1
        || history.records.len() > MAX_ACTIVE_ITEMS + MAX_RESOLVED_ITEMS
    {
        return Err(AttentionInboxErrorV1::InvalidData);
    }
    for (key, record) in &history.records {
        if key != &record.attention_id
            || record.attention_id
                != stable_attention_id(record.target_kind, record.condition, &record.target_id)
            || record.first_seen_at_unix_ms == 0
            || record.last_seen_at_unix_ms == 0
            || record
                .resolved_at_unix_ms
                .is_some_and(|resolved| resolved < record.first_seen_at_unix_ms)
        {
            return Err(AttentionInboxErrorV1::InvalidData);
        }
        match (&record.item, record.resolved_at_unix_ms) {
            (Some(item), None)
                if item.attention_id == record.attention_id
                    && item.target_kind == record.target_kind
                    && item.condition == record.condition
                    && item.target_id == record.target_id
                    && item.first_seen_at_unix_ms == record.first_seen_at_unix_ms
                    && item.last_seen_at_unix_ms == record.last_seen_at_unix_ms
                    && valid_deep_link(&item.deep_link) => {}
            (None, Some(_)) => {}
            _ => return Err(AttentionInboxErrorV1::InvalidData),
        }
    }
    Ok(())
}

fn now_unix_ms() -> Result<u64, AttentionInboxErrorV1> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AttentionInboxErrorV1::Unavailable)?
            .as_millis(),
    )
    .map_err(|_| AttentionInboxErrorV1::Unavailable)
}

const fn map_runner_error(error: RunnerAttentionErrorV1) -> AttentionInboxErrorV1 {
    match error {
        RunnerAttentionErrorV1::Unavailable => AttentionInboxErrorV1::Unavailable,
        RunnerAttentionErrorV1::NotFound
        | RunnerAttentionErrorV1::Conflict
        | RunnerAttentionErrorV1::InvalidData => AttentionInboxErrorV1::InvalidData,
    }
}

#[derive(Clone)]
struct AttentionInboxRouterStateV1 {
    inbox: Arc<dyn AttentionInboxApiV1>,
}

trait AttentionInboxApiV1: Send + Sync {
    fn refresh(&self) -> Result<AttentionInboxResponseV1, AttentionInboxErrorV1>;
}

impl<S: AttentionInboxSourceV1> AttentionInboxApiV1 for AttentionInboxV1<S> {
    fn refresh(&self) -> Result<AttentionInboxResponseV1, AttentionInboxErrorV1> {
        AttentionInboxV1::refresh(self)
    }
}

pub fn attention_inbox_router<S: AttentionInboxSourceV1>(inbox: AttentionInboxV1<S>) -> Router {
    Router::new()
        .route("/api/v1/attention-inbox", get(get_attention_inbox))
        .with_state(AttentionInboxRouterStateV1 {
            inbox: Arc::new(inbox),
        })
}

async fn get_attention_inbox(State(state): State<AttentionInboxRouterStateV1>) -> Response {
    let inbox = Arc::clone(&state.inbox);
    match tokio::task::spawn_blocking(move || inbox.refresh()).await {
        Ok(Ok(response)) => (axum::http::StatusCode::OK, Json(response)).into_response(),
        Ok(Err(AttentionInboxErrorV1::InvalidData)) => (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"code":"invalid_attention_state"})),
        )
            .into_response(),
        Ok(Err(AttentionInboxErrorV1::Unavailable)) | Err(_) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"code":"attention_unavailable"})),
        )
            .into_response(),
    }
}

#[cfg(unix)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source
        .to_str()
        .ok_or_else(|| std::io::Error::other("source path is not Unicode"))?;
    let target = target
        .to_str()
        .ok_or_else(|| std::io::Error::other("target path is not Unicode"))?;
    winsafe::MoveFileEx(
        source,
        Some(target),
        winsafe::co::MOVEFILE::REPLACE_EXISTING,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}
