# Initialization input contracts

This document defines the files accepted by the approved `init` import flags.
It complements the [operator CLI reference](cli-reference.md). The bounded
declaration parsers and reviewed import workflow are part of the default
CLI-first build. Parsing a declaration alone does not approve it, read its
referenced files, or start a process.

These are local prerequisite declarations, not Room Setup Specifications.
An external-only Room needs no managed provider credential. Do not put secret
values, installation control credentials, Membership credentials, or Runner
credentials in these JSON files.

## File selection and bounds

| Flag | Required `schema` | Maximum JSON file size |
| --- | --- | --- |
| `--runner-template` | `worldstream/runner-template/v1` | 64 KiB |
| `--provider-declaration` | `worldstream/model-provider-credential-import/v1` | 64 KiB |
| `--agent-profile` | `worldstream/studio-agent-profile-publish/v2` | 64 KiB |
| `--client-declaration` | `worldstream/client-declaration-import/v1` | 256 KiB |

Each flag accepts at most 16 explicit files. Unknown fields, including nested
fields, unsupported schemas, malformed JSON, and a file of the wrong kind
are errors. Errors must not echo file contents or secret values.

Paths are nonempty UTF-8 strings, at most 4096 bytes, with no control
characters. The importer resolves relative paths against the file that
declares them, not the shell's current directory. Absolute local paths are
allowed. There is no shell evaluation, environment-variable substitution,
tilde expansion, directory discovery, wildcard matching, or network fetch by
the importer. These rules also apply to paths inside a declaration.

The expanded non-secret declaration set is limited to 16 MiB. Each referenced
Client Release or Client Binding bootstrap file is limited to 256 KiB.
Referenced executable and secret bytes are separate from that JSON budget:
an executable is nonempty and at most 256 MiB; a provider secret is nonempty
and at most 64 KiB. The importer must bound reads before parsing or hashing.

## Runner Template

Use the existing versioned Runner Template shape. Every field below is
required. The example shows shape only: its placeholder executable digest
does not approve or identify an executable.

```json
{
  "schema": "worldstream/runner-template/v1",
  "template_id": "counter-reference",
  "revision": "1",
  "display_name": "Counter reference Runner",
  "executable": {
    "path": "./bin/worldstream-managed-agent-host",
    "blake3": "0000000000000000000000000000000000000000000000000000000000000000"
  },
  "compatibility": [{"activity_pack_id": "worldstream.counter", "exact_revisions": ["4.0.0"]}],
  "capacity": {"maximum_concurrent_invocations": 1},
  "health": {"path": "/health", "timeout_ms": 1000, "stale_after_ms": 5000},
  "non_secret_environment": {},
  "secret_environment": [],
  "instances": [{"instance_id": "counter-local", "health_address": "127.0.0.1:9511"}]
}
```

The existing `executable.blake3` field uses **64 lowercase hex characters,
without a `blake3:` prefix**. Preview must resolve the executable to its exact
canonical local target and verify its bytes. An import never runs it.

The existing `exact_revisions` field contains exact Pack **version labels**
such as `4.0.0`; it does not contain Pack semantic digests. This preserves the
existing Runner Template contract. Room authority still uses an exact Pack
reference and its semantic digest. Counter compatibility is not evidence of
Heist compatibility.

For this public initialization input, `secret_environment` must be `[]`.
Do not copy private vault references from another installation. Managed
provider access is selected through a named Agent Profile below. The retained
internal Runner Template type is unchanged; this is a restriction on fresh
public imports.

The existing Runner registry remains responsible for semantic bounds,
executable verification, immutable template revisions, unique instance IDs,
and loopback health addresses. Successful JSON parsing is not evidence that
those checks passed or that an instance is running.

## Named provider credential

Use a public file-selection declaration, not an internal vault record:

```json
{
  "schema": "worldstream/model-provider-credential-import/v1",
  "credential_id": "local-model",
  "display_name": "Local model provider",
  "provider": "open_ai_compatible",
  "secret_file": "./private/model-token"
}
```

All five fields are required. `credential_id` is 1–64 lowercase ASCII
letters, digits, `_`, or `-`, beginning with a letter or digit. `display_name`
is nonempty, at most 256 bytes, with no control characters. The only current
provider value is `open_ai_compatible`.

The selected secret file must be a protected, owner-only, non-symlink file.
Preview does not read it and does not hash secret bytes. Approved apply loads
the selected file and stores the secret in the existing local vault. The
resulting retained `worldstream/model-provider-credential/v1` record contains
a vault reference, not `secret_file`. These schemas are deliberately different;
an internal vault record is not an accepted public import file.

Existing credential identities are immutable. Import is not implicit credential
rotation. Re-import must not replace a named secret with unrelated bytes or
regenerate an existing installation's authority.

## Agent Profile

Use the existing named publish request shape. A generic external integration
has no managed provider reference:

```json
{
  "schema": "worldstream/studio-agent-profile-publish/v2",
  "profile_id": "external-agent",
  "revision": "1",
  "display_name": "External agent",
  "non_secret_configuration": {},
  "host_contract": {"kind": "generic_mcp"}
}
```

The `studio` segment is a retained schema identifier, not a dependency on
the Studio web application. It must not be renamed as part of this import.

For a managed reference host, supply the named credential and an exact
Runner Template revision. Both dependencies may already be installed or be
included in the same explicitly approved import batch:

```json
{
  "schema": "worldstream/studio-agent-profile-publish/v2",
  "profile_id": "counter-agent",
  "revision": "1",
  "display_name": "Managed Counter agent",
  "non_secret_configuration": {},
  "host_contract": {
    "kind": "managed_reference",
    "host_contract_revision": "v1",
    "runner_template": {"template_id": "counter-reference", "revision": "1"},
    "provider": "open_ai_compatible",
    "provider_address": "127.0.0.1:11434",
    "model_id": "local-model"
  },
  "managed_provider_credential_id": "local-model"
}
```

The examples show declaration shape, not a verified runnable host contract.
Use the host contract revision supported by the selected implementation.
The importer resolves the exact Runner Template and named provider references
before publication. Compatibility with the Room's exact Pack is checked when
setting up the Room or starting its Runner, not inferred from this import.
Generic profiles require the
managed credential reference to be absent or `null`. Managed profiles require
it to be present. `secret_settings` and inline tokens are not accepted here.

Existing profile validation and immutable `(profile_id, revision)` rules
remain authoritative. A name is not approval for an executable, Room Role,
or participant capability.

## Client declarations

Select an explicit bounded set of existing descriptors:

```json
{
  "schema": "worldstream/client-declaration-import/v1",
  "release_files": ["./releases/agent-heist-web.json", "./releases/negotiate-web.json", "./releases/negotiate-web-v2.json", "./releases/inspector-web.json"],
  "bindings_file": "./local-bindings.json"
}
```

All three fields are required. `release_files` contains 1–16 local file paths.
`bindings_file` selects exactly one local file. This wrapper does not inline
or replace the existing release, deployment, binding, or fallback contracts:

| Referenced file | Existing required schema and contents |
| --- | --- |
| Each release | `worldstream/activity-client-release/v1`: `schema`, `client_id`, `release_digest`, `client_contract`, `artifacts`, `surfaces`, `conformance` |
| Binding bootstrap | `worldstream/client-binding-bootstrap/v1`: `schema`, `deployment_trust_policy`, `deployments`, `bindings`, `inspector_fallback` |

Deployment entries use `worldstream/client-deployment/v1`; binding entries use
`worldstream/client-binding/v1`; the required inspector fallback uses
`worldstream/inspector-fallback/v1`. The tracked
[CLI import wrapper](../config/activity-clients/cli-import.json) explicitly selects the
[first-party declarations](../config/activity-clients/local-bindings.json)
and their release files. These local development launch URLs require a separately
running Client Host; importing the wrapper does not verify one is listening.
The typed Activity Client
validators remain authoritative for all nested fields and bounds.

Preview resolves release/deployment/binding cross-references and shows the
exact client identity, Pack identity, surface, trust label, and launch target.
An externally trusted deployment is not byte-verified evidence. Re-import
must preserve disabled or revoked bindings. It must not select arbitrary
nearby files or upgrade an immutable release behind a binding.

The explicitly reviewed deployment policy is retained separately from the
existing binding records. An identical policy is reused; a conflicting policy
is rejected. Policy changes are not supported by `init`. Existing installations
without this policy record retain the conservative `verified_only` startup
default until an explicit approved import establishes the record. Starting the
controller never imports declarations or resets binding status.

Importing a client declaration does not start or deploy its Client Host.
`client open` can use an already approved compatible deployment; it cannot
approve or install a missing one.

### Hosting the declared clients separately

For the repository's current local-development declarations, start the
independent Client Host in another terminal:

```sh
pnpm ui:dev
```

This workflow builds and serves the retained client directories at
`http://127.0.0.1:5173`; it does not start the Runtime or Controller. The
Activity Clients are independent of the retired Studio web application. See
[Activity Client local development](activity-clients.md#local-development)
for the existing hosting and artifact-identity workflow.

If an approved launch URL is unavailable, start or restore that separate Client
Host and check the declared surface before retrying launch. Do not reset
installation authority or re-import declarations merely because a host is
offline. A different launch target requires a new exact review and appropriate
new immutable identities; it cannot silently replace an existing Deployment.
Local examples remain `externally_trusted`, not attestation of running bytes.
Import may succeed while the Client Host is offline: it prepares approved
metadata, not runtime health or browser sessions.

When the Controller does not use port 9420, changing only the Client Host
Content Security Policy is insufficient. The build-time
`VITE_WORLDSTREAM_SUPERVISOR_URL`, Host `--controller-origin`, prepared exact
Release and Deployment launch URLs, and Runtime
`--participant-console-origin` must agree. Building for another Controller
changes the artifact digest, so do not reuse the checked-in Release or Binding.
Follow the complete
[alternate-Controller preparation recipe](activity-clients.md#local-development),
then review and import the generated `client-declaration.json` below.

## Review and application boundary

Initialize the protected installation first. Import preview requires its
existing Host identity; it does not invent or persist one during review:

```sh
worldstreamctl init --json
worldstreamctl init --agent-profile ./config/initialization/external-agent.json --preview --json
worldstreamctl init --agent-profile ./config/initialization/external-agent.json --approve-imports 'blake3:<exact digest from import_review.digest>' --json
```

The approval argument above is a placeholder: copy the complete digest from
your own preview. Use the same explicit `--config`, `--state-dir`, and import
file selections for preview and apply when using a non-default installation.
There is no blanket `--yes` approval. A generic external Profile needs no
provider declaration. Neither base initialization nor import starts services.

Import prerequisites before starting the controller. Runner Template and named
provider registries are startup snapshots; import does not hot-reload an
already-running controller. After later imports, explicitly reload that
controller using the same installation's configuration and state options:

```sh
worldstreamctl server controller-stop
worldstreamctl server start
```

This stops only the controller, then starts it with the refreshed catalogs and
reuses a healthy retained Runtime. `server restart` is different: it restarts
the Runtime and is not a controller-catalog reload command. The importer never
issues these lifecycle commands on the operator's behalf.

The importer must parse and validate the complete bounded non-secret input
set before it returns an approval digest. Review binds the exact declaration
bytes, resolved canonical declaration paths, selected installation, verified
executable target and BLAKE3 digest, and declared client identities and launch
targets. Different inputs or targets require a new preview and approval.
Input order must not grant different authority. Preview returns the typed
`worldstream/initialization-import-review/v1` receipt. Its digest is `blake3:`
followed by 64 lowercase hexadecimal characters. The versioned internal binding
is serialized with fixed struct-field ordering and sorted canonical document
paths and named identities, then hashed with BLAKE3's derive-key context
`worldstream/initialization-import-review/v1`. It also binds retained Host
identity and the exact selected configuration plus effective non-secret choices.
The bare declaration parser does not perform this review.

Apply must recheck the reviewed inputs before mutation. It must enforce
protected-path rules, existing semantic validators, reference resolution,
immutable identities, and existing trust/revocation state. A successful parse
alone authorizes none of these actions. Provider secret bytes never appear in
the approval digest, JSON reports, ordinary output, or diagnostics.

Apply returns `worldstream/initialization-import-apply/v1`, identifying created
and reused prerequisites without exposing private vault references. A failed
publication can leave a subset installed: do not interpret failure as rollback,
delete retained authority, or supply unrelated replacement files. Inspect the
installation and retry the exact reviewed inputs. Import never rotates an
existing named provider credential or changes an immutable Profile revision.

A client retry may complete a reviewed identity whose status was already
published, preserving that status exactly. It does not recreate a missing
status beside an existing identity: doing so could silently undo a previous
disable or revocation. That inconsistent state fails closed and requires
investigation or restoration of trustworthy retained state, not a new blanket
approval. This is bounded import recovery, not general installation repair.

The import and catalog-selection tests do not prove a live model invocation,
live Heist or Negotiate run, browser handoff, or production deployment. Those
require their separate end-to-end qualification; this work does not close the
existing IMO-81 live-proof debt. Counter-compatible examples must not be
presented as Heist-compatible managed execution.

See the executable declaration fixtures in the retained Controller crate's
[initialization tests](../crates/worldstream-studio-supervisor/tests/initialization_inputs.rs).
