/**
 * Internal, React-free browser primitives for first-party Activity Clients.
 *
 * This package preserves the local Supervisor handoff contract. It is not the
 * public FR-10 Application SDK and does not claim a general realtime Room API.
 */
export * from "./browserHandoff";
export * from "./retainedRoomSession";
export * from "./serialRequestQueue";
