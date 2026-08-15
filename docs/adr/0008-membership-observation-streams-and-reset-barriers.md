# ADR 0008: Membership Observation Streams and Reset Barriers

Status: Accepted, 2026-08-15

Each Membership owns one monotonically sequenced Observation Stream with an independent frame head, retained floor, and durable Cursor; Genesis emits no frame and each later Transition emits at most one coalesced frame for that viewer. Attach captures a complete Room/frame barrier and returns either the retained range or a full Projection Reset on first attach, a below-floor/pruned range, visibility loss, or another incremental-inappropriate condition. Only `room.sync_ack` carrying that Session's opaque token makes that Session Live, and it never advances Cursor; only the distinct `observation.ack` may advance the shared Membership Cursor, and it never satisfies a Session barrier. This preserves privacy and creates a gap-free reconnect contract at the cost of per-Membership materialization and explicit reset handling.
