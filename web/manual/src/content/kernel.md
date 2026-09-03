# Build on the Room Kernel

Applications build on WorldStream at four seams: rules, transport, execution,
and presentation. Keep each seam narrow so deterministic shared truth does not
become coupled to one UI or model provider.

## 1. Rules: ActivityPackV1

Implement the trusted Rust `ActivityPackV1` contract for initialization,
stimulus reduction, viewer-scoped projection, attention, codecs, schemas, and
immutable revision identity. The pack receives deterministic inputs; host I/O,
wall-clock reads, model calls, filesystem access, and network calls do not
belong in reduction.

Start with [Activity Pack overview](#/activity-packs/overview).

## 2. Transport: protocol clients

Human and product clients attach with a scoped Membership capability, install a
Projection Reset, process Observation Frames durably, ACK the highest contiguous
frame, and submit only an exact Action Offer against the Head that produced it.

Use the [HTTP/WebSocket reference](#/reference/http-api) and public Python SDK
as examples. The protocol requires correlation IDs, versioned envelopes,
bounded payloads, and explicit reset capability.

## 3. Execution: external agent Runners

The foundational integration is the local assignment-bound MCP helper. The
Supervisor creates an opaque launch reference for one exact Agent Profile
assignment. The helper resolves authority locally and exposes seven generic
tools. Any policy able to speak MCP over stdio can participate without learning
daemon-specific credentials or querying arbitrary Rooms.

Use the [MCP agent runbook](#/agents/mcp). The managed reference host is a
post-MVP convenience, not the kernel's interoperability boundary.

## 4. Presentation: independent clients

The CLI is the local host-operator surface and client launcher. Participant UI
belongs to an independently executing Activity Client—a browser, terminal,
mobile, SDK-based, or agent-owned application—and receives only
Membership-authorized responses. An Activity Pack is headless and never ships
a React component into the Controller. Presentation may group or label facts, but must
not invent canonical state, collapse integrity and freshness into one status,
or duplicate Activity Pack legality logic.

## Extension checklist

Before adding a new feature, ask:

- Is this shared authoritative truth or external/private policy state?
- Does it require a new canonical record, or only a Projection/read model?
- Which authority owns it, and what exact purpose is sealed?
- What happens after a lost response or process crash?
- Can Replay reconstruct the same result without external I/O?
- Which Memberships may see each field?
- Which exact revision owns the behavior?
- Does the SQLite and PostgreSQL conformance story remain identical?

## Conformance before convenience

A new kernel behavior should first exist as a storage-neutral contract and
black-box scenario. Then implement SQLite and PostgreSQL adapters, add protocol
surfaces, and finally add client convenience. Do not make a browser workflow
the only proof of a canonical invariant.

Primary source: [architecture invariants](https://github.com/imom39a/worldstream/blob/main/docs/architecture.md#architecture-invariants),
[Activity Pack contract](https://github.com/imom39a/worldstream/blob/main/docs/activity-packs.md),
and [UI architecture](https://github.com/imom39a/worldstream/blob/main/docs/ui-architecture.md).
