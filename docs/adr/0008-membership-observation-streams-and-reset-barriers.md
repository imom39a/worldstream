# ADR 0008: Membership Observation Streams and Reset Barriers

Status: Accepted, 2026-08-15

Each Membership owns one monotonically sequenced Observation Stream with an independent frame head, retained floor, and durable Cursor; Genesis emits no frame and each later Transition emits at most one coalesced frame for that viewer. Attach captures a complete Room/frame barrier and returns either the retained range or a full Projection Reset, and only an acknowledgement carrying that Session's opaque sync token makes that Session Live. This preserves privacy and creates a gap-free reconnect contract even when frames are pruned, at the cost of per-Membership materialization and explicit reset handling.
