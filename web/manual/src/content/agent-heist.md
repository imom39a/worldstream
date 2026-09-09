# Agent Heist: complete reference Activity

Agent Heist is the full v0.1 reference story: one human Navigator, multiple Agent
Participants, scoped secrets, exact offers, a six-phase timer machine, durable
attention/Activation, restart recovery, a committed Outcome, and Replay parity.

## Domain shape

The pack uses declared seats and phases rather than letting a model invent game
structure. The Activity State contains only canonical game facts. Private agent
memory and provider prompts remain outside the Room.

The six phase transitions are timer-governed and generation-fenced. Duplicate,
obsolete, early, or wrong-generation timer firings cannot advance state. The
Outcome applies deterministic majority and five-check scoring rules, including
missing or split votes.

## Privacy model

- Public/operator views never contain sealed commitments or fixture truth.
- Each participant sees only its own private exchange/commitment information.
- Historical participant views preserve the original authorization boundary.
- Final reveal is structurally separate and appears only at the terminal rule
  boundary.
- Action Offers expose what the viewer may propose now without revealing why a
  hidden choice is legal or valuable.

## Offline story

Use this first; it needs no daemon, browser, database, or model:

```sh
uv run --project sdk/python --python 3.14.7 \
  python examples/heist/run_story.py --self-test

uv run --project sdk/python --python 3.14.7 \
  python -m unittest discover -s examples/heist -p 'test_*.py'
```

This proves deterministic story/parity fixtures. It is not live service
evidence.

For the visual version of the recorded story, run `pnpm demos:dev` and open
`http://127.0.0.1:5180/demos/agent-heist/`. The demo reuses the Activity
Client's presentation through a recorded adapter, but it has no Room authority
or network connection. A live CLI/Controller handoff selects the exact approved
client binding. The current Heist 0.3 binding opens
`http://127.0.0.1:5173/agent-heist-v6/`; retained older bindings can still
select their original route, including `/agent-heist-v3/`.

## Complete local MVP gate

The real gate owns disposable state and launches the production daemon,
Supervisor, Agent Heist Activity Client, browser, WebSocket, and assignment MCP
boundaries. Build the required artifacts first:

```sh
cargo build --locked -p worldstream-server --bin worldstreamd
cargo build --locked -p worldstream-studio-supervisor \
  --bin worldstream-studio-supervisor \
  --bin worldstream-assignment-mcp
pnpm ui:build
uv sync --project sdk/python --locked --python 3.14.7
```

Install the repository-pinned Chrome-for-Testing identity and export the seven
`WORLDSTREAM_BROWSER_*` identity variables exactly as documented in
`docs/mvp-agent-heist-acceptance.md`. On macOS, the checked source bootstrap
validates the pinned binary, version, archive URL, hashes, and byte sizes:

```sh
scripts/macos-source-quickstart.sh --help
```

Do not substitute an arbitrary installed Chrome build: the browser identity is
part of the acceptance evidence. For the current macOS arm64 pin, validate the
binary with the full `scripts/macos-source-quickstart.sh` command in the
acceptance contract, then export:

```sh
export WORLDSTREAM_BROWSER_BINARY=/absolute/path/to/chrome-headless-shell
export WORLDSTREAM_BROWSER_VERSION=152.0.7977.54
export WORLDSTREAM_BROWSER_SHA256=4e0c165ef2f0d7265fb1e6b3df2d03d1d6581fb72cdfcebeac19c09760571df6
export WORLDSTREAM_BROWSER_SIZE_BYTES=167333040
export WORLDSTREAM_BROWSER_ARCHIVE_URL=https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-arm64/chrome-headless-shell-mac-arm64.zip
export WORLDSTREAM_BROWSER_ARCHIVE_SHA256=ef5d61434f13d9d2d9bdc7c9ab4bff92225979e196458cf846640862b25f127d
export WORLDSTREAM_BROWSER_ARCHIVE_SIZE_BYTES=98034515
```

The acceptance contract carries the separately closed x86_64 identity. Once
the seven variables are exported, run the actual live story:

```sh
uv run --project sdk/python --python 3.14.7 python \
  examples/heist/mvp_live/run_mvp_acceptance.py \
  --live \
  --report target/agent-heist-mvp-acceptance.json
```

Then revalidate the retained bounded report independently:

```sh
uv run --project sdk/python --python 3.14.7 python \
  examples/heist/mvp_live/run_mvp_acceptance.py \
  --report target/agent-heist-mvp-acceptance.json
```

Exit code `2` and a single `blocked:<reason>` mean a required real boundary is
unavailable; that is not acceptance. Use `--keep-artifacts` only for local
diagnosis because the otherwise temporary evidence directory is sensitive.

The gate is designed to prove:

1. reviewed CLI setup creates exactly one Room;
2. human and external-agent seats are provisioned separately;
3. Lobby remains gated on declared readiness and explicit launch;
4. the agent uses only assigned MCP tools and exact Action Offers;
5. helper restart reclaims work without duplicate Action/completion;
6. the story reaches a committed visible Outcome;
7. Replay reproduces the exact Outcome and final Transition lineage;
8. browser/MCP/log evidence contains no raw authority or routing secrets.

## Where to learn from it

| Topic | Source |
| --- | --- |
| pack reducer, phases, privacy | `crates/worldstream-core/src/agent_heist.rs` |
| lobby/readiness rules | `agent_heist_lobby.rs` |
| production registry and goldens | `agent_heist_registry.rs` |
| retained corpus | `examples/heist/retained_corpus.json` |
| deterministic story | `examples/heist/story.py` |
| MVP live gate | `examples/heist/mvp_live/` |

Source: [Activity Pack Agent Heist section](https://github.com/imom39a/worldstream/blob/main/docs/activity-packs.md#reference-activity-a-agent-heist),
[MVP acceptance contract](https://github.com/imom39a/worldstream/blob/main/docs/mvp-agent-heist-acceptance.md),
and [Heist examples](https://github.com/imom39a/worldstream/tree/main/examples/heist).
