# Managed reference Agent Host

The managed reference host is a post-MVP convenience for running one constrained
model-host process under the headless Controller. External assignment-bound MCP
remains the foundational interoperability contract.

## Isolation boundary

The Controller starts two fixed child processes:

1. `worldstream-assignment-mcp` receives the owner-only state directory and one
   opaque launch reference, then resolves sealed participant and Runner
   authority for that assignment.
2. `worldstream-managed-agent-host` receives only bridged MCP stdio plus exact
   provider/model configuration. It receives no state path, launch reference,
   Room/Membership ID, bearer, or daemon storage access.

The model token is resolved immediately before launch, delivered through a
private stdin frame, bounded, and zeroized. Child stderr is discarded. Controller
status excludes prompts, responses, Invocation Context, memory, arguments,
paths, secret references, and authority.

## Exact profile requirements

The immutable Agent Profile revision must pin:

- host contract `managed_reference`;
- exact approved Runner Template revision and instance;
- exact host contract revision;
- provider `open_ai_compatible`;
- loopback provider socket address;
- exact model ID;
- one kind-bound `MODEL_PROVIDER_TOKEN` reference.

## Provider adapter contract

The current host is intentionally narrow:

- transport: stdio to assignment MCP;
- provider address: loopback only;
- provider request: plain HTTP `POST /v1/chat/completions`;
- authentication: Bearer token from private launch framing;
- response mode: JSON object;
- result: exact current `offer_id` plus `payload`;
- total request/response time and bytes are bounded.

It is not a general provider URL field, TLS client, Responses API host, native
Anthropic Messages client, or arbitrary tool runner. Use a trusted loopback
adapter or an external MCP agent for those needs.

## Recovery and operations

Start intent is retained before child launch. Identical retries preserve the
same assignment/profile/template/provider/model binding. Supervisor restart and
early child exit reconcile to the observed process or a bounded attention item;
they never choose a different assignment.

Within a turn, the host acquires/resumes Activation, observes, lists offers,
submits one exact offer, retains the Action response before ACK, and completes
with `handled`, `declined`, or `failed`. Typed no-work keeps the process alive
with bounded backoff.

The Controller reports only bounded lifecycle, selected capacity use, MCP-derived
freshness, and safe remediation. PID existence alone does not establish
freshness.

Source: [managed Agent Host contract](https://github.com/imom39a/worldstream/blob/main/docs/managed-agent-host.md)
and [host implementation](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-studio-supervisor/src/managed_agent_host_main.rs).
