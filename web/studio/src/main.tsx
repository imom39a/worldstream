import { StrictMode, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
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
import "./styles.css";

const REFRESH_INTERVAL_MS = 5_000;

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

  useEffect(() => {
    let active = true;
    const refresh = async () => {
      const [
        nextStatus,
        nextLifecycle,
        nextSecretStatus,
        nextRunnerTemplates,
        nextRunnerInstances,
        nextActivityPackCatalog,
        nextRoomInventory,
      ] = await Promise.all([
        loadDaemonStatus(),
        loadDaemonLifecycle(),
        loadSecretStatus(),
        loadRunnerTemplates(),
        loadRunnerInstances(),
        loadActivityPackCatalog(),
        loadRoomInventory(),
      ]);
      if (active) {
        setStatus(nextStatus);
        setLifecycle(nextLifecycle);
        setSecretStatus(nextSecretStatus);
        setRunnerTemplates(nextRunnerTemplates);
        setRunnerInstances(nextRunnerInstances);
        setActivityPackCatalog(nextActivityPackCatalog);
        setActivityPackCatalogLoading(false);
        setRoomInventory((previous) => staleAfterFailedRefresh(previous, nextRoomInventory));
      }
    };
    void refresh();
    const interval = window.setInterval(() => void refresh(), REFRESH_INTERVAL_MS);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
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
    />
  );
}

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root mount point");
createRoot(root).render(<StrictMode><LiveStudio /></StrictMode>);
