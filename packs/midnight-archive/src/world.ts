import type { ArchiveState, Location } from "./model.js";

export function legalDestinationsFrom(state: ArchiveState, location: Location): Location[] {
  const destinations: Location[] = [];
  const add = (left: Location, right: Location, open = true): void => {
    if (!open) return;
    if (location === left) destinations.push(right);
    if (location === right) destinations.push(left);
  };
  add("atrium", "records");
  add("atrium", "conservation");
  add("records", "conservation");
  add("records", "plant");
  add("conservation", "vault", state.gates.conservation_vault_open);
  add("plant", "vault", state.gates.plant_vault_open);
  return destinations.sort();
}

export function nextLocationToward(
  state: ArchiveState,
  origin: Location,
  destination: Location,
): Location | null {
  if (origin === destination) return null;
  const queue: Array<{ location: Location; first: Location }> = [];
  const seen = new Set<Location>([origin]);
  for (const neighbor of legalDestinationsFrom(state, origin)) {
    queue.push({ location: neighbor, first: neighbor });
    seen.add(neighbor);
  }
  while (queue.length > 0) {
    const candidate = queue.shift()!;
    if (candidate.location === destination) return candidate.first;
    for (const neighbor of legalDestinationsFrom(state, candidate.location)) {
      if (seen.has(neighbor)) continue;
      seen.add(neighbor);
      queue.push({ location: neighbor, first: candidate.first });
    }
  }
  return null;
}
