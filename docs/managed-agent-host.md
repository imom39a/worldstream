# Managed reference Agent Host

Status: post-MVP reference implementation. External assignment-bound MCP agents remain the foundational execution model.

The managed reference host is a Supervisor-owned convenience for one approved Agent Profile revision and Runner Template revision. It does not move model execution into `worldstreamd` and does not expand the authority of a participant or Runner.

## Isolation boundary

The Supervisor starts two fixed child processes:

1. `worldstream-assignment-mcp` receives the owner-only Supervisor state directory and one opaque, active assignment launch reference. It resolves the sealed participant and Runner authorities for that assignment only.
2. `worldstream-managed-agent-host` receives no Supervisor path, launch reference, Room ID, Membership ID, participant bearer, or Runner bearer. It receives only the bridged MCP stdio channel and the exact provider/model configuration selected by the immutable Agent Profile revision.

The model credential is resolved from the Supervisor vault immediately before launch and delivered only to the model-host process over its private stdin framing. It is bounded and zeroized after delivery. Child stderr is discarded, and browser-visible Operations status contains only stable assignment/host revision, lifecycle, capacity, freshness, and closed remediation text.

The reference provider adapter accepts only the typed Agent Profile API value `open_ai_compatible` (CLI value `openai-compatible`), a loopback socket address, and the fixed `/v1/chat/completions` path. Requests and responses have total byte and time limits. The provider must return one exact listed Action offer and payload; the assignment MCP helper revalidates the offer, schema, Head precondition, and authority before daemon submission.

## Lifecycle and recovery

The post-setup start operation is available only after Task setup is durably `Ready`, its exact Agent Profile assignment is retained, and the seat's Runner capability is durably provisioned. The profile must use the `managed_reference` host contract and pin the exact approved Runner Template revision, host contract revision, provider, loopback provider address, model ID, and one `MODEL_PROVIDER_TOKEN` secret reference.

Start intent is persisted before either child is launched. Identical retries reuse the same immutable binding. Supervisor restart, early child exit, and the crash window between process launch and the `Running` checkpoint reconcile to bounded Operations attention or the observed live process; they never select another assignment, host, template, provider, model, or credential reference. Dropping the process handle kills and reaps both children.

Within a turn, activation and Action operation identities are derived from the exact activation cursor, lease generation, and context digest. The host uses the generic assignment contract in this order: acquire Activation, observe, list current offers, submit the exact selected offer, acknowledge delivered frames only after the Action receipt is durably retained, and complete the Activation with one closed `handled`, `declined`, or `failed` disposition. A typed no-work result keeps the process alive with bounded backoff. The assignment MCP Action and Activation ledgers provide authoritative idempotent reconciliation across ambiguous daemon responses and process restarts.

## Operational scope

Studio labels external assignment-bound MCP as the foundational execution path and managed reference hosts as post-MVP. Exposing this post-MVP configuration does not change the MVP completion gate or make a managed host an MVP dependency. The existing Studio Runner attention panel includes managed reference host lifecycle, capacity, freshness, and safe failure guidance. It intentionally excludes provider prompts/responses, Invocation Context, memory, local paths, process arguments, secret references, and authority material. Start, safe retry, stop, and status remain bounded Supervisor operations for exact retained assignment IDs; there is no arbitrary executable, URL, storage, Room, Membership, or authority input.
