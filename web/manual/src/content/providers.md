# Provider and agent recipes

These are **integration recipes, not compatibility certifications**. The
repository currently has no checked-in end-to-end acceptance fixture for any
specific commercial model/agent host. Verify tool discovery and a real Activity
turn before recording support.

## Compatibility matrix

| Agent/provider | Best local path | Current caveat |
| --- | --- | --- |
| OpenAI Codex / ChatGPT desktop | local stdio MCP helper on the Codex host | assignment reference must remain local/private; acceptance not yet checked in |
| ChatGPT web | future approved remote MCP or Secure MCP Tunnel | cannot connect directly to the current local stdio helper |
| Claude Code | local stdio MCP helper | no checked-in Claude acceptance fixture |
| Anthropic API | developer-owned external runner/translator | native Messages API is not the managed host's chat-completions contract |
| xAI Grok API | custom external MCP agent or trusted loopback adapter | public HTTPS endpoint cannot be used directly by the loopback-only host |
| DeepSeek API | custom external MCP agent or trusted loopback adapter | normalize TLS/base path and verify exact JSON-object response |
| OpenClaw | outbound local stdio MCP helper | structurally compatible; acceptance not yet checked in |

## OpenAI Codex and ChatGPT desktop

Configure this repository's helper as a local stdio MCP server in the Codex
host, using an active assignment reference:

```text
command: /absolute/path/to/target/debug/worldstream-assignment-mcp
args:
  - --launch-reference
  - <SUPERVISOR_ISSUED_REFERENCE>
  - --state-dir
  - /absolute/owner-only/path/.worldstream/studio
```

OpenAI documents stdio MCP support on Codex hosts. ChatGPT web is a different
boundary: it connects to remote MCP servers and does not directly launch this
local process. See [OpenAI MCP documentation](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)
and [ChatGPT developer mode/MCP apps](https://help.openai.com/en/articles/12584461-developer-mode-and-full-mcp-connectors-in-chatgpt).

## Claude Code

Anthropic documents local stdio registration with this command form:

```sh
claude mcp add worldstream -- \
  /absolute/path/to/target/debug/worldstream-assignment-mcp \
  --launch-reference <SUPERVISOR_ISSUED_REFERENCE> \
  --state-dir /absolute/owner-only/path/.worldstream/studio
```

Confirm the seven tools are discoverable before asking Claude to acquire work.
See [Claude Code MCP](https://code.claude.com/docs/en/mcp). Claude's native
Messages API has its own request/authentication shape; use a developer-owned
adapter rather than assuming it is OpenAI-compatible. See the
[Anthropic API overview](https://platform.claude.com/docs/en/api/overview).

## OpenClaw

Register the helper as an outbound stdio MCP server using OpenClaw's documented
`mcp add` command with `--command`, repeated `--arg`, and an owner-only working
directory. Use placeholders for the launch reference and probe the connection
before a real turn:

```text
openclaw mcp add worldstream \
  --command /absolute/path/to/worldstream-assignment-mcp \
  --arg --launch-reference --arg <REFERENCE> \
  --arg --state-dir --arg /absolute/path/.worldstream/studio
openclaw mcp doctor worldstream --probe
```

Check the current syntax in [OpenClaw MCP CLI](https://docs.openclaw.ai/cli/mcp).
Provider availability in OpenClaw does not by itself prove WorldStream
interoperability; see its [provider directory](https://docs.openclaw.ai/providers).

## Grok and DeepSeek: design boundary, not current support

xAI and DeepSeek expose chat-completion-compatible API shapes, but the current
WorldStream managed host accepts only a **loopback socket** and sends plain HTTP
to fixed `/v1/chat/completions`. Therefore use either:

1. a custom external agent that owns the provider HTTPS SDK and talks to
   WorldStream through MCP; or
2. a trusted local loopback proxy that owns TLS/auth and converts the provider
   response to the host's exact bounded JSON-object contract.

No such adapter, external runner, provider-specific fixture, or acceptance
command is shipped in this repository today. Treat both integrations as
**design only**, not supported setup. Do not point the managed host at the
providers' public HTTPS endpoints or place an API key in WorldStream
configuration.

The adapter must return one selected `offer_id` and `payload` inside
`choices[0].message.content`; WorldStream revalidates the exact offer, schema,
Head, and authority before daemon submission.

First-party API references: [xAI Chat Completions](https://docs.x.ai/developers/model-capabilities/legacy/chat-completions),
[xAI text generation](https://docs.x.ai/developers/model-capabilities/text/generate-text),
and [DeepSeek Chat Completions](https://api-docs.deepseek.com/api/create-chat-completion/).

Avoid hard-coding “latest” model IDs in shared docs. Provider models change
outside WorldStream's revision contract; pin an exact tested model in an Agent
Profile revision and date the acceptance evidence.

To promote either provider from design to a usable recipe, check in the adapter
or external runner, owner-only credential setup, exact model/profile fixture,
ambiguous-retry and restart tests, and one end-to-end acceptance command. Until
that evidence exists, the MCP-capable Codex/Claude/OpenClaw paths are the only
concrete external-host recipes in this manual.

## Acceptance checklist per provider

- helper starts without exposing the launch reference in logs;
- exactly seven tools are discovered;
- only the assigned Task is visible;
- observation is processed before ACK;
- the model selects only an exact offered Action and valid payload;
- ambiguous retry returns the same receipt;
- helper restart resumes lease/Cursor without duplicates;
- completion uses an exact closed disposition;
- prompts, responses, browser evidence, and logs contain no authority material.
