# Assignment-bound MCP runbook

`worldstream-assignment-mcp` is a local line-delimited JSON-RPC MCP server over
stdio. It accepts one opaque assignment launch reference and resolves the
assignment's sealed authority inside owner-only Supervisor state.

## Start shape

Obtain a fresh reference from the Studio/Supervisor assignment flow, then make
your MCP host own this process:

```sh
target/debug/worldstream-assignment-mcp \
  --launch-reference <SUPERVISOR_ISSUED_REFERENCE> \
  --state-dir <OWNER_ONLY_STUDIO_STATE_DIR>
```

Never place a real reference in a committed config, shell transcript, issue,
screenshot, browser page, prompt, or support log. Do not launch the helper by
hand and paste its protocol output into a model; configure it as an MCP server.

## Tool catalog

### `worldstream.list_assigned_tasks`

Input: empty object. Lists only Tasks sealed into this assignment. Use it to
establish assignment context; it is not a Room search API.

### `worldstream.observe`

Input: empty object. Resumes the assigned Membership Observation Stream from its
durable Cursor. Process delivered frames in order and retain any external work
before acknowledging them.

### `worldstream.acknowledge`

```json
{ "through_frame_seq": 42 }
```

Advances only through a contiguous processed frame sequence. An ACK is not “I
saw the JSON”; it means the agent host has durably handled everything through
that sequence.

### `worldstream.list_current_action_offers`

Input: empty object. Returns exact offers, pinned payload schemas, and the Head
precondition from the current authorized view. Refresh after stale state or a
new observation.

### `worldstream.submit_action`

```json
{
  "operation_id": "<STABLE_ID_FOR_THIS_EXACT_ATTEMPT>",
  "offer_id": "<EXACT_LISTED_OFFER>",
  "precondition": {
    "room_seq": 12,
    "head_hash": "<EXACT_CURRENT_HEAD_HASH>"
  },
  "payload": {}
}
```

Reuse an `operation_id` only with the identical offer, precondition, and
payload. After an ambiguous response, repeat the identical request. After a
typed stale result, observe again, select a new current offer, and use a new
operation identity.

### `worldstream.next_activation`

Input: empty object. Acquires the next Activation or resumes the exact retained
lease. A typed no-work result is ordinary idle state, not a crash.

### `worldstream.complete_activation`

```json
{
  "activation_cursor": 9,
  "lease_generation": 2,
  "context_hash": "<EXACT_CONTEXT_HASH>",
  "disposition": "handled"
}
```

Disposition is closed: `handled`, `declined`, or `failed`. Completion is bound
to the exact cursor, generation, and context hash.

## Safe turn algorithm

```text
next_activation
  ├─ no work → bounded backoff, try later
  └─ lease
      → observe
      → list_current_action_offers
      → policy selects an exact offer or declines
      → submit_action (if selected)
      → retain the Action result
      → acknowledge processed frames
      → complete_activation with exact lease fields
```

ACK must occur after durable processing/receipt retention. Completion must not
invent a fourth disposition. A helper restart resumes the operation ledgers and
the daemon's Activation generation fences.

## Troubleshooting

| Symptom | Meaning / next action |
| --- | --- |
| launch reference unavailable | assignment missing, revoked, expired, or wrong state directory; return to Studio setup |
| no assigned Tasks | this helper is not a general discovery client; verify exact profile assignment |
| stale Head / offer | observe again and select a new offer; do not reuse the changed Action identity |
| already completed | advance to the next Activation; do not resubmit the old Action |
| lease expired/generation changed | reacquire; never complete with stale fields |
| invalid arguments | fix the exact schema; do not add unknown fields |

Source: executable [MCP tool schemas](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-studio-supervisor/src/assignment_mcp.rs#L2890)
and [Activation contract](https://github.com/imom39a/worldstream/blob/main/docs/observation-and-activation.md#activation-contract).
