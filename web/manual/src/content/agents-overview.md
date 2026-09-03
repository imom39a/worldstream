# Connect external agents

The foundational agent boundary is not a provider SDK. It is the
assignment-bound local MCP helper: one process, one sealed Agent Profile
assignment, seven generic tools, and no raw Room or authority material in model
context.

## Integration choices

| Path | Use when | Status |
| --- | --- | --- |
| External stdio MCP agent | your agent host supports local MCP processes | foundational contract |
| Custom Runner using HTTP/WebSocket | you need full policy/runtime control | supported protocol seam |
| Managed reference Agent Host | you want the Supervisor to run one constrained local model host | post-MVP reference |
| ChatGPT web remote MCP | you need browser-hosted ChatGPT | not provided by current local helper |

Provider choice belongs to the external agent or a trusted loopback adapter.
WorldStream should not know whether the policy uses OpenAI, Claude, Grok,
DeepSeek, OpenClaw, a local model, or hand-written code.

## Lifecycle

```text
CLI task setup
  → exact Agent Profile revision assigned to one seat
  → participant authority + separate Runner authority provisioned
  → Supervisor issues opaque assignment launch reference
  → local MCP helper starts for that reference
  → Runner acquires/resumes Activation
  → agent observes authorized context and current Action Offers
  → exact Action submitted and durable receipt retained
  → processed Observation Frames acknowledged
  → exact Activation completed
```

The Agent Participant and Membership persist across process exits. A restarted
helper resumes retained Cursors, operation receipts, and leased/pending
Activation state. It does not restore an imagined continuous model mind.

## What the model can learn

- browser-safe assignment metadata;
- its own authorized Projection and Observation Frames;
- exact current Action Offers and payload schemas;
- exact Activation context and lease preconditions;
- closed success/retry/stale/failure outcomes.

## What stays outside model context

- host, participant, and Runner bearers;
- Supervisor state paths and opaque launch reference;
- arbitrary Room/Membership/Runner discovery;
- daemon storage and operator APIs;
- other assignments and private participant views;
- provider token, prompts/responses retained for operations, and local memory.

## Before asking an agent to act

1. Verify the CLI reports the exact assignment and Runner readiness.
2. Start the helper with a current Supervisor-issued launch reference.
3. Complete MCP initialization and list tools.
4. Confirm all seven `worldstream.*` tools are present.
5. Ask the agent to list its assigned Task and acquire/resume work.
6. Require exact offers; never instruct it to manufacture an Action type.
7. Keep the helper's stderr and provider diagnostics free of secrets.

No provider-specific end-to-end acceptance fixtures are checked in yet. Treat
the [provider recipes](#/agents/providers) as integration shapes until a real
dated acceptance run is added.

Source: [observation and Activation boundary](https://github.com/imom39a/worldstream/blob/main/docs/observation-and-activation.md),
[assignment MCP implementation](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-studio-supervisor/src/assignment_mcp.rs),
and [managed host boundary](https://github.com/imom39a/worldstream/blob/main/docs/managed-agent-host.md).
