# Managed reference Agent Host

Status: post-MVP reference implementation. An external assignment-bound MCP
agent remains the foundational execution model.

The managed reference Agent Host is a Controller-owned convenience for one
approved Agent Profile revision and Runner Template revision. It does not move
model execution into `worldstreamd`, and it does not expand participant or
Runner authority. The Controller binary keeps the compatibility name
`worldstream-studio-supervisor`.

## Isolation boundary

The Controller starts two fixed child processes:

1. `worldstream-assignment-mcp` receives the owner-only Controller state
   directory and one opaque active assignment launch reference. It can resolve
   sealed participant and Runner authority for that assignment only.
2. `worldstream-managed-agent-host` receives no Controller path, launch
   reference, Room ID, Membership ID, or bearer. It receives only the bridged
   MCP stdio channel and the exact provider and model configuration selected by
   the immutable Agent Profile revision.

The Controller resolves the model credential from its vault immediately before
launch. It sends the credential only to the model-host process through private
stdin framing and zeroizes the local buffer after delivery. Browser-visible
status excludes prompts, responses, Invocation Context, memory, local paths,
process arguments, secret references, and authority material.

The reference provider adapter accepts only the typed Agent Profile API value
`open_ai_compatible` (CLI value `openai-compatible`), a loopback address, a
fixed `/v1/chat/completions` path, and bounded request and response sizes and
times. The provider must return one listed Action Offer and its payload. The
assignment MCP helper revalidates the offer, schema, Room Head precondition,
and authority before it submits the Action.

## CLI lifecycle

Import the reviewed Runner Template, provider declaration, and Agent Profile
through the two-phase `worldstreamctl init` flow before Controller startup.
Room setup records the exact assignment. After setup reports `ready`, use only
the bounded CLI operations for that operation and seat:

```sh
worldstreamctl runner inspect --operation OPERATION_ID --seat SEAT
worldstreamctl runner start --operation OPERATION_ID --seat SEAT
worldstreamctl runner stop --operation OPERATION_ID --seat SEAT
```

Start intent is durable before either child process starts. An identical retry
reuses the same immutable binding. Controller restart, early child exit, and a
crash between process launch and the running checkpoint reconcile to bounded
operator attention or the observed live process. They never select another
assignment, template, provider, model, or credential reference.

Within one turn, identities derive from the exact Activation cursor, lease
generation, and context digest. The Host acquires an Activation, observes the Room, and lists current offers. It submits the selected offer and retains the Action receipt. It then acknowledges delivered frames and completes the Activation with one closed `handled`, `declined`, or `failed` disposition. A no-work result
keeps the process alive with bounded backoff. The assignment MCP ledgers make
ambiguous retries idempotent across process restart.

## Scope

The CLI and Controller expose status, safe retry, start, and stop for exact
retained assignment identities. They do not accept an arbitrary executable,
URL, storage path, Room, Membership, command, or authority value. There is no
web administration panel. Model execution stays outside `worldstreamd`, and
the Controller does not mutate Authoritative Room State.
