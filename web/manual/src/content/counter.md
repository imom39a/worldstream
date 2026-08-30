# Counter: the smallest working pack

This is the first complete WorldStream story. It uses Counter only: one human
Participant, one managed Agent Participant, a deterministic local provider, and
one Room. You will see an acknowledged private Action cause one managed
`increment`, then verify that the resulting Canonical History replays without
starting a Runner or contacting a model.

The local provider is a demonstration fixture. It always makes the prescribed
decision for this guide; it is not a model-provider recommendation.

## What you need

Run the commands from the repository root. The story creates its own protected
temporary state and uses local loopback ports. It does not ask you to delete,
reset, or modify existing WorldStream data.

| Tool | Version |
| --- | --- |
| Rust | 1.97.1 |
| Python | 3.14.7 |
| Node | 24.18.1 |
| pnpm | 11.19.0 |

Install the repository dependencies once:

```sh
pnpm install --frozen-lockfile
uv sync --project sdk/python --locked --python 3.14.7
```

## Start the complete local story

Use the one entrypoint:

```sh
DEMO_ROOT="$(mktemp -d)"
chmod 700 "$DEMO_ROOT"
pnpm counter:studio --state-dir "$DEMO_ROOT/state" --control-file "$DEMO_ROOT/control.json"
```

It coordinates these real local components:

```text
Studio browser ── Supervisor ── worldstreamd
      │                 │
      │                 ├── managed Agent Host ── assignment MCP helper
      │                 └── deterministic loopback provider
      └── Participant Console (protected cookie handoff)
```

In its normal mode, the command builds the local daemon, Supervisor binaries,
Studio, and Console, then starts Supervisor (`9410`), daemon (`9420`), Studio
(`5174`), Participant Console (`5173`), and provider (`19431`). It prints the
local addresses and keeps the fixture processes running. Leave that terminal
open. Its state directory is separate from your normal local state, so stopping
the story only stops its own processes.

For a fast rerun after a successful build, use:

```sh
pnpm counter:studio --skip-build
```

`--state-dir PATH` requires a fresh owner-only directory. Reusing one requires
the explicit `--retain-state` flag; the entrypoint never clears it for you.

Continue with the interactive Studio walkthrough below. It is the recommended
first experience. The automated browser acceptance near the end of this page
is an alternative verification path: do not run it before the walkthrough
against this same state, because it deliberately creates the same immutable
Profile, template, draft, and Room identities for repeatability.

## Follow the story in Studio

### 1. Publish the reusable template

Open Studio at the address printed by the entrypoint. The fixture installs only
the exact Runner Template and one named, configured credential. It does **not**
create an Agent Profile, a Room draft, a Task Template, a Room, or participant
authority. Complete the following values in **Build**.

#### Publish the managed Agent Profile

Under **Agent Profiles**, enter these exact values, then choose **Publish
immutable revision**:

| Studio field | Value |
| --- | --- |
| Profile ID | `counter-managed` |
| Execution kind | `Managed reference host` |
| Host contract revision | `v1` |
| Approved Runner Template | `Counter deterministic managed reference · v1` (`counter-managed-reference` / `v1`) |
| Provider loopback address | `127.0.0.1:19431` |
| Model ID | `counter-deterministic` |
| Owner-installed provider credential | `Counter deterministic loopback · configured` (`local-openai`) |
| Revision | `v1` |
| Display name | `Counter managed reference` |
| Non-secret configuration | `{}` |

The credential selector names a Supervisor-held credential; do not paste a
credential value into any Studio field. Wait for **Published exact revision
v1** before continuing.

#### Make the reviewed source draft that the Task Template needs

The empty Room draft is named `new-room`. It is the required **source reviewed
draft** for the immutable template; publishing a profile alone is not enough.

1. In **Plan a new Room**, open **1. Activity**. On the Counter card for
   `worldstream.counter` **4.0.0** (digest
   `blake3:2a1d2e493cbaffa3803724dfef42d35c167db2237aa9b1e113dfb79679e9c052`),
   choose **Use exact revision**.
2. Open **2. Configuration** and set `initial_value` to `0` and `maximum_value`
   to `3`.
3. Open **3. Seats** and choose **Use declared seat policy**. Counter 4 permits
   eight `counter` seats. Configure only the first two and leave seats 3–8
   blank with **Required for this Task** unchecked:

   | Seat | Required and participant | Exact managed binding |
   | --- | --- | --- |
   | `counter 1` | Check **Required for this Task**; label it `Human Counter`; enter principal ID `01ARZ3NDEKTSV4RRFFQ69G5FB0`; choose `Human`. | None |
   | `counter 2` | Check **Required for this Task**; label it `Managed Counter`; enter principal ID `01ARZ3NDEKTSV4RRFFQ69G5FB1`; choose `Agent`. | Choose `Managed agent`, `Counter managed reference · v1`, and `Counter deterministic managed reference · v1`. |

4. Open **4. Readiness**. It must list the two populated seats as required and
   the remaining six as optional and unfilled. Then open **5. Review**, check
   **Include read-only operator view**, and choose **Save Review**. Wait until
   the button reads **Review saved**.

The source review still has no Room and no authority. It is a frozen planning
snapshot used to publish a reusable template.

#### Publish and instantiate the Task Template

Under **Task Templates**, use this reviewed `new-room` source and enter:

| Field | Value |
| --- | --- |
| Template ID | `counter4-managed-task` |
| New revision | `v1` |
| Display name | `Counter 4 managed task` |

Choose **Publish immutable revision**. Studio shows `Source draft: new-room`;
that line confirms the reviewed-source dependency. Set **New independent draft
ID** to `counter4-demo` and choose **Create editable draft**. The confirmation
must say that no Room was created.

### 2. Review the independent Task draft and provision it

The new `counter4-demo` draft is an independent copy of the exact template. Go
to **5. Review**, confirm the Counter 4 configuration, the two required seats,
the exact Profile and Runner bindings, six empty optional seats, and the
read-only operator view. Choose **Save Review** again, then **Create from
reviewed draft**. Confirm the resulting Task setup shows both populated seats,
the selected Agent Profile, and the selected Runner Template. Use **Provision
participant access** to create the distinct participant authorities.

Review creates exactly one Counter Room. The setup screen then shows **Room
active** at Genesis. Counter has no separate launch step in this story: the
first authoritative Room Head is sequence `0`.

| Room sequence | Counter value | What happened |
| ---: | ---: | --- |
| 0 | 0 | Genesis; the reviewed setup is ready |

### 3. Open the protected human Participant Console

On the human seat, choose **Open Participant View ↗**. Studio opens the
separate Participant Console through a short-lived fragment handoff. The
fragment is immediately exchanged for a local HttpOnly cookie and removed from
the browser URL.

You do not copy a Room identifier, Membership identifier, or bearer credential.
The Console shows **Participant session** and **Authorized Room projection**.
Those are membership-authorized views; they are not a second Room runtime.

### 4. Submit `private_ack`

In the Console, use the offered **Submit private_ack** Action with the shown
JSON payload. The Action is admitted against the exact current Room Head.

The Room moves to sequence `1`. The human's authorized projection changes, and
Studio displays a bounded agent-attention status. That status means the Activity
Pack has requested that the managed Agent Participant be considered for one
Invocation. It is not proof that a model has already run.

| Room sequence | Counter value | What happened |
| ---: | ---: | --- |
| 1 | 0 | Human `private_ack`; managed-agent attention is available |

### 5. Watch the managed turn converge

Choose **Start managed host** when Studio presents that readiness operation.
The managed Agent Host consumes the Activation through the assignment MCP
helper. The deterministic provider selects the currently offered `increment`
Action. The Agent Host submits that Action through its own bounded Membership
authority; it does not use the human Console cookie.

Studio's Runner status returns to a healthy or idle state after the Invocation.
The Participant Console performs bounded, one-at-a-time refreshes from its last
received frame. It does not advance the Membership Cursor itself, and it does
not issue overlapping request storms while it waits for the managed turn.

Both Studio and the Console then show the same Counter result:

| Room sequence | Counter value | What happened |
| ---: | ---: | --- |
| 2 | 2 | One managed `increment` committed |

If the Console briefly says it is reconnecting, use **Reconnect**. It resumes
from the retained cookie and last received frame, rather than creating a new
participant authority or repeating an Action.

## Restart recovery

Use Studio's managed-host controls; the entrypoint intentionally does not
restart a host for you.

**Mid-turn recovery.** Immediately after **Start managed host**, while the
managed-seat card reports an Activation state of **Waiting** or **Leased**,
choose **Stop managed host**. Wait for the same card to report **Attention** or
**Unavailable**, then choose **Retry managed host**. Studio should return the
managed host to its bounded work and eventually show an idle/healthy result;
the Console and operator view converge to sequence `2`, `Counter value: 2`.
The retained activation cursor and lease are resumed, so this recovery commits
one increment, not two.

**Completed-turn restart.** After the table below already shows sequence `2`
and value `2`, choose **Stop managed host**, wait for its stopped/unavailable
state, then choose **Retry managed host** when Studio offers it. Check that the
Room Head is still sequence `2`, `Counter value: 2` remains visible, and the
managed-seat card has no new waiting or leased work. A completed activation is
not invoked again and the deterministic provider is not contacted again.

In both cases, the human Console reconnects from its retained session without
leaking a credential into its URL or page.

The repeated Action identity returns its original result; a different request
with that identity is rejected. A request made against an old Room Head is
rejected until the client synchronizes again.

## Verify read-only Replay

In the Participant Console, choose **Verify Replay at current sequence** under
**Authorized Replay**. This asks the local Supervisor to call the daemon's
existing member-authorized Replay API with the Room address and bearer retained
server-side. The browser sends only the cookie and `{ "at_room_seq": 2 }`.

The Console displays **Verified Canonical History at sequence 2**, the
authorized historical projection (`value: 2`), its projection hash, the
authoritative-state hash, and the lineage hash. They must agree with the
committed sequence-2 Room Head shown in the live story.

Replay reconstructs the immutable Canonical History with the exact retained
Counter executor. It is read-only: it does not invoke the managed Agent Host,
the assignment MCP helper, the Runner, or the deterministic provider.

## Recovery guide

| What you see | What to do |
| --- | --- |
| Studio cannot reach the Supervisor | Keep `pnpm counter:studio` running and reload Studio. To resume the retained `counter4-demo` draft or its Task setup, use **Open existing draft counter4-demo** on its retained template usage instead of creating another draft. The fixture owns its local processes. |
| Participant Console says reconnect | Choose **Reconnect**. Do not reopen the handoff URL or copy its old fragment. |
| No managed turn appears | Check the bounded attention and Runner status in Studio. The deterministic provider should be labeled as a local fixture. |
| Replay is unavailable | The existing human credential may predate Replay scope provisioning. Return to Task setup and create a new reviewed Counter draft; old credentials are intentionally not upgraded. |
| A value differs from `2` at sequence `2` | Stop the fixture, keep any diagnostic output, and rerun the complete story. Do not delete existing WorldStream data. |

## Verify the implementation

The interactive walkthrough above is the product story. The checked-in browser
acceptance repeats that story automatically for implementation verification.
Run it with a fresh coordinator state, not with the state used for the manual
walkthrough.

### Run the automated browser acceptance

In terminal A, start a fresh coordinator. The small marker file lets terminal B
read the exact temporary path without relying on a shell variable from another
terminal:

```sh
mkdir -p target
AUTO_ROOT="$(mktemp -d)"
chmod 700 "$AUTO_ROOT"
printf '%s\n' "$AUTO_ROOT" > target/counter-studio-demo-root
pnpm counter:studio \
  --state-dir "$AUTO_ROOT/state" \
  --control-file "$AUTO_ROOT/control.json"
```

Keep terminal A running. In terminal B, install the local Playwright Chromium
once and run the public fixture:

```sh
AUTO_ROOT="$(cat target/counter-studio-demo-root)"
pnpm exec playwright install chromium
pnpm counter:studio:browser \
  --adapter playwright \
  --fixture examples/counter/counter_studio_browser_fixture.json \
  --studio-url http://127.0.0.1:5174 \
  --control-file "$AUTO_ROOT/control.json" \
  --state-dir "$AUTO_ROOT/state/studio" \
  --draft-name counter4-demo \
  --provider-status-url http://127.0.0.1:19431 \
  --artifact-dir "$AUTO_ROOT/browser-evidence" \
  --capture-witness "$AUTO_ROOT/browser-evidence/before-replay.json" \
  --check-witness "$AUTO_ROOT/browser-evidence/after-replay.json"
```

The automation uses the same Studio controls as the guide, stops and retries
the managed host at the provider boundary, performs a completed-host restart,
and verifies Replay. Its owner-only report checks DOM and request-URL
disclosure, retained context canaries, exactly one committed `increment`,
recovery, and Replay without printing protected values. When it finishes,
press **Ctrl+C** in terminal A to stop only the processes owned by this fixture.

### Run focused checks

These checks are useful when changing the coordinator or either web app:

```sh
uv run --project sdk/python --python 3.14.7 pytest -q examples/counter/test_run_studio_demo.py
pnpm --dir web/console test
pnpm --dir web/studio test
```

The older low-level Counter v2 harness remains a regression check for the
daemon protocol and retained replay executor:

```sh
uv run --project sdk/python --python 3.14.7 python examples/counter/run_live_acceptance.py
```

It is not the setup path taught above.
