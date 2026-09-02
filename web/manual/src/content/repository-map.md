# Repository map

```text
agent-streamer/
├── CONTEXT.md                 canonical domain vocabulary
├── compatibility.toml/json    release/build compatibility contract and mirror
├── config/                    local non-secret runtime and Activity Client configuration
├── clients/                   first-party standalone Activity Clients
├── crates/                    Rust kernel, client contracts, adapters, server, Supervisor
├── docs/                      normative architecture, protocol, ADRs, runbooks
├── examples/                  Counter and Agent Heist fixtures/acceptance
├── sdk/python/                public asynchronous participant client
├── sdk/typescript-client/     internal browser Room client seam
├── scripts/                   gates, smoke tests, local launch, release tools
├── tests/fixtures/            compatibility/storage/conformance fixtures
├── web/console/               first-party Client Host and compatibility UI
├── web/studio/                local operator portal
├── web/manual/                this static developer manual
└── xtask/                     deterministic repository maintenance commands
```

## Change routing

| Change | Start in | Also inspect |
| --- | --- | --- |
| domain vocabulary | `CONTEXT.md` | affected ADRs and all public names |
| canonical Room behavior | `worldstream-core` | architecture, both adapters, conformance, protocol |
| wire shape | `worldstream-protocol` | server, SDK, Activity Clients, MCP, compatibility |
| SQLite behavior | `worldstream-sqlite` | shared conformance and backup/transfer |
| PostgreSQL behavior | `worldstream-postgres` | migrations, native restore, shared conformance |
| daemon/operator API | `worldstream-server` | protocol docs, Studio proxies, SDK |
| Studio workflow | `web/studio` + Supervisor module | Studio ADR/docs and browser-safe DTO tests |
| Activity Client contract or binding | `worldstream-activity-client` + Supervisor binding store | ADR 0017, `config/activity-clients`, clients, Studio launcher |
| Activity Pack | `worldstream-core` registry/pack | compatibility, retained corpus, examples, all views |
| agent contract | assignment MCP + activation core/protocol | Studio setup, Runner attention, managed host |
| release contract | compatibility + packaging/gates | generated mirror, workflows, docs |
| developer manual | `web/manual` | primary docs |

## Normative vs explanatory

- `docs/adr/`, requirements, architecture, protocol, security, and domain
  context govern behavior.
- examples prove only the boundary they actually execute.
- this manual curates and teaches; it should link to the owning source for
  every important claim.

## Generated and local state

Do not commit `target/`, `node_modules/`, web `dist/`, `.worldstream/`, runtime
credentials, provider tokens, acceptance artifacts containing private evidence,
or temporary backup/restore output.

Source: [workspace Cargo manifest](https://github.com/imom39a/worldstream/blob/main/Cargo.toml),
[documentation index](https://github.com/imom39a/worldstream/blob/main/README.md#documentation),
and [architecture target layout](https://github.com/imom39a/worldstream/blob/main/docs/architecture.md#target-repository-layout).
