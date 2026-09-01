# WorldStream internal browser client

`@worldstream/client` contains React-free primitives shared by first-party
Activity Clients. Its current boundary is deliberately narrow:

- consume and immediately scrub a one-use Supervisor handoff;
- retain the Supervisor's HttpOnly Membership session;
- read authorized delivery batches;
- submit exact offered Actions through that retained authority;
- request authorized verified Replay; and
- serialize polling and user operations.

Its code-facing vocabulary is Activity Client terminology:

- `ActivityClientHandoffClient` consumes and resumes local handoffs;
- `AuthorizedRoomDeliveryBatch` is the validated result of `observe`;
- `OfferedActionSubmission` and `ActivityClientActionReceipt` bound `act`;
- `VerifiedRoomReplay` is the validated result of `replay`; and
- `ActivityClientSession` exposes a live `deliveryBatch` plus reconnect state.

An Action receipt is an opaque browser-safe record whose required `state` is
either `accepted` or `rejected`. Successful primitive, array, or unknown-state
responses are rejected rather than leaking an untyped value into a client.

The package preserves the existing `/api/v1/participant-console/*` wire routes,
handoff header, safe error codes, and `participant_console_session.v1` response
because those are frozen compatibility identities, not code-facing product
terminology. Legacy Console aliases live only in the Console compatibility
shims; they are not exported by this package.

This package is private and internal. It is not the FR-10 public Application
SDK, a general realtime Room protocol implementation, or Pack presentation
code. Activity Clients remain responsible for interpreting their Pack's
authorized Projection and Observation schemas.
