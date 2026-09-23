# Codex app-server adapter status

Checked on 2026-09-22 against installed `codex-cli 0.149.0` on macOS arm64.

The adapter now supports one bidirectional app-server turn inside the existing
owned process guard. This replaces the new-invocation `codex exec --json`
transport, which did not report the effective model and reasoning effort the
Swarm contract requires. Existing stored exec transcripts remain decodable.
Protocol support alone does **not** admit Codex to a live Swarm. A new explicit
invocation-contract binding also prevents existing v2 exec qualification from
authorizing app-server on the strength of an unchanged CLI digest. Current v2
records cannot establish that binding; a future receipt/schema revision must
derive it from native evidence.

The guard initializes the connection, reads account type without retaining
account details, requires provider-managed ChatGPT sign-in, starts or resumes
the exact private thread, and checks the provider's `model`, `modelProvider`,
`reasoningEffort`, working directory, approval policy, and write sandbox before
submitting the prompt. A fresh thread explicitly disables provider model
fallback. Resume has no such field in the installed schema, so any reported
substitution is rejected before the turn. The turn inherits those verified
settings without a second, unreported override. Configuration-change
notifications, terminal status, and thread/turn identities are checked.

The resumable thread ID and the independently reported session-tree ID remain
distinct. The driver never grants interactive approval, serves a dynamic tool,
or refreshes external auth tokens. Unexpected requests, delegation/MCP events,
malformed or oversized protocol output, missing reports, and failed or
interrupted turns fail closed. Account details are excluded from the retained
transcript. The same process guard terminates and reaps the provider group on
turn completion, protocol failure, cancellation, or loss of its owner.

These checks are detection and transport controls. They are not proof that an
unqualified provider cannot execute a disallowed tool before reporting an
event, or that a provider's configuration report accurately reflects OS
confinement. Native qualification must establish those properties separately.

## Local discovery, without model execution

A bounded `initialize` → `account/read` → `model/list` discovery exchange succeeded.
The account type was `chatgpt`; that discovery sent no prompt or model turn. The non-hidden
model catalog returned:

| Model identifier | Explicit efforts |
| --- | --- |
| `gpt-5.6-sol` | low, medium, high, xhigh, max, ultra |
| `gpt-5.6-terra` | low, medium, high, xhigh, max, ultra |
| `gpt-5.6-luna` | low, medium, high, xhigh, max |
| `gpt-5.5` | low, medium, high, xhigh |

This catalog is account- and time-specific. None of these identifiers contains
a date-pinned snapshot. The user subsequently selected `gpt-5.6-sol` with
`medium` effort and acknowledged the moving alias. The genuine synthetic
[local probe evidence](evidence/agent-swarm-codex-local-probe/README.md) records
fresh and resumed turns using exactly that selection. It is not an integrated
Swarm run or installed provider qualification.

## Expiring local development admission

The installed experimental schema exposes `permissions` on thread start/resume
and `activePermissionProfile`, `instructionSources` and `runtimeWorkspaceRoots`
on the response. These permit a named filesystem profile that is narrower than
the legacy `readOnly` report. The native trial uses a temporary owner-only
Codex configuration, provider-managed ChatGPT login, no ambient MCP servers or
project instructions, disabled tool features and denied network access. Model
workers return source as bounded `artifact.inline_text`; the coordinator
publishes the exact bytes immutably and supplies digest-verified Room artifacts
as context for later workers.

`--local-codex-evidence` admits only the exact measured executable and guard,
selected model/effort, working directory, configuration hash and read-only
profile until its explicit expiry. Each launch rechecks scope and configuration.
The loader requires hash-bound native isolation, fresh/resumed turn,
cancellation and owner-loss receipts, including process censuses. Deleting the
temporary profile revokes this local admission. It never emits an installable
release qualification record. The explicit switch is a trusted local evaluation
input, not a signature or attestation from an independent security authority.

Before admission, retain genuine native receipts for both selected settings,
provider-reported model/effort, fresh and resumed sessions, cancellation and
owner death, in-root access, denied out-of-root access, denied unallowed tools,
and an integrated reviewed result. Do not derive qualification from unit-test
fixtures, protocol schemas, arguments, or synthetic `pass` booleans.

The release qualification aggregator still requires its complete native matrix
before emitting installable provider qualification. One expiring local trial
cannot satisfy that release gate or establish other model selections, operating
systems, tools, write access or cross-run reliability.

## Sources and verification

The installed CLI generated stable and experimental schemas with
`codex app-server generate-json-schema [--experimental] --out <temporary-dir>`.
Relevant types are `ThreadStartParams`, `ThreadResumeParams`,
`ThreadStartResponse`, `ThreadResumeResponse`, `TurnStartParams`,
`ThreadSettingsUpdatedNotification`, and `SandboxPolicy`. Installed schemas
take precedence over examples for newer CLI versions.

The [official app-server documentation](https://learn.chatgpt.com/docs/app-server)
describes the initialize/thread/turn lifecycle, schema generation, model-list
discovery, and provider thread/session identities. Application constraints are
in [ADR 0038](adr/0038-local-subscription-cli-execution-for-agent-swarm.md) and
the [native qualification contract](agent-swarm-qualification.md).
The [official permissions documentation](https://learn.chatgpt.com/docs/permissions)
describes the separate named-profile filesystem and network controls.

Dedicated protocol fixtures exercise subscription-only authentication,
effective-setting substitution, response/notification ordering, exact resume
identity, disallowed interactive/delegation requests, failed completion,
duplicate messages, bounded input, and widened write sandboxes. Fixtures make
no model calls and are not native provider qualification. A genuine resume
probe found that the provider replays previous-turn token-usage telemetry;
the driver now ignores that old turn ID only for token accounting, while
keeping result, item, and completion identity checks strict. The corrected
fresh/resume probe completed successfully.
