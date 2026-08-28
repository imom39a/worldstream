import type { OperatorRoom, RoomInventoryState } from "./roomInventory";
import type { RoomOperatorView } from "./roomOperatorView";
import "./roomOperations.css";

export function RoomOperations({
  inventory,
  selectedRoomId,
  onSelectRoom,
  operatorView = null,
  operatorViewLoading = false,
  onEnableOperatorView,
}: {
  inventory: RoomInventoryState;
  selectedRoomId?: string | null;
  onSelectRoom?: (roomId: string) => void;
  operatorView?: RoomOperatorView | null;
  operatorViewLoading?: boolean;
  onEnableOperatorView?: (roomId: string) => void;
}) {
  if (inventory.status === "loading") {
    return <section className="room-operations" aria-busy="true"><p>Loading Rooms…</p></section>;
  }
  if (inventory.status === "unavailable") {
    return (
      <section className="room-operations" role="status">
        <h2>Rooms unavailable</h2>
        <p>The Supervisor could not obtain a host-authorized Room inventory.</p>
      </section>
    );
  }
  if (inventory.page.rooms.length === 0) {
    return (
      <section className="room-operations" role="status">
        <h2>No Rooms yet</h2>
        <p>The configured host authority has no visible Rooms.</p>
      </section>
    );
  }

  const selected = inventory.page.rooms.find((room) => room.room_id === selectedRoomId)
    ?? inventory.page.rooms[0];
  return (
    <section className="room-operations" aria-label="Room operations">
      <div className="room-list">
        <p className="eyebrow">Tasks · Rooms</p>
        <h2>Authoritative Rooms</h2>
        {inventory.page.rooms.map((room) => (
          <button
            className={room.room_id === selected?.room_id ? "is-selected" : undefined}
            key={room.room_id}
            type="button"
            onClick={() => onSelectRoom?.(room.room_id)}
          >
            <strong>{room.pack.id}</strong>
            <span>{room.room_id}</span>
            <small>{integrityLabel(room)}</small>
          </button>
        ))}
      </div>
      {selected ? <RoomDetail room={selected} operatorView={operatorView?.room_id === selected.room_id ? operatorView : null} operatorViewLoading={operatorViewLoading} onEnableOperatorView={onEnableOperatorView} /> : null}
    </section>
  );
}

export function RoomDetail({ room, operatorView = null, operatorViewLoading = false, onEnableOperatorView }: { room: OperatorRoom; operatorView?: RoomOperatorView | null; operatorViewLoading?: boolean; onEnableOperatorView?: (roomId: string) => void }) {
  return (
    <article className="room-detail">
      <p className="eyebrow">Room detail</p>
      <h2>{room.pack.id}</h2>
      <p className="room-identity">{room.room_id}</p>
      <dl className="room-facts">
        <Fact label="Setup progress" value={setupLabel(room)} />
        <Fact label="Participant readiness" value={readinessLabel(room)} />
        <Fact label="Activity phase" value={phaseLabel(room)} />
        <Fact label="Room integrity" value={integrityLabel(room)} />
        <Fact label="Integrity generation" value={String(room.integrity.generation)} />
        <Fact label="Data freshness" value={freshnessLabel(room)} />
        <Fact label="Head sequence" value={String(room.room_head.room_seq)} />
        <Fact label="Exact Activity Pack" value={`${room.pack.id} ${room.pack.version}`} />
      </dl>
      <p className="privacy-note">Participant-private seat and Invocation data are not included.</p>
      <section aria-label="Operator view">
        <h3>Operator view</h3>
        {operatorView?.state === "available" ? <p>Counter value: {operatorView.counter.value} · Head sequence: {operatorView.room_head.room_seq}</p> : <>
          <p>{operatorView?.unavailable_reason?.replaceAll("_", " ") ?? "Operator projection is unavailable."}</p>
          <button type="button" disabled={operatorViewLoading || onEnableOperatorView === undefined} onClick={() => onEnableOperatorView?.(room.room_id)}>{operatorViewLoading ? "Enabling operator view…" : "Enable operator view"}</button>
        </>}
      </section>
    </article>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>;
}

function setupLabel(room: OperatorRoom) {
  const setup = room.setup_progress;
  if (setup.status === "complete") return `Complete · ${setup.completed_steps}/${setup.total_steps}`;
  if (setup.status === "partially_provisioned") return `Partially provisioned · ${setup.completed_steps}/${setup.total_steps}`;
  return "Unavailable";
}

function readinessLabel(room: OperatorRoom) {
  const readiness = room.participant_readiness;
  return readiness.status === "available"
    ? `${readiness.ready}/${readiness.total} ready`
    : "Unavailable · operator membership required";
}

function phaseLabel(room: OperatorRoom) {
  return room.activity_phase.status === "available"
    ? room.activity_phase.value
    : "Unavailable · operator membership required";
}

function integrityLabel(room: OperatorRoom) {
  return room.integrity.status.charAt(0).toUpperCase() + room.integrity.status.slice(1);
}

function freshnessLabel(room: OperatorRoom) {
  const freshness = room.freshness;
  if (freshness.status === "fresh") return `Fresh · ${freshness.observed_at}`;
  if (freshness.status === "stale") return `Stale · last observed ${freshness.observed_at}`;
  return "Unavailable";
}
