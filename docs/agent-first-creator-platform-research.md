# Agent-first creator platform research

**Date:** 2026-09-11
**Scope:** Product lessons from creator platforms, remote agent protocols, and multi-agent environment APIs. This note uses 12 primary sources: official documentation, specifications, and project repositories. It does not change the canonical WorldStream model.

## Findings: creator loop and progressive sharing

### Evidence

Roblox offers a legible progression: build and playtest in Studio, publish a cloud-backed experience that is private by default, then deliberately widen its audience. Studio's solo tests run distinct client and server simulations; its programmatic test service can simulate up to eight clients. Publishing stores the place data model in Roblox's cloud, but a new experience remains private. Private access is limited to owners/editors; sharing with playtesters is a separate **Limited** audience choice, and public release adds account and content-compliance requirements. [Studio testing modes](https://create.roblox.com/docs/studio/testing-modes), [publishing and audience controls](https://create.roblox.com/docs/production/publishing/publish-games-and-places)

itch.io shows a smaller, web-native creator contract: choose “HTML Game,” upload one HTML file or a ZIP containing `index.html`, preview it after processing, and let itch host its assets inside an iframe. The platform imposes archive limits (including file count and extracted size) and documents relative-path, case-sensitivity, compression, and browser-performance pitfalls. A replacement upload is operationally easy, but the page does not present uploads as immutable releases or as a security review boundary. [itch.io HTML5 upload documentation](https://itch.io/docs/creators/html5)

### Recommendation for WorldStream

Adopt the understandable shape, not either platform's authority model:

1. **Create locally:** scaffold a code-first Activity Pack project, run deterministic fixtures and conformance checks locally, and preview through a separately running Activity Client. Current prompt support is bounded to naming and describing a fixed two-party negotiation blueprint; prompt-assisted project editing would be a proposed extension, and must not become part of authoritative execution.
2. **Create an immutable revision:** one explicit build produces an Activity Pack Revision and content-addressed Activity Pack Bundle plus conformance evidence. A separate client build produces an immutable Activity Client Release. The UX may call this “Create test build,” but it must show the identities behind the friendly label.
3. **Private preview:** upload through a reviewed GitHub submission and create an unlisted/private Activity Listing Revision pinned to the exact Pack, Client, and projector identities. An ordinary share URL is discovery, not participant authority. A separately issued Seat Invitation is an opaque, revocable pre-Genesis capability to claim one exact seat; it still requires sign-in and grants neither Membership nor Host authority.
4. **Public promotion:** require a new reviewed Listing Revision and explicit catalog/public-viewing policy. Never mutate a private revision into a public executable by flipping one visibility bit.

For the hobby/MVP, accept reviewed GitHub submissions only. Defer self-service executable upload, marketplace discovery, mutable in-browser authoring, and arbitrary client hosting. This preserves an itch-like short loop while keeping operator approval, immutable retention, Replay, and client deployment trust visible and enforceable.

The creator-facing flow can stay as simple as **Create → Test → Share with friends → Submit for public review**. Each friendly step resolves to exact immutable identities and scoped access behind the scenes; hiding that plumbing must never bypass it.

## Findings: MCP transport, authorization, and WebMCP

### Evidence

The current MCP specification defines two standard transports: stdio and **Streamable HTTP**. Streamable HTTP uses one endpoint supporting POST and GET, optionally upgrades responses or GETs to SSE, can be stateless or sessionful, and replaces the deprecated 2024-11-05 HTTP+SSE transport. It requires Origin validation, recommends localhost binding for local servers, and recommends authentication. Session IDs, if issued, are carried separately in `MCP-Session-Id`; resumability uses SSE event IDs and `Last-Event-ID`. [MCP Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)

MCP authorization is optional overall but specified for HTTP transports. A protected MCP server is an OAuth resource server. The current flow requires Protected Resource Metadata discovery, authorization-server discovery, PKCE-style OAuth 2.1 behavior, and RFC 8707 `resource` indicators so tokens are audience-bound to the intended server. It recommends least-privilege scopes and distinguishes HTTP authorization from stdio credential handling. [MCP authorization](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization)

OpenAI's Responses API illustrates the client work that remains: an application configures an MCP tool with a `server_url`, supplies authorization when required, imports the server's tool list, and handles approval requests; supported remote transports are Streamable HTTP and legacy HTTP/SSE. Sensitive calls default to approval, and OpenAI warns that remote servers are unverified third parties. [OpenAI MCP and Connectors guide](https://developers.openai.com/api/docs/guides/tools-connectors-mcp)

WebMCP is different in kind, not a replacement network transport. The 10 September 2026 draft exposes `document.modelContext` so a page can register JavaScript tools for a browser-mediated agent. Its own status says **Draft Community Group Report**, not a W3C Standard or Standards Track deliverable; declarative WebMCP is still marked TODO, and the draft calls out prompt injection, intent misrepresentation, privacy leakage, and same-origin risks. [WebMCP draft](https://webmachinelearning.github.io/webmcp/)

### Recommendation for WorldStream

Use Streamable HTTP plus standards-conformant OAuth for a future remote agent-facing integration boundary, but do not expose generic Host administration or treat MCP session state as Room state. MCP tools should be a scoped adapter over existing contracts: inspect the authorized Projection and Action Offers, submit only offered Actions, and receive ordinary receipts. Authorization must bind account/principal purpose, exact deployment, scopes, and target resource; it does not grant Membership, Runner authority, or Pack approval.

Do not promise that pasting a game URL into an arbitrary LLM will connect an agent. Remote MCP requires a compatible, explicitly configured client and authorization flow. WebMCP requires the game page to be open in a supporting browser/agent. A direct BYO integration can instead use an optional non-authoritative Client SDK.

Keep WebMCP inside each exact Activity Client Release as progressive enhancement. Register the same legal Actions offered to a human through that client; do not create agent-only Pack operations. A House Agent and BYO agent should reach the same legal Action set through their own scoped clients and credentials. Human UI may hide schemas and receipts, but no UI or WebMCP callback may bypass immutable identity, authorization, legality, or receipt plumbing.

Do not make WebMCP a launch dependency in the MVP. Feature-detect it, maintain a non-WebMCP client path, and pin any experiment to a tested browser/version because the draft and implementations are still moving.

## Findings: environment and evaluation interfaces

### Evidence

PettingZoo's primary AEC API models turn ordering explicitly: `agent_iter()` selects an agent; `last()` returns that agent's observation, accumulated reward, termination, truncation, and info; `step(action)` advances the environment. Its contrasting Parallel model collects simultaneous Actions by cycle. Optional action masks advertise valid moves. [PettingZoo AEC API](https://pettingzoo.farama.org/api/aec/)

PettingZoo also ships executable conformance tests. Its API test checks seeded reset shape, observations against declared spaces, lifecycle of live agents, consistency of reward/termination/truncation/info maps, numeric rewards, and correct terminal stepping. This is stronger evidence than accepting a package because it imports. [PettingZoo API test source](https://github.com/Farama-Foundation/PettingZoo/blob/main/pettingzoo/test/api_test.py)

TextArena optimizes for LLM interchange: an agent only implements string observation to string action; an environment supplies `reset`, current-player observation, `step`, and final rewards/game information. Its core supports per-recipient observations and secret roles, while the public loop supports self-play and model-vs-model evaluation. [TextArena README](https://github.com/TextArena/TextArena/blob/main/README.md), [TextArena core interface](https://github.com/TextArena/TextArena/blob/main/textarena/core.py)

### Recommendation for WorldStream

Borrow the adapter ergonomics and conformance mindset, not either library's state ownership. Publish tiny Runner/client adapters that map authorized Projection plus Action Offers to model input, parse model output into an offered Action, and record rejection/receipt outcomes. Keep the Activity Pack authoritative for legality, visibility, phase, and Outcome; keep model calls, prompts, retries, budgets, and private memory in external Runners.

The existing Rust WorldStream referee/runtime must remain separate from every model Runner and from the JavaScript Activity Client. Humans, House Agents, and BYO agents can have different presentation and execution paths, but must face equivalent Pack-defined rules and Action types for the same Role and knowledge. Their actual Action Offers remain Membership- and view-bound, with distinct identity and freshness fences; no participation source receives privileged game operations.

Evaluation should pin Activity Pack Revision, Bundle, Activity Client Release where relevant, Runner/Profile revision, seeds and setup, agent/model identifiers, and scoring projector. Report at least completion rate, invalid/rejected Action rate, per-role results, cost/latency, and Replay verification. Use fixed golden matches plus cross-play, role swaps, and adversarial malformed outputs. Do not collapse unrelated Activities into a universal leaderboard score.

## Isolation evidence and limits

WebAssembly Components expose only typed imports and exports; a component without an import for a resource cannot access it. This supports WorldStream's zero-import, WASI-free Pack execution boundary, but the Component Model documentation does not claim that this alone is hostile multi-tenant process isolation. Resource limits, runtime hardening, admission review, content-addressed retention, and fail-closed Replay checks remain necessary. [Component Model worlds](https://component-model.bytecodealliance.org/design/worlds.html)

These sources demonstrate interface and workflow patterns, not product-market fit, moderation capacity, legal compliance, or safe arbitrary-code hosting. Agent Heist should remain the technical proof of deterministic, multi-agent plumbing; the flagship creator experience should be a separate reviewed Activity whose appeal is understandable without knowing the architecture.
