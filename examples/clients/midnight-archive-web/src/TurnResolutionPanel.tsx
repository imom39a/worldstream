import type { MidnightArchiveProjection } from "./model";

export function TurnResolutionPanel({ projection }: { readonly projection: MidnightArchiveProjection }) {
  if (projection.mira.presence === "absent" && projection.jonah.presence === "absent") return null;
  const resolution = projection.turnResolution;
  return <section className={`turn-resolution resolution-${resolution.status}`} aria-labelledby="turn-resolution-title">
    <p className="archive-kicker">Shared turn preparation</p>
    <h2 id="turn-resolution-title">{resolution.status === "clear" ? "Turn is clear" : "Turn has conflicts"}</h2>
    <p>{resolution.powerReserved} of {projection.power} remaining power reserved for this turn.</p>
    {resolution.reservations.length === 0 ? <p>No prepared effect reserves a scarce resource.</p> : <ol className="resolution-order">
      {resolution.reservations.map((reservation) => <li key={reservation.role}>
        <strong>{roleName(reservation.role)} · {reservation.power} power</strong>
        <span>{preparedEffect(projection, reservation.role)} · {interactionLabel(reservation.interaction)}</span>
      </li>)}
    </ol>}
    {resolution.preparedRoles.length === 0 ? null : <p><strong>Prepared:</strong> {roleList(resolution.preparedRoles)}</p>}
    {resolution.deferredRoles.length === 0 ? null : <p><strong>Deferred:</strong> {roleList(resolution.deferredRoles)}</p>}
    {resolution.unpreparedRoles.length === 0 ? null : <p><strong>No prepared effect:</strong> {roleList(resolution.unpreparedRoles)}</p>}
    {resolution.conflicts.length === 0 ? null : <ul className="resolution-conflicts">
      {resolution.conflicts.map((conflict) => <li key={`${conflict.code}:${conflict.roles.join("-")}`}>
        <strong>{conflictLabel(conflict.code)}</strong><span>{roleList(conflict.roles)}</span>
      </li>)}
    </ul>}
  </section>;
}

function preparedEffect(projection: MidnightArchiveProjection, role: "lead" | "mira" | "jonah"): string {
  if (role === "mira") return projection.mira.preparation.summary;
  if (role === "jonah") return projection.jonah.preparation.summary;
  return projection.stagedAction === null
    ? "No personal Action staged"
    : projection.stagedAction.actionType.replace("stage_", "").replaceAll("_", " ");
}

function conflictLabel(code: MidnightArchiveProjection["turnResolution"]["conflicts"][number]["code"]): string {
  if (code === "shared_power") return "Shared power is over-reserved";
  if (code === "service_hatch") return "The service hatch has competing work";
  if (code === "catalog_verifier") return "The catalog verifier has competing work";
  return "A prepared contribution is no longer eligible";
}

function interactionLabel(interaction: MidnightArchiveProjection["turnResolution"]["reservations"][number]["interaction"]): string {
  if (interaction === "service_hatch") return "Plant service hatch";
  if (interaction === "catalog_verifier") return "Catalog verifier";
  return "Independent effect";
}

function roleName(role: "lead" | "mira" | "jonah"): string {
  return role === "lead" ? "Lead" : role === "mira" ? "Mira" : "Jonah";
}

function roleList(roles: readonly ("lead" | "mira" | "jonah")[]): string {
  return roles.map(roleName).join(", ");
}
