import type {
  DaemonLifecycle,
  DaemonLifecycleAction,
} from "./daemonLifecycle";
import type { DaemonStatus } from "./daemonStatus";
import { ActivityPackCatalogView } from "./ActivityPackCatalog";
import type {
  ActivityPackCatalog,
  ActivityPackDetailResponse,
  ActivityPackReference,
} from "./activityPacks";
import { RunnerOperations } from "./RunnerOperations";
import { RunnerAttentionPanel } from "./RunnerAttentionPanel";
import { AttentionInboxPanel } from "./AttentionInboxPanel";
import type { AttentionInboxResponse } from "./attentionInbox";
import type {
  RunnerAttentionOperations,
  TaskAgentAttention,
} from "./runnerAttention";
import { BackupOperations } from "./BackupOperations";
import type { BackupOperationStatus, BackupProfileStatus } from "./backups";
import { RoomOperations } from "./RoomOperations";
import { RoomDraftWizard } from "./RoomDraftWizard";
import { RoomCreationOperation } from "./RoomCreationOperation";
import type { RoomCreationStatus } from "./roomCreation";
import { TaskSetupOperation } from "./TaskSetupOperation";
import type { TaskSetupStatus } from "./taskSetup";
import { AgentProfileBuild } from "./AgentProfileBuild";
import type { AgentProfileCatalog, AgentProfileRevision, ModelProviderCredentialCatalog } from "./agentProfiles";
import { TaskTemplateBuild } from "./TaskTemplateBuild";
import type { TaskTemplateCatalog } from "./taskTemplates";
import type {
  RunnerInstanceLifecycleAction,
  RunnerInstanceStatusResponse,
  RunnerTemplateCatalog,
} from "./runnerTemplates";
import type { SecretStatusResponse } from "./secretStatus";
import type { RoomInventoryState } from "./roomInventory";
import type { RoomOperatorView } from "./roomOperatorView";
import type {
  RoomDraft,
  RoomDraftFieldError,
  RoomDraftStep,
} from "./roomDrafts";

export interface AppProps {
  status: DaemonStatus | null;
  lifecycle: DaemonLifecycle | null;
  onLifecycleAction?: (action: DaemonLifecycleAction) => void;
  secretStatus?: SecretStatusResponse | null;
  runnerTemplates?: RunnerTemplateCatalog | null;
  runnerInstances?: RunnerInstanceStatusResponse | null;
  onRunnerLifecycleAction?: (
    instanceId: string,
    action: RunnerInstanceLifecycleAction,
  ) => void;
  runnerAttention?: RunnerAttentionOperations | null;
  taskAgentAttention?: TaskAgentAttention | null;
  onRestartApprovedRunner?: (instanceId: string) => void;
  managedHostRoomId?: string | null;
  managedHostBusySeats?: ReadonlySet<string>;
  onManagedHostAction?: (seatId: string, action: "start" | "retry") => void;
  attentionInbox?: AttentionInboxResponse | null;
  attentionNotificationsEnabled?: boolean;
  attentionNotificationsAvailable?: boolean;
  onAttentionNotificationPreference?: (enabled: boolean) => void;
  activityPackCatalog?: ActivityPackCatalog | null;
  activityPackDetail?: ActivityPackDetailResponse | null;
  inspectedActivityPackDigest?: string | null;
  activityPackSelection?: ActivityPackReference | null;
  activityPackCatalogLoading?: boolean;
  onInspectActivityPack?: (digest: string) => void;
  onSelectActivityPack?: (selection: ActivityPackReference) => void;
  onClearActivityPackSelection?: () => void;
  roomInventory?: RoomInventoryState;
  selectedRoomId?: string | null;
  onSelectRoom?: (roomId: string) => void;
  operatorView?: RoomOperatorView | null;
  operatorViewLoading?: boolean;
  onEnableOperatorView?: (roomId: string) => void;
  roomDraft?: RoomDraft | null;
  roomDraftStep?: RoomDraftStep;
  roomDraftErrors?: RoomDraftFieldError[];
  roomDraftSaving?: boolean;
  roomDraftSaved?: boolean;
  onRoomDraftChange?: (draft: RoomDraft) => void;
  onRoomDraftStepChange?: (step: RoomDraftStep) => void;
  onSaveRoomDraft?: (draft: RoomDraft) => void;
  roomCreation?: RoomCreationStatus | null;
  roomCreationStatusAvailable?: boolean;
  roomCreationLoading?: boolean;
  onStartRoomCreation?: () => void;
  onRetryRoomCreation?: () => void;
  taskSetup?: TaskSetupStatus | null;
  taskSetupStatusAvailable?: boolean;
  taskSetupLoading?: boolean;
  onStartTaskSetup?: () => void;
  onRetryTaskSetup?: () => void;
  onLaunchTask?: () => void;
  agentProfiles?: AgentProfileCatalog | null;
  modelProviderCredentials?: ModelProviderCredentialCatalog | null;
  onAgentProfilePublished?: (profile: AgentProfileRevision) => void;
  taskTemplates?: TaskTemplateCatalog | null;
  onTaskTemplateCatalogChanged?: () => void;
  onTaskTemplateDraftCreated?: (draft: RoomDraft) => void;
  onOpenParticipantView?: (seatId: string) => void;
  backupProfile?: BackupProfileStatus | null;
  backupOperation?: BackupOperationStatus | null;
  backupOperationId?: string | null;
  backupOperationStatusAvailable?: boolean;
  backupLoading?: boolean;
  onStartBackup?: () => void;
  onRetryBackup?: () => void;
}

export function App({
  status,
  lifecycle,
  onLifecycleAction,
  secretStatus,
  runnerTemplates = null,
  runnerInstances = null,
  onRunnerLifecycleAction,
  runnerAttention = null,
  taskAgentAttention = null,
  onRestartApprovedRunner,
  managedHostRoomId = null,
  managedHostBusySeats = new Set(),
  onManagedHostAction,
  attentionInbox = null,
  attentionNotificationsEnabled = false,
  attentionNotificationsAvailable = false,
  onAttentionNotificationPreference,
  activityPackCatalog = null,
  activityPackDetail = null,
  inspectedActivityPackDigest = null,
  activityPackSelection = null,
  activityPackCatalogLoading = false,
  onInspectActivityPack,
  onSelectActivityPack,
  onClearActivityPackSelection,
  roomInventory = { status: "loading" },
  selectedRoomId = null,
  onSelectRoom,
  operatorView = null,
  operatorViewLoading = false,
  onEnableOperatorView,
  roomDraft = null,
  roomDraftStep = "activity",
  roomDraftErrors = [],
  roomDraftSaving = false,
  roomDraftSaved = false,
  onRoomDraftChange,
  onRoomDraftStepChange,
  onSaveRoomDraft,
  roomCreation = null,
  roomCreationStatusAvailable = false,
  roomCreationLoading = false,
  onStartRoomCreation,
  onRetryRoomCreation,
  taskSetup = null,
  taskSetupStatusAvailable = false,
  taskSetupLoading = false,
  onStartTaskSetup,
  onRetryTaskSetup,
  onLaunchTask,
  agentProfiles = null,
  modelProviderCredentials = null,
  onAgentProfilePublished,
  taskTemplates = null,
  onTaskTemplateCatalogChanged,
  onTaskTemplateDraftCreated,
  onOpenParticipantView,
  backupProfile = null,
  backupOperation = null,
  backupOperationId = null,
  backupOperationStatusAvailable = true,
  backupLoading = false,
  onStartBackup,
  onRetryBackup,
}: AppProps) {
  const connected = status?.connectivity === "connected";
  const loading = status === null;

  return (
    <main className="studio-shell">
      <aside className="studio-sidebar">
        <div className="brand-mark" aria-hidden="true"><span /></div>
        <div>
          <strong>WorldStream Studio</strong>
          <span>Host operator portal</span>
        </div>
        <nav aria-label="Studio navigation">
          <a aria-current="page" href="#home">Home</a>
          <a href="#tasks">Tasks</a>
          <a href="#room-draft">Build</a>
          <a href="#runner-processes">Operations</a>
        </nav>
        <p>Companion control plane</p>
      </aside>

      <section className="studio-content" id="home">
        <header>
          <div>
            <p className="eyebrow">Local installation</p>
            <h1>Operator overview</h1>
            <p>Observe the authoritative Room runtime through the local Supervisor.</p>
          </div>
          <StatusBadge connected={connected} loading={loading} />
        </header>

        <AttentionInboxPanel
          inbox={attentionInbox}
          notificationsEnabled={attentionNotificationsEnabled}
          notificationsAvailable={attentionNotificationsAvailable}
          onNotificationPreference={onAttentionNotificationPreference}
        />

        <section
          className={`daemon-card ${connected ? "is-connected" : "is-unavailable"}`}
          id="operations"
        >
          <div className="daemon-heading">
            <div>
              <p className="eyebrow">Authoritative runtime</p>
              <h2>{daemonHeading(connected, loading, lifecycle)}</h2>
            </div>
            <span className="status-orb" aria-hidden="true" />
          </div>

          {connected && status ? (
            <dl className="status-grid">
              <StatusFact label="Health" value={status.health === "live" ? "Process live" : "Unavailable"} />
              <StatusFact label="Readiness" value={readinessLabel(status.readiness)} />
              <StatusFact label="Version" value={`Build ${status.version?.build_version ?? "Unavailable"}`} />
              <StatusFact label="Source" value={shortRevision(status.version?.source_revision)} />
            </dl>
          ) : loading ? (
            <p className="status-message">Asking the Supervisor for live daemon health and version information.</p>
          ) : (
            <div className="unavailable-message" role="status">
              <strong>Start or reconnect the local daemon</strong>
              <p>The Supervisor cannot establish live health. No cached version is shown.</p>
            </div>
          )}

          <LifecycleControls
            lifecycle={lifecycle}
            connected={connected}
            onAction={onLifecycleAction}
          />
        </section>

        <section className="credential-card" id="settings">
          <p className="eyebrow">Settings · Credentials</p>
          <h2>Protected local references</h2>
          <p>Studio receives configuration state only. Credential values stay inside the Supervisor.</p>
          <dl className="credential-grid">
            {(secretStatus?.credentials ?? []).map((credential) => (
              <StatusFact
                key={credential.kind}
                label={credentialLabel(credential.kind)}
                value={availabilityLabel(credential)}
              />
            ))}
          </dl>
          {secretStatus === null || secretStatus === undefined ? (
            <p className="status-message">Checking protected credential configuration…</p>
          ) : null}
        </section>

        <div id="tasks">
          <RoomOperations
            inventory={roomInventory}
            selectedRoomId={selectedRoomId}
            onSelectRoom={onSelectRoom}
            operatorView={operatorView}
            operatorViewLoading={operatorViewLoading}
            onEnableOperatorView={onEnableOperatorView}
          />
        </div>

        <div className="agent-profile-build" id="agent-profiles">
          <AgentProfileBuild
            catalog={agentProfiles}
            onPublished={onAgentProfilePublished}
            credentials={modelProviderCredentials}
            runnerTemplates={runnerTemplates}
          />
        </div>

        {roomDraft !== null ? (
          <RoomDraftWizard
            draft={roomDraft}
            catalog={activityPackCatalog}
            detail={activityPackDetail}
            agentProfiles={agentProfiles}
            runnerTemplates={runnerTemplates}
            activeStep={roomDraftStep}
            fieldErrors={roomDraftErrors}
            saving={roomDraftSaving}
            saved={roomDraftSaved}
            onDraftChange={onRoomDraftChange}
            onStepChange={onRoomDraftStepChange}
            onInspectPack={onInspectActivityPack}
            onSave={onSaveRoomDraft}
          />
        ) : null}

        <div className="task-template-build" id="task-templates">
          <TaskTemplateBuild
            catalog={taskTemplates}
            reviewedDraft={roomDraft}
            onCatalogChanged={onTaskTemplateCatalogChanged}
            onDraftCreated={onTaskTemplateDraftCreated}
          />
        </div>

        <RoomCreationOperation
          status={roomCreation}
          statusAvailable={roomCreationStatusAvailable}
          reviewed={roomDraftSaved && roomDraft?.last_valid_step === "review"}
          loading={roomCreationLoading}
          onStart={onStartRoomCreation}
          onRetry={onRetryRoomCreation}
        />

        <TaskSetupOperation
          setup={taskSetup}
          statusAvailable={taskSetupStatusAvailable}
          roomCreated={roomCreation?.state === "succeeded"}
          loading={taskSetupLoading}
          onStart={onStartTaskSetup}
          onRetry={onRetryTaskSetup}
          onLaunch={onLaunchTask}
          onOpenParticipantView={onOpenParticipantView}
        />

        <ActivityPackCatalogView
          catalog={activityPackCatalog}
          detail={activityPackDetail}
          inspectedDigest={inspectedActivityPackDigest}
          selection={activityPackSelection}
          loading={activityPackCatalogLoading}
          onInspect={onInspectActivityPack}
          onSelect={onSelectActivityPack}
          onClearSelection={onClearActivityPackSelection}
        />

        <BackupOperations
          profile={backupProfile}
          operation={backupOperation}
          operationId={backupOperationId}
          operationStatusAvailable={backupOperationStatusAvailable}
          loading={backupLoading}
          onStart={onStartBackup}
          onRetry={onRetryBackup}
        />

        <RunnerOperations
          catalog={runnerTemplates}
          instances={runnerInstances}
          onLifecycleAction={onRunnerLifecycleAction}
        />

        <RunnerAttentionPanel
          operations={runnerAttention}
          task={taskAgentAttention}
          onRestart={onRestartApprovedRunner}
          managedHostRoomId={managedHostRoomId}
          managedReferenceSeatIds={managedReferenceSeatIds(taskSetup, agentProfiles, managedHostRoomId)}
          managedHostBusySeats={managedHostBusySeats}
          onManagedHostAction={onManagedHostAction}
        />

        <section className="authority-note">
          <p className="eyebrow">Authority boundary</p>
          <h2>Rooms stay with worldstreamd</h2>
          <p>
            Studio is an operator-facing companion. It observes and requests work only through supported daemon APIs; it does not own or directly mutate Authoritative Room State.
          </p>
        </section>
      </section>
    </main>
  );
}

function managedReferenceSeatIds(
  setup: TaskSetupStatus | null,
  profiles: AgentProfileCatalog | null,
  selectedRoomId: string | null | undefined,
): ReadonlySet<string> {
  if (setup === null || profiles === null || selectedRoomId !== setup.room_id) return new Set();
  return new Set(setup.seats.flatMap((seat) => {
    if (seat.agent_assignment !== "managed" || seat.agent_profile === null) return [];
    const profile = profiles.profiles.find((candidate) =>
      candidate.profile_id === seat.agent_profile?.profile_id
      && candidate.revision === seat.agent_profile.revision);
    return profile?.host_contract.kind === "managed_reference" ? [seat.seat_id] : [];
  }));
}

function credentialLabel(kind: SecretStatusResponse["credentials"][number]["kind"]) {
  if (kind === "host_authority") return "Host authority";
  if (kind === "membership_authority") return "Membership authority";
  if (kind === "runner_authority") return "Runner authority";
  return "Model provider";
}

function availabilityLabel(
  { kind, availability }: SecretStatusResponse["credentials"][number],
) {
  if (availability === "configured") return "Configured";
  if (availability === "missing") {
    if (kind === "host_authority") return "Required";
    if (kind === "model_provider") return "Optional";
    return "Created during Task setup";
  }
  return "Unavailable";
}

function daemonHeading(
  connected: boolean,
  loading: boolean,
  lifecycle: DaemonLifecycle | null,
) {
  if (lifecycle?.state === "starting") return "Starting worldstreamd…";
  if (lifecycle?.state === "stopping") return "Stopping worldstreamd…";
  if (lifecycle?.state === "stopped") return "worldstreamd stopped";
  if (loading) return "Checking worldstreamd…";
  return connected ? "worldstreamd connected" : "worldstreamd unavailable";
}

function LifecycleControls({
  lifecycle,
  connected,
  onAction,
}: {
  lifecycle: DaemonLifecycle | null;
  connected: boolean;
  onAction?: (action: DaemonLifecycleAction) => void;
}) {
  if (lifecycle === null) {
    return <p className="lifecycle-message">Checking configured lifecycle control…</p>;
  }
  if (lifecycle.state === "starting" || lifecycle.state === "stopping") {
    return (
      <div className="lifecycle-progress" role="status">
        <strong>Lifecycle operation in progress</strong>
        <p>Operation {lifecycle.operation_id} is being reconciled by the Supervisor.</p>
      </div>
    );
  }

  const failure = lifecycle.failure;
  return (
    <div className="lifecycle-controls">
      {failure ? (
        <div className="lifecycle-failure" role="alert">
          <strong>Lifecycle control needs attention</strong>
          <p>{failure.explanation}</p>
          <p>{failure.next_action}</p>
        </div>
      ) : null}
      {lifecycle.state === "running" && !lifecycle.managed_by_supervisor ? (
        <div className="lifecycle-guidance" role="status">
          <strong>Daemon reconciled after Supervisor restart</strong>
          <p>It remains running, but this Supervisor session will not signal a process it does not own.</p>
          <p>Stop it from its original process owner before starting it here.</p>
        </div>
      ) : null}
      <div className="lifecycle-actions">
        {lifecycle.state === "stopped" ||
        (lifecycle.state === "failed" && !lifecycle.managed_by_supervisor && !connected) ? (
          <LifecycleButton
            label={lifecycle.state === "failed" ? "Retry start" : "Start daemon"}
            action="start"
            onAction={onAction}
          />
        ) : null}
        {lifecycle.state === "running" && lifecycle.managed_by_supervisor ? (
          <>
            <LifecycleButton label="Stop daemon" action="stop" onAction={onAction} />
            <LifecycleButton label="Restart daemon" action="restart" onAction={onAction} />
          </>
        ) : null}
        {lifecycle.state === "failed" && lifecycle.managed_by_supervisor ? (
          <LifecycleButton label="Retry stop" action="stop" onAction={onAction} />
        ) : null}
      </div>
    </div>
  );
}

function LifecycleButton({
  label,
  action,
  onAction,
}: {
  label: string;
  action: DaemonLifecycleAction;
  onAction?: (action: DaemonLifecycleAction) => void;
}) {
  return (
    <button type="button" onClick={() => onAction?.(action)}>
      {label}
    </button>
  );
}

function StatusBadge({ connected, loading }: { connected: boolean; loading: boolean }) {
  return <span className={`top-status ${connected ? "good" : "muted"}`}>{loading ? "Checking" : connected ? "Daemon online" : "Daemon offline"}</span>;
}

function StatusFact({ label, value }: { label: string; value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>;
}

function readinessLabel(readiness: DaemonStatus["readiness"]) {
  if (readiness === "ready") return "Runtime ready";
  if (readiness === "not_ready") return "Runtime not ready";
  return "Unavailable";
}

function shortRevision(revision: string | undefined) {
  return revision ? revision.slice(0, 12) : "Unavailable";
}
