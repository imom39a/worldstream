# Hosted local development

Use this setup to run the hosted MVP on one development computer. The setup
uses the same service boundaries as the hosted system. It uses three explicit
development substitutes:

- a local identity instead of GitHub OAuth;
- a deterministic fake OpenRouter service;
- a ready Hosted Gateway backend that accepts typed test operations.

The command identifies each substitute in its output. Each substitute refuses
production mode.

This setup does not start a Studio UI. The retained Controller directory is
named `studio` only because that is the current kernel storage contract.

## Prerequisites

Install these tools:

- Node.js `24.18.1`;
- pnpm `11.19.0`;
- Rust `1.97.1`;
- Docker Desktop;
- Supabase CLI;
- PostgreSQL `psql`.

Start Docker Desktop before you run the command.

## Start the stack

From the repository root, run:

```sh
pnpm hosted:dev
```

The first run can take several minutes because it builds the Rust binaries.
Later runs use the build cache.

The command does this work:

1. Installs the pinned pnpm dependencies.
2. Builds the Runtime, Controller, Hosted Gateway, and Platform BFF from source.
3. Starts the minimum local Supabase services.
4. Applies the committed migrations and Listing seed.
5. Adds the clearly named local development identity.
6. Initializes a dedicated WorldStream installation.
7. Reviews and imports the exact checked-in Activity Client declarations.
8. Starts the Runtime, Controller, product, Agent Heist client, Hosted Gateway,
   Platform BFF, and fake OpenRouter service.
9. Checks every service and runs one request through each development seam.

The ready output shows these default endpoints:

| Service | Address | Exposure |
| --- | --- | --- |
| Product | `http://127.0.0.1:5180/` | Local browser |
| Agent Heist asset server | `http://127.0.0.1:5173/agent-heist-v6/hosted/` | Product proxy only |
| Hosted Gateway | `http://127.0.0.1:8080/` | Local client |
| Platform BFF | `http://127.0.0.1:3000/` | Product proxy only |
| Fake OpenRouter | `http://127.0.0.1:8787/` | Local process only |
| Runtime | `127.0.0.1:9410` | Loopback only |
| Controller | `127.0.0.1:9420` | Loopback only |
| Supabase API | `http://127.0.0.1:55321` | Local process only |

The product proxies `/api/*` to the Platform BFF. Browser cookies and request
origins therefore use the product origin, as they do in the hosted design.
Start at the product catalog, not at the asset server. The catalog opens
`/agent-heist-v6/hosted/` on the product origin after it authorizes your entry.
The separate `/agent-heist-v6/` surface is for the standalone local-kernel
flow in [Getting started](getting-started.md); it does not use platform sign-in.

If an approved Activity Client Host is already on port `5173`, the command
checks its Agent Heist page and Controller policy. It reuses a compatible host.
It does not stop that user-owned process during cleanup.

Press `Ctrl-C` to stop the processes that the command started. The command
stops its dedicated Runtime and Controller. It preserves all retained data in
`.worldstream/hosted-dev`. If it started Supabase, it stops Supabase with its
normal retained backup. If Supabase was already running, it leaves it running.

## Run the bounded acceptance check

Use this command in local verification or automation:

```sh
pnpm hosted:dev:check
```

It starts the same topology, runs the end-to-end checks, and then stops the
processes that it owns. It does not delete retained data.

For the full canonical acceptance story, use:

```sh
pnpm hosted:acceptance:local
```

This command builds the native programs with `cargo build --locked --release`.
The CLI, Controller, Runtime, assignment MCP, Gateway, and approved House
executable are selected from that build. The log records the source revision,
checkout cleanliness, and selected native profile. It does not attest binary
digests or the running process tree. Normal `hosted:dev` and `hosted:dev:check`
commands still use debug builds.

For an explicit debug comparison, use:

```sh
pnpm hosted:acceptance:local --native-profile=debug
```

Only `debug` and `release` are accepted. A profile change does not waive an
existing executable approval; use a separate clean checkout for a comparison
if the retained setup pins a different executable. Do not delete retained data
to force a new approval.

Both profiles run the same prerequisites and story assertions, with the same
timeouts and local identity and provider substitutes. Debug-only diagnostic
hooks remain absent from release binaries. This is a local source/profile
check, not proof of the deployed Linux image or a real provider call.

Use these fast tests when you change only the development controls:

```sh
pnpm hosted:dev:test
pnpm hosted-platform:check
cargo test -p worldstream-hosted-gateway
```

## Build the hosted product

From the repository root, run:

```sh
pnpm install --frozen-lockfile
pnpm demos:build
```

The build prepares the Pack SDK and hosted contracts before it compiles the
Platform BFF. It then builds the Agent Heist Activity Client, builds the product,
and verifies the copied client against its reviewed release digest. It does not
need build artifacts from a previous development run.

Local builds read the source revision from Git. Vercel source builds use the
provider-supplied `VERCEL_GIT_COMMIT_SHA`, so they do not need a `.git` directory.
Enable Vercel system environment variables. Do not replace the provider's
revision with a hand-written production value.

## Development sign-in

The temporary sign-in endpoint is:

```text
POST /api/dev/sign-in
```

It accepts only same-origin JSON with this exact acknowledgement:

```json
{"mode":"visible-local-only"}
```

It creates the standard encrypted Platform session cookie. The BFF then uses
the normal session, CSRF, Supabase Account, and mutation checks. It replaces
only the GitHub OAuth exchange. The endpoint and its response include a visible
development-substitute label.

Do not add this route to a hosted deployment. The code rejects it when
`NODE_ENV=production` or `VERCEL_ENV=production`, when the canonical origin is
not loopback HTTP, or when the exact development acknowledgement is absent.

## Port rules

The Controller port `9420` and Agent Heist port `5173` are part of the reviewed
local Activity Client declaration. You cannot override them silently. A change
requires a new client build, immutable identity, declaration, and review.

You can override the other WorldStream ports before startup. For example:

```sh
WORLDSTREAM_HOSTED_PRODUCT_PORT=5280 \
WORLDSTREAM_HOSTED_GATEWAY_PORT=8180 \
pnpm hosted:dev
```

Available variables are:

- `WORLDSTREAM_HOSTED_RUNTIME_PORT`;
- `WORLDSTREAM_HOSTED_GATEWAY_PORT`;
- `WORLDSTREAM_HOSTED_PRODUCT_PORT`;
- `WORLDSTREAM_HOSTED_PLATFORM_PORT`;
- `WORLDSTREAM_HOSTED_FAKE_OPENROUTER_PORT`.

Supabase ports stay fixed in `supabase/config.toml`.

## Troubleshooting

If a port is in use, the command names the service and port. Stop the unrelated
process. Do not change ports `9420` or `5173` without a new client review.

If Supabase cannot start, verify that Docker Desktop is running. Then run:

```sh
supabase status
```

If a required command is missing, the startup error names that command. Install
it and retry.

If Runtime startup fails, inspect the dedicated retained logs:

```sh
target/debug/worldstreamctl \
  --config .worldstream/hosted-dev/worldstream.toml \
  server logs \
  --state-dir .worldstream/hosted-dev/studio \
  --controller 127.0.0.1:9420 \
  --tail 50
```

Do not delete `.worldstream/hosted-dev` as a general repair step. It contains
the retained local authority and Runtime data. Read the reported error and
repair the specific prerequisite.

### Local Heist browser verification on macOS

`node scripts/verify-standalone-heist.mjs` checks the separate local-kernel
Heist client against a real Controller and Runtime. Run it after Rust builds
finish, with ports `9410` and `9420` free. It uses an isolated browser and
temporary installation. It does not call a model provider.

New executable files can wait before they reach application code on macOS.
The check first runs `--help` on each exact copied executable, in sequence.
Each call has a 180-second limit; the whole preflight has a 480-second limit.
Its receipt records durations and exit results. This is help-only evidence,
not service-readiness evidence or a change to OS security policy. The normal
managed startup deadlines and ownership checks remain in force. A failed
preflight stops the check before Controller or Runtime startup. Do not disable
OS security controls to make it pass.
