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

## Complete local MVP gate

Read `docs/mvp-agent-heist-acceptance.md` before running the live harness. It
requires built daemon, Supervisor, Console, assignment MCP helper, local
authority state, and browser support. The checked-in entry point is:

```sh
uv run --project sdk/python --python 3.14.7 \
  python examples/heist/mvp_live/run_mvp_acceptance.py --help
```

The gate is designed to prove:

1. reviewed Studio draft creates exactly one Room;
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
