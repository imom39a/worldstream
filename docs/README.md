# Documentation

Start with [Getting started](getting-started.md) to run the kernel, or
[Architecture](architecture.md) to examine its design. The project is preserved
as a kernel and examples; [ADR 0044](adr/0044-preserve-kernel-and-examples.md)
records the retirement of the product and hosted deployment plans.

| Subject | Reference |
| --- | --- |
| Canonical terminology | [Domain glossary](../CONTEXT.md) |
| Architecture and implementation map | [Architecture](architecture.md) |
| Sessions, Actions, observations, and Replay | [Wire protocol](protocol.md) |
| Cursors, resets, attention, and leases | [Observation and activation](observation-and-activation.md) |
| Deterministic rule contract and portable bundles | [Activity Packs](activity-packs.md) |
| Independent browser and terminal applications | [Activity Clients](activity-clients.md) |
| CLI and local installation | [CLI reference](cli-reference.md), [initialization inputs](cli-initialization-inputs.md) |
| Backup, restore, and offline transfer | [Storage operations](operator-storage.md) |
| Authority and privacy | [Security model](security.md) |
| State and bounded invocation context | [Context and memory](context-and-memory.md) |
| Build and test commands | [Verification](gates.md) |
| Application code | [Examples](../examples/README.md) |
| Design rationale | [Architecture decisions](adr/README.md) |

Detailed protocol documents retain historical wire and contract names. Their
references to release versions describe implemented contracts, not a current
product roadmap or support promise.

## Writing conventions

Use ASD-STE100 principles for explanatory text. Use short sentences and active
voice. Use one technical name for each concept. Give one action in each
instruction. Avoid slogans and unsupported claims. Keep code identifiers and
protocol values exact.
