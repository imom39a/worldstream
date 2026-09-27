# Examples

These are preserved applications and conformance workloads built around the
WorldStream kernel. They illustrate particular design choices; they are not
maintained products or evidence of autonomous-agent performance.

Start with Counter to see an end-to-end Room without a model provider. For Pack
authoring, start with the late-join reference or Tower of Hanoi.

| Example | What it demonstrates | Entry point |
| --- | --- | --- |
| Counter | Typed Actions, scoped views, reconnect, and restart | [Runner](counter/README.md) |
| Late join | Bounded context for a newly attached agent | [Pack](packs/late-join-reference/README.md) |
| Tower of Hanoi | Legal moves, attribution, and participant acceptance | [Pack](packs/tower-of-hanoi/README.md), [runner](tower_of_hanoi/README.md) |
| Agent Heist | Hidden information, timers, scoped observations, Replay | [Runner](heist/README.md), [browser](clients/agent-heist-web/README.md) |
| Midnight Archive | Human/agent cooperation and companion plans | [Pack](packs/midnight-archive/README.md), [runner](midnight_archive/README.md) |
| Negotiate | Multi-party approval, deadlines, external evidence | [Pack](packs/negotiate/README.md), [integration](negotiate/README.md) |
| Agent Swarm | Goals, work ownership, contributions, accepted results | [Application](agent-swarm/README.md), [Pack](packs/agent-swarm/README.md) |
| Minesweeper | Application-side cooperative solver experiment | [Runner](minesweeper/README.md) |

[`clients/`](clients/README.md) contains independent browser clients and the
Pack-neutral Inspector. [`room-setup/`](room-setup/README.md) contains reviewable
setup inputs, and [`cli_activity/`](cli_activity/) contains terminal clients.

## TypeScript packs

From the repository root, with the pinned Node and pnpm versions installed:

```sh
pnpm install --frozen-lockfile
pnpm pack:build
pnpm --dir examples/packs/tower-of-hanoi pack:check
pnpm --dir examples/packs/tower-of-hanoi pack:test
```

Each Pack README describes building and proving its `.wspack`. A new build has
its own exact digest and needs explicit operator approval before installation.
Checked-in digest-named bundles and golden inputs are retained for reproducible
tests and historical Replay; rebuilding source does not replace those identities.

## Rust applications

The example crates are outside the default Cargo member set. Select one with
`cargo test --locked -p worldstream-agent-swarm`, or include all examples with
`cargo test --workspace --locked`. Live provider execution requires explicit
configuration and credentials; default checks use deterministic fixtures.
