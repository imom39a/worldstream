# Agent Swarm

Agent Swarm is a preserved application experiment built on WorldStream. One
goal occupies one Room. Its Activity Pack records work ownership, contributions,
reviews, and accepted results; the application owns provider execution and
scheduling outside the kernel.

The default application build runs deterministic fixtures. It does not call an
AI provider or prove autonomous delivery. Live provider adapters were exploratory
and are not maintained or qualified against current provider releases.

From the repository root:

```sh
cargo test --locked -p worldstream-agent-swarm
cargo run --locked -p worldstream-agent-swarm -- --help
```

The optional `managed-local-runtime` feature exercises the local Controller and
Runtime integration:

```sh
cargo test --locked -p worldstream-agent-swarm --features managed-local-runtime
```

Read the [application reference](app/README.md), [Pack](../packs/agent-swarm/README.md),
and [design notes](docs/design.md) for the retained implementation. The design explains the retained authority boundaries and result lifecycle.
