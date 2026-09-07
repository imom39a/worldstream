# WebMCP as a WorldStream Agent Participation Surface

Status: research note

Date: 2026-09-03

Question: Can a person open a WorldStream game URL in ChatGPT, Codex, or another agent on their own machine and let that agent participate through WebMCP?

## Decision in one paragraph

Yes, with an important boundary: **WebMCP should be an optional interface implemented by a browser Activity Client, not a WorldStream kernel feature and not a replacement for the WorldStream client protocol.** It can make the first experience unusually simple: open an invite URL in a supported agentic browser, approve the site, and tell the agent to play. The page continues to use WorldStream's authenticated HTTPS/WebSocket client contract for current state and Actions. WebMCP exposes a small, structured view of that already-authorized client functionality to the browser's agent. Direct SDK clients and conventional Runners remain the correct paths for headless, unattended, or verifiable agents.

## What WebMCP is—and is not

The current WebMCP document is a **W3C Community Group draft**, not a W3C Standard or a specification on the W3C Standards Track. Its purpose is to let a web page register JavaScript-backed tools for agents. A tool has a name, description, JSON input schema, execution callback, and optional annotations such as `readOnlyHint` and `untrustedContentHint`. The execution callback runs in the page and can return a Promise. [WebMCP draft specification](https://webmachinelearning.github.io/webmcp/)

This means that WebMCP is a page capability, not a hosted protocol endpoint. A page calls `document.modelContext.registerTool(...)`; a compatible browser or browser agent discovers those tools while the document is open. The draft explicitly leaves the mechanism by which a browser exposes the tools to its agent implementation-defined. It may use MCP, proprietary function calling, or another mechanism. [WebMCP interaction with agents](https://webmachinelearning.github.io/webmcp/#interaction-with-agents)

Google's guidance makes the boundary especially clear: WebMCP is ephemeral and tab-bound, whereas ordinary MCP is persistent and can be available without a web page. WebMCP uses the live page state, cookies, and DOM. Its tools disappear when the user closes or leaves the page. [Chrome: When to use WebMCP and MCP](https://developer.chrome.com/docs/ai/webmcp/compare-mcp)

Therefore:

- A WorldStream web URL can expose WebMCP tools after it is opened in a compatible browser.
- That URL is **not** a remote MCP server URL that a CLI can register by itself.
- WebMCP does not host or supply the agent. The visiting browser or extension supplies the agent.
- WebMCP does not give the page a standard way to request arbitrary model completions from the visitor's model. The current API standardizes tools the agent calls, not a reverse `sampling` operation.
- WebMCP does not replace Room state, the Observation Stream, Cursor/Catch-up, Action admission, Replay, timers, or Runner activation.

The current tool callback receives the tool input and an abort signal. It does not receive a standardized model identity, system prompt, usage meter, stable agent identity, or attestation. This is an inference from the current WebIDL, not a claim that a particular browser can never add product-specific metadata. [WebMCP `ModelContextTool` and callback WebIDL](https://webmachinelearning.github.io/webmcp/#modelcontexttool-dictionary)

## Does “paste the URL into ChatGPT or Codex” work?

### Confirmed OpenAI path

OpenAI documents WebMCP as **site tools** in the ChatGPT desktop app's built-in browser. The user opens the site in that browser, signs in on the site if needed, reviews the website-access prompt, and then asks ChatGPT to work with the current page. Tool use depends on the user's account, selected model, and whether the current page provides a matching tool. The page must remain open. [OpenAI: Using site tools](https://help.openai.com/en/articles/20001423-using-site-tools-in-the-chatgpt-desktop-app)

OpenAI's built-in-browser instructions say to open a browser from a **Work or Codex** chat and ask ChatGPT to use the current page. Taken together with the site-tools documentation, this is the documented route for trying a WorldStream WebMCP client from a Codex task in the desktop experience. [OpenAI: Using the built-in browser](https://help.openai.com/en/articles/20001277-using-the-built-in-browser-in-the-chatgpt-desktop-app)

The reliable instruction is therefore:

1. Open the invite link in the ChatGPT desktop app's built-in browser from a Work or Codex chat.
2. Sign in or redeem the invitation on the web page.
3. Confirm that the address-bar site-tools indicator appears.
4. Tell the agent to read the rules and play the match.
5. Keep the match page open until the agent finishes or intentionally disconnects.

Pasting a URL into an arbitrary ChatGPT client is not a portable guarantee. The official documentation currently confirms site tools in the desktop built-in browser, subject to account and model availability. It does not establish URL-based WebMCP discovery for every ChatGPT web/mobile surface or for a standalone Codex CLI process.

OpenAI's own WebMCP Challenge page says its in-app browser supports WebMCP out of the box. This provides a practical test target, but WebMCP is still described as experimental. [OpenAI WebMCP Challenge](https://openai.com/webmcp-challenge/)

| Agent surface | Can a user start from the game URL? | What is actually required today? |
| --- | --- | --- |
| ChatGPT desktop built-in browser | **Yes, supported with conditions** | Open the page in the built-in browser, complete page authentication, approve website access, use a supported account/model, and keep the page open. |
| Codex task in the ChatGPT desktop experience | **Yes, through the same built-in-browser route** | OpenAI's browser instructions explicitly start from a Work or Codex chat; site-tool availability remains account/model/page dependent. |
| ChatGPT web or mobile | **Do not promise it** | The cited site-tools documentation currently identifies the desktop built-in browser as the supported surface. |
| Standalone Codex CLI | **No, not from a WebMCP URL alone** | A web URL is not an MCP server configuration. Use the WorldStream Client SDK, a conventional MCP bridge, or a controlled WebMCP-aware browser. |
| Chrome 149+ | **Only with additional setup** | Enable the origin trial or development flag and provide a compatible browser agent/extension; the browser API by itself does not choose Actions. |
| Arbitrary local agent | **Only if it has a WebMCP browser integration** | Otherwise use the direct WorldStream client protocol or SDK. |

### Chrome and other local agents

Chrome documents WebMCP as a proposed standard under active development. It is available through an origin trial from Chrome 149 and through `chrome://flags/#enable-webmcp-testing` for local development. Chrome also notes that clients must visit the page before discovering its tools and that the design primarily targets local, human-in-the-loop browser workflows. [Chrome WebMCP overview](https://developer.chrome.com/docs/ai/webmcp)

Enabling the browser API is not the same as installing an agent. The user still needs Gemini in Chrome, an extension, an embedded page agent, a DevTools bridge, or another WebMCP-aware consumer. Chrome's own Model Context Tool Inspector is a test consumer and is explicitly separate from Gemini in Chrome. The broad browser-agent ecosystem is therefore a promising compatibility target, not a dependency the MVP should assume every visitor has.

For an arbitrary local agent daemon or CLI, use one of these instead:

- the existing direct WorldStream Client SDK;
- a future conventional MCP server/bridge that exposes a scoped Room membership; or
- a WebMCP-aware browser/extension controlled by that local agent.

Do not describe the WebMCP page URL as a generic MCP endpoint.

## What “bring your own agent” means here

WebMCP supports one valuable version of bring-your-own-agent:

> A person brings the browser agent they already use, opens a WorldStream Activity Client, and lets that agent choose and submit legal Actions through the same authorized page the person can see.

This can avoid requiring that person to give WorldStream an LLM API key. Usage remains governed by the person's agent product and account. WorldStream receives game Actions, not access to the person's subscription or model API.

It does **not** mean that WorldStream can rent unused ChatGPT or Codex subscription capacity, start the user's agent whenever a Room needs attention, or keep it running after the page closes. For unattended play, durable activation, scheduling, and agent-private memory, WorldStream still needs an external Runner or a directly connected agent client.

It also does not provide verified competition identity. A WebMCP player can honestly be called an **external browser agent**, but the platform cannot derive from WebMCP alone which model ran, which hidden instructions or tools it used, how much inference it consumed, or whether a human intervened. The Room's Actions, outcome, timing, and Replay remain authoritative; claims about the entrant's model and configuration are self-declared unless another execution or attestation layer verifies them.

Recommended competition treatment:

| Participation path | Suitable use | Verification posture |
| --- | --- | --- |
| WebMCP in a user's browser | Community play, human-agent collaboration, coached/exhibition matches | Authoritative game result; unverified model/configuration/assistance |
| Direct user-owned Client SDK | Custom and headless agents | Authoritative game result; configuration may still be self-declared |
| Platform-approved Runner | Controlled evaluations and ranked divisions | Can enforce and record the approved execution policy |

## Architectural fit with WorldStream

The accepted WorldStream model already permits this. An Activity Client is an independently executing browser, terminal, service, or agent-owned application using one scoped client contract. It can be Pack-aware and presentation-rich, but it owns no Room authority or legality. Direct agent clients remain peers rather than Studio extensions. See [CONTEXT.md](../CONTEXT.md) and [ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md).

WebMCP belongs inside the browser Activity Client Release:

```text
Person's conversation
        |
        v
ChatGPT/Codex/browser agent
        |
        | WebMCP discovery + tool calls
        v
Pack-specific browser Activity Client
  - human UI
  - WebMCP adapter
  - retained Room session
        |
        | authenticated HTTPS + WebSocket client contract
        v
WorldStream runtime
  - Membership and authorized Projection
  - Observation Stream, Cursor and Catch-up
  - Action admission and receipts
  - authoritative Pack execution and Replay
```

This preserves all accepted boundaries:

- The Activity Pack continues to define rules, Actions, visibility, phases, and Outcome.
- The kernel remains unaware of WebMCP, ChatGPT, Codex, and browser-specific tool APIs.
- `worldstreamctl` remains the primary Host Operator interface. The retained
  headless Supervisor provides only the bounded process and handoff operations
  still needed behind that CLI; neither contains Pack-specific player UI.
- The Activity Client translates authorized, current client state into agent-friendly tools.
- Unsupported browsers still get the ordinary human UI.
- Direct clients and Runners continue to use the underlying client contracts without opening a browser.

WebMCP can later be declared as a non-authoritative Client Surface capability, such as `webmcp.tools.v1`. It should not be added to Genesis, Authoritative Room State, Pack revision identity, or Replay.

## Recommended WorldStream WebMCP surface

Start with one Pack-specific game client, not a universal tool generator. A small shared package can manage registration, cancellation, response bounds, and conversion from a retained client session, while the Pack-specific Activity Client provides semantic names and schemas.

For a three-player iterated prisoner's dilemma client, a useful stable surface is:

| Tool | Behavior |
| --- | --- |
| `read_match` | Returns the compact authorized Projection, current phase/round, whether this membership may act, legal choices, cursor, and terminal result if present. Read-only. |
| `wait_for_turn` | Waits for a bounded interval after a supplied Cursor and returns the next authorized state or a timeout continuation. Read-only and abortable. |
| `choose_move` | Accepts `cooperate` or `defect`, but submits only when the matching current Action Offer exists. Returns the authoritative receipt plus the next compact authorized state. |
| `send_message` | Optional and present only if communication is part of this exact game revision. Submits a bounded message through a current Action Offer. |

The JavaScript callback must not construct authority. It should:

1. Read the latest retained authorized session state.
2. Match a current Action Offer and its schema digest.
3. Bind the submission to the current Room sequence.
4. Submit through the existing Activity Client session.
5. Return the authoritative receipt and a compact refreshed view.

WorldStream already carries the necessary stale-action fences in its client shape: `basedOnRoomSeq`, offer identity, schema digest, Action type, and payload. The browser client must reuse those checks rather than calling a special WebMCP-only mutation API.

Stable, semantic tools are preferable to dozens of dynamically changing low-level tools. Chrome recommends minimizing overlapping tools, using specific parameter types, treating static registration as the default, validating strictly in code, and returning useful errors. [Chrome WebMCP best practices](https://developer.chrome.com/docs/ai/webmcp/best-practices)

The GoogleChromeLabs WebMCP examples include a maze that exposes `look`, `move`, `pickup`, `drop`, and `use` so an agent can play through natural-language prompts. This is direct evidence that browser-agent game play is technically plausible; it is not evidence that the browser supplies multiplayer authority, durable recovery, or fair evaluation. Those are the parts WorldStream contributes. [GoogleChromeLabs WebMCP tools and demos](https://github.com/GoogleChromeLabs/webmcp-tools)

## Where the realtime stream remains

WebMCP should not replace the WebSocket. It is a tool-call surface, not WorldStream's Observation Stream.

While the page is open, its ordinary WorldStream client receives Projection Reset and Observation Frames over the existing authenticated connection. The human UI updates immediately. When the browser agent calls `read_match`, the tool returns a bounded snapshot of the latest **authorized** state. `wait_for_turn` may await one new client delivery for a short, bounded period and honor WebMCP's abort signal.

The current WebMCP draft describes each tool call as a Promise that eventually returns one serializable result. It does not define a durable server-push subscription into the model. The browser decides when to re-observe the page, and that timing is implementation-defined. Therefore a long game must use repeated `wait_for_turn`/`read_match` calls or a product-specific agent loop. [WebMCP tool execution](https://webmachinelearning.github.io/webmcp/#modelcontexttool-dictionary), [WebMCP page observations](https://webmachinelearning.github.io/webmcp/#page-observations)

If the page closes, WorldStream's logical Participant and Membership can outlive it. A later browser session reconnects and catches up through the normal Cursor/Projection Reset contract. WebMCP adds no durability of its own.

## Security requirements

WebMCP makes the cloud browser handoff more useful; it does not make it safe automatically. ADR 0017 currently permits only exact approved loopback origins and explicitly requires a separate remote HTTPS handoff design with registered redirect targets, origin-bound one-use authorization, short expiry, redirect handling, CORS policy, and credential retention. That work remains a prerequisite for a public Vercel/Fly URL. [ADR 0017 remote HTTPS boundary](adr/0017-separate-activity-clients-from-packs-and-studio.md#decision)

The WebMCP adapter should follow these rules:

1. **Bind normal web authentication to one Membership.** Redeem a one-use invitation and retain a secure, HttpOnly, origin-bound session. Do not accept a principal, Role, or Membership ID from a tool argument as authority.
2. **Never expose credentials in tool metadata, inputs, outputs, page text, or URLs.** The callback uses the page's existing session internally.
3. **Return only the current Membership's Projection and Observation data.** A tool must never expose Canonical History, another player's private Projection, server secrets, or Pack state merely because the calling entity is called an agent.
4. **Submit only currently offered Actions.** The kernel remains the final authority and must reject stale, illegal, malformed, duplicate, or over-budget requests.
5. **Treat player messages as hostile input.** Other agents can place prompt-injection instructions into chat or game content. Mark outputs containing user/agent-generated content with `untrustedContentHint`, bound their length, and keep rules and system guidance outside that content.
6. **Use `readOnlyHint` only for tools that cannot change state.** Do not label a move or message tool read-only.
7. **Keep exposure same-origin by default.** Do not use WebMCP's cross-origin `exposedTo` option unless a specific reviewed integration needs an exact trusted origin.
8. **Do not add external side effects to game moves.** Joining a simulated match and taking an in-game Action should not authorize purchases, messages, filesystem changes, or other real-world operations.

The WebMCP draft itself calls out prompt injection, tool-description poisoning, malicious tool output, privacy leakage, and the mismatch that can exist between a tool's description and its actual effect. Chrome recommends careful origin exposure and notes that even read-only tools may reveal user information. [WebMCP security considerations](https://webmachinelearning.github.io/webmcp/#security-and-privacy-considerations), [Chrome WebMCP tool security](https://developer.chrome.com/docs/ai/webmcp/secure-tools)

OpenAI separately says site tools inherit the page's signed-in state, request website access, and may require confirmation for sensitive actions. WorldStream must not assume those agent-product safeguards replace server-side authorization. [OpenAI site-tool safety model](https://help.openai.com/en/articles/20001423-using-site-tools-in-the-chatgpt-desktop-app)

## MVP slice

The smallest credible demonstration is **one WebMCP-capable Activity Client for one short turn-based Pack**. Do not first build a generic WebMCP schema compiler, a remote MCP service, or an autonomous browser farm.

### User experience

1. A host creates a short match and copies a Player invitation URL.
2. A second person opens the URL in ChatGPT desktop's built-in browser.
3. The person redeems the invitation and sees the same live match screen a human would see.
4. Site tools appear. The person prompts: “Read the rules and play this match. Use this strategy: cooperate first, then retaliate once after a betrayal.”
5. ChatGPT calls `read_match`, `wait_for_turn`, and `choose_move` until the Room is terminal or the agent asks the person for help.
6. Humans watch the live public Projection and open the authoritative Replay afterward.

This demonstrates the distinctive combination: **URL-level agent onboarding plus a shared authoritative multi-agent Room**, not merely a single-player JavaScript game controlled by a chat bot.

### Engineering order

1. Complete the accepted remote HTTPS handoff and scoped browser-session security contract.
2. Add a small WebMCP registration adapter to the selected standalone Activity Client.
3. Expose three stable tools: `read_match`, `wait_for_turn`, and one Pack-specific Action tool.
4. Keep all tool callbacks over the existing retained Activity Client session.
5. Add deterministic tests proving private state is omitted, stale offers fail, Actions use current Room sequence/schema/offer identity, reconnect catches up, and unsupported browsers retain the human UI.
6. Add agent evals for tool selection and multi-turn completion across several wording variants. Chrome's guidance explicitly recommends testing whether agents select, parameterize, and complete WebMCP tool flows. [Chrome WebMCP evals](https://developer.chrome.com/docs/ai/webmcp/evals)
7. Manually verify the deployed URL in a supported ChatGPT desktop account and Chrome's experimental environment. Record the exact client/browser versions used; do not claim universal support.

### MVP acceptance conditions

- A new user can open one invitation URL and reach the match without configuring an API key or MCP server.
- The browser agent can read only its authorized game state and can submit only a current offered Action.
- Closing and reopening the page does not lose authoritative Room progress.
- A human can use the same client when WebMCP is unavailable.
- The Replay records the resulting WorldStream Actions and outcome exactly as for any other client.
- Results identify the entrant as an external/unverified browser agent unless execution claims are separately verified.
- No WebMCP-specific code is added to the kernel, Studio, Pack executor, or canonical Room model.

## Challenge relevance

The linked challenge rules require a working live URL accessible in ChatGPT's in-app browser or Chrome with WebMCP enabled, a public licensed repository, a short demo video, and source that actually registers a WebMCP tool. An existing project is eligible only if it is meaningfully extended with WebMCP during the submission period and the new work is clearly distinguished from prior work. [Official Devpost rules](https://webmcp.devpost.com/rules)

The rules list the submission period as August 25 through **September 3, 2026 at 1:00 p.m. Pacific Time**. The architecture recommendation above should not be weakened to meet that deadline: a public participation URL requires the remote browser authorization work already deferred by ADR 0017. If a challenge submission is attempted, represent only what is actually deployed and keep timestamped commits separating the WebMCP extension from earlier WorldStream work.

## Final recommendation

Adopt WebMCP as a first-class **optional Activity Client capability** and use it for the community/exhibition onboarding path:

> Open the match link in your agentic browser. Give your agent a strategy. Watch it play in the shared Room.

Keep three lanes rather than forcing one interface to do every job:

1. WebMCP browser client for low-friction, human-present participation.
2. Direct Client SDK for custom, local, and headless agents.
3. Approved Runner for unattended and verifiable evaluation.

That split adds a compelling agent-native doorway without changing what WorldStream is: the authoritative realtime Room runtime underneath every client.
