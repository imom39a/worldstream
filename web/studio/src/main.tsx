import { StrictMode, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import {
  loadBackupOperationId,
  loadBackupOperationState,
  loadBackupProfile,
  newBackupOperationId,
  runBackupOperation,
  saveBackupOperationId,
  type BackupOperationStatus,
  type BackupProfileStatus,
} from "./backups";
import {
  loadActivityPackCatalog,
  loadActivityPackDetail,
  loadActivityPackSelection,
  saveActivityPackSelection,
  type ActivityPackCatalog,
  type ActivityPackDetailResponse,
  type ActivityPackReference,
} from "./activityPacks";
import {
  loadDaemonLifecycle,
  requestDaemonLifecycle,
  type DaemonLifecycle,
  type DaemonLifecycleAction,
} from "./daemonLifecycle";
import { loadDaemonStatus, type DaemonStatus } from "./daemonStatus";
import {
  loadRunnerInstances,
  loadRunnerTemplates,
  requestRunnerInstanceLifecycle,
  type RunnerInstanceLifecycleAction,
  type RunnerInstanceStatusResponse,
  type RunnerTemplateCatalog,
} from "./runnerTemplates";
import { loadSecretStatus, type SecretStatusResponse } from "./secretStatus";
import {
  loadRoomDetail,
  loadRoomInventory,
  staleAfterFailedRefresh,
  type RoomInventoryState,
} from "./roomInventory";
import { resumeRoomDraftStep } from "./RoomDraftWizard";
import {
  createRoomDraft,
  invalidateRoomDraftReview,
  loadRoomDraft,
  saveRoomDraft,
  type RoomDraft,
  type RoomDraftFieldError,
  type RoomDraftStep,
} from "./roomDrafts";
import {
  loadRoomCreation,
  requestRoomCreation,
  type RoomCreationStatus,
} from "./roomCreation";
import {
  loadTaskSetup,
  requestTaskSetup,
  type TaskSetupStatus,
} from "./taskSetup";
import "./styles.css";

const REFRESH_INTERVAL_MS = 5_000;
const NEW_ROOM_DRAFT_ID = "new-room";

function LiveStudio() {
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [lifecycle, setLifecycle] = useState<DaemonLifecycle | null>(null);
  const [secretStatus, setSecretStatus] = useState<SecretStatusResponse | null>(null);
  const [runnerTemplates, setRunnerTemplates] = useState<RunnerTemplateCatalog | null>(null);
  const [runnerInstances, setRunnerInstances] = useState<RunnerInstanceStatusResponse | null>(null);
  const [activityPackCatalog, setActivityPackCatalog] = useState<ActivityPackCatalog | null>(null);
  const [activityPackCatalogLoading, setActivityPackCatalogLoading] = useState(true);
  const [activityPackDetail, setActivityPackDetail] = useState<ActivityPackDetailResponse | null>(null);
  const [inspectedActivityPackDigest, setInspectedActivityPackDigest] = useState<string | null>(null);
  const [activityPackSelection, setActivityPackSelection] = useState<ActivityPackReference | null>(
    () => loadActivityPackSelection(),
  );
  const activityPackDetailRequest = useRef(0);
  const [roomInventory, setRoomInventory] = useState<RoomInventoryState>({ status: "loading" });
  const [selectedRoomId, setSelectedRoomId] = useState<string | null>(null);
  const roomDetailRequest = useRef(0);
  const [roomDraft, setRoomDraft] = useState<RoomDraft>(() => createRoomDraft(NEW_ROOM_DRAFT_ID));
  const [roomDraftStep, setRoomDraftStep] = useState<RoomDraftStep>("activity");
  const [roomDraftErrors, setRoomDraftErrors] = useState<RoomDraftFieldError[]>([]);
  const [roomDraftSaving, setRoomDraftSaving] = useState(false);
  const [roomDraftSaved, setRoomDraftSaved] = useState(false);
  const [roomCreation, setRoomCreation] = useState<RoomCreationStatus | null>(null);
  const [roomCreationStatusAvailable, setRoomCreationStatusAvailable] = useState(false);
  const [roomCreationLoading, setRoomCreationLoading] = useState(false);
  const [taskSetup, setTaskSetup] = useState<TaskSetupStatus | null>(null);
  const [taskSetupStatusAvailable, setTaskSetupStatusAvailable] = useState(false);
  const [taskSetupLoading, setTaskSetupLoading] = useState(false);
  const [backupProfile, setBackupProfile] = useState<BackupProfileStatus | null>(null);
  const [backupOperation, setBackupOperation] = useState<BackupOperationStatus | null>(null);
  const [backupOperationId, setBackupOperationId] = useState<string | null>(
    () => loadBackupOperationId(),
  );
  const [backupOperationStatusAvailable, setBackupOperationStatusAvailable] = useState(
    () => loadBackupOperationId() === null,
  );
  const [backupLoading, setBackupLoading] = useState(false);
  const backupMutationInFlight = useRef(false);

  useEffect(() => {
    let active = true;
    const refresh = async () => {
      const activeBackupOperationId = loadBackupOperationId();
      const [
        nextStatus,
        nextLifecycle,
        nextSecretStatus,
        nextRunnerTemplates,
        nextRunnerInstances,
        nextActivityPackCatalog,
        nextRoomInventory,
        nextRoomCreation,
        nextTaskSetup,
        nextBackupProfile,
        nextBackupOperation,
      ] = await Promise.all([
        loadDaemonStatus(),
        loadDaemonLifecycle(),
        loadSecretStatus(),
        loadRunnerTemplates(),
        loadRunnerInstances(),
        loadActivityPackCatalog(),
        loadRoomInventory(),
        loadRoomCreation(NEW_ROOM_DRAFT_ID),
        loadTaskSetup(NEW_ROOM_DRAFT_ID),
        loadBackupProfile(),
        activeBackupOperationId === null
          ? Promise.resolve({ availability: "available" as const, operation: null })
          : loadBackupOperationState(activeBackupOperationId),
      ]);
      if (active) {
        setStatus(nextStatus);
        setLifecycle(nextLifecycle);
        setSecretStatus(nextSecretStatus);
        setRunnerTemplates(nextRunnerTemplates);
        setRunnerInstances(nextRunnerInstances);
        setActivityPackCatalog(nextActivityPackCatalog);
        setActivityPackCatalogLoading(false);
        setRoomInventory((previous) => overlayTaskSetup(
          staleAfterFailedRefresh(previous, nextRoomInventory),
          nextTaskSetup.availability === "available" ? nextTaskSetup.setup : null,
          nextTaskSetup.availability === "available",
        ));
        setRoomCreation(nextRoomCreation.operation);
        setRoomCreationStatusAvailable(nextRoomCreation.availability === "available");
        setTaskSetup(nextTaskSetup.availability === "available" ? nextTaskSetup.setup : null);
        setTaskSetupStatusAvailable(nextTaskSetup.availability === "available");
        setBackupProfile(nextBackupProfile);
        if (loadBackupOperationId() === activeBackupOperationId) {
          setBackupOperation(nextBackupOperation.operation);
          setBackupOperationId(activeBackupOperationId);
          setBackupOperationStatusAvailable(nextBackupOperation.availability === "available");
        }
      }
    };
    void refresh();
    const interval = window.setInterval(() => void refresh(), REFRESH_INTERVAL_MS);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, []);

  useEffect(() => {
    let active = true;
    void loadRoomDraft(NEW_ROOM_DRAFT_ID).then(async (response) => {
      if (!active || response === null) return;
      setRoomDraft(response.draft);
      setRoomDraftSaved(true);
      setRoomDraftStep(resumeRoomDraftStep(response.draft));
      if (response.draft.pack !== null) {
        const detail = await loadActivityPackDetail(response.draft.pack.digest);
        if (active) {
          setInspectedActivityPackDigest(response.draft.pack.digest);
          setActivityPackDetail(detail);
        }
      }
    });
    return () => { active = false; };
  }, []);

  const requestLifecycle = async (action: DaemonLifecycleAction) => {
    const next = await requestDaemonLifecycle(action);
    setLifecycle(next);
    setStatus(await loadDaemonStatus());
  };

  const requestRunnerLifecycle = async (
    instanceId: string,
    action: RunnerInstanceLifecycleAction,
  ) => {
    const next = await requestRunnerInstanceLifecycle(instanceId, action);
    setRunnerInstances(next);
  };

  const inspectActivityPack = async (digest: string) => {
    const request = activityPackDetailRequest.current + 1;
    activityPackDetailRequest.current = request;
    setInspectedActivityPackDigest(digest);
    setActivityPackDetail(null);
    const detail = await loadActivityPackDetail(digest);
    if (activityPackDetailRequest.current === request) setActivityPackDetail(detail);
  };

  const selectActivityPack = (selection: ActivityPackReference) => {
    saveActivityPackSelection(selection);
    setActivityPackSelection(selection);
  };

  const clearActivityPackSelection = () => {
    saveActivityPackSelection(null);
    setActivityPackSelection(null);
  };

  const inspectRoom = async (roomId: string) => {
    const request = roomDetailRequest.current + 1;
    roomDetailRequest.current = request;
    setSelectedRoomId(roomId);
    const detail = await loadRoomDetail(roomId);
    if (detail === null || roomDetailRequest.current !== request) return;
    setRoomInventory((current) => current.status === "available" ? {
      status: "available",
      page: {
        ...current.page,
        rooms: current.page.rooms.map((room) => room.room_id === roomId ? detail : room),
      },
    } : current);
  };

  const changeRoomDraft = (next: RoomDraft) => {
    const packChanged = next.pack?.digest !== roomDraft.pack?.digest;
    setRoomDraft(invalidateRoomDraftReview(roomDraft, next));
    setRoomDraftSaved(false);
    setRoomDraftErrors([]);
    if (packChanged && next.pack !== null) void inspectActivityPack(next.pack.digest);
  };

  const persistRoomDraft = async (next: RoomDraft) => {
    setRoomDraftSaving(true);
    setRoomDraftSaved(false);
    setRoomDraftErrors([]);
    const result = await saveRoomDraft(next);
    setRoomDraftSaving(false);
    if (result?.version === "studio_room_draft.v1") {
      setRoomDraft(result.draft);
      setRoomDraftSaved(true);
    } else if (result?.version === "studio_room_draft_error.v1") {
      setRoomDraftErrors(result.field_errors);
    }
  };

  const startBackup = async () => {
    if (backupMutationInFlight.current) return;
    backupMutationInFlight.current = true;
    const operationId = newBackupOperationId();
    saveBackupOperationId(operationId);
    setBackupOperationId(operationId);
    setBackupOperation(null);
    setBackupOperationStatusAvailable(false);
    setBackupLoading(true);
    try {
      const result = await runBackupOperation(operationId);
      if (result !== null) {
        setBackupOperation(result);
        setBackupOperationStatusAvailable(true);
      }
    } finally {
      backupMutationInFlight.current = false;
      setBackupLoading(false);
    }
  };

  const retryBackup = async () => {
    if (backupMutationInFlight.current) return;
    const operationId = backupOperation?.operation_id ?? backupOperationId;
    if (operationId === null || (backupOperation !== null && backupOperation.phase !== "retrying")) return;
    backupMutationInFlight.current = true;
    setBackupLoading(true);
    try {
      const result = await runBackupOperation(operationId);
      if (result !== null) {
        setBackupOperation(result);
        setBackupOperationStatusAvailable(true);
      } else {
        setBackupOperation(null);
        setBackupOperationStatusAvailable(false);
      }
    } finally {
      backupMutationInFlight.current = false;
      setBackupLoading(false);
    }
  };

  const runRoomCreation = async (action: "start" | "retry") => {
    setRoomCreationLoading(true);
    const result = await requestRoomCreation(NEW_ROOM_DRAFT_ID, action);
    setRoomCreation(result);
    setRoomCreationStatusAvailable(result !== null);
    setRoomCreationLoading(false);
  };

  const runTaskSetup = async (action: "start" | "retry") => {
    setTaskSetupLoading(true);
    const result = await requestTaskSetup(NEW_ROOM_DRAFT_ID, action);
    setTaskSetup(result);
    setTaskSetupStatusAvailable(result !== null);
    setTaskSetupLoading(false);
  };

  return (
    <App
      status={status}
      lifecycle={lifecycle}
      secretStatus={secretStatus}
      runnerTemplates={runnerTemplates}
      runnerInstances={runnerInstances}
      activityPackCatalog={activityPackCatalog}
      activityPackCatalogLoading={activityPackCatalogLoading}
      activityPackDetail={activityPackDetail}
      inspectedActivityPackDigest={inspectedActivityPackDigest}
      activityPackSelection={activityPackSelection}
      roomInventory={roomInventory}
      selectedRoomId={selectedRoomId}
      onLifecycleAction={(action) => void requestLifecycle(action)}
      onRunnerLifecycleAction={(instanceId, action) => {
        void requestRunnerLifecycle(instanceId, action);
      }}
      onInspectActivityPack={(digest) => void inspectActivityPack(digest)}
      onSelectActivityPack={selectActivityPack}
      onClearActivityPackSelection={clearActivityPackSelection}
      onSelectRoom={(roomId) => void inspectRoom(roomId)}
      roomDraft={roomDraft}
      roomDraftStep={roomDraftStep}
      roomDraftErrors={roomDraftErrors}
      roomDraftSaving={roomDraftSaving}
      roomDraftSaved={roomDraftSaved}
      onRoomDraftChange={changeRoomDraft}
      onRoomDraftStepChange={setRoomDraftStep}
      onSaveRoomDraft={(draft) => void persistRoomDraft(draft)}
      roomCreation={roomCreation}
      roomCreationStatusAvailable={roomCreationStatusAvailable}
      roomCreationLoading={roomCreationLoading}
      onStartRoomCreation={() => void runRoomCreation("start")}
      onRetryRoomCreation={() => void runRoomCreation("retry")}
      taskSetup={taskSetup}
      taskSetupStatusAvailable={taskSetupStatusAvailable}
      taskSetupLoading={taskSetupLoading}
      onStartTaskSetup={() => void runTaskSetup("start")}
      onRetryTaskSetup={() => void runTaskSetup("retry")}
      backupProfile={backupProfile}
      backupOperation={backupOperation}
      backupOperationId={backupOperationId}
      backupOperationStatusAvailable={backupOperationStatusAvailable}
      backupLoading={backupLoading}
      onStartBackup={() => void startBackup()}
      onRetryBackup={() => void retryBackup()}
    />
  );
}

function overlayTaskSetup(
  inventory: RoomInventoryState,
  setup: TaskSetupStatus | null,
  setupAvailable: boolean,
): RoomInventoryState {
  if (inventory.status !== "available") return inventory;
  return {
    status: "available",
    page: {
      ...inventory.page,
      rooms: inventory.page.rooms.map((room) => !setupAvailable ? {
        ...room,
        setup_progress: { status: "unavailable" as const, reason: "task_setup_refresh_unavailable" },
      } : setup === null || room.room_id !== setup.room_id ? room : {
        ...room,
        setup_progress: setup.state === "ready"
          ? { status: "complete" as const, completed_steps: setup.completed_stages, total_steps: setup.total_stages }
          : {
              status: "partially_provisioned" as const,
              completed_steps: setup.completed_stages,
              total_steps: setup.total_stages,
              reason: setup.attention?.code ?? "setup_in_progress",
            },
      }),
    },
  };
}

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root mount point");
createRoot(root).render(<StrictMode><LiveStudio /></StrictMode>);
