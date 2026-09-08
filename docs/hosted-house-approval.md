# Approve the installed House Agents

This is an operator-only step for the hosted preview. It is not a public API,
a migration, or an automatic startup action. Use it after the approved
spending-limited OpenRouter credential and the exact Fly image are installed.
Keep launches and House calls closed until the final checks pass.

The current candidate Listing is Agent Heist `0.11.0`. Its two House strategies are
Cooperative Planner `7` and Skeptical Auditor `6`. Both use the exact Granite
model and DeepInfra provider route declared in their immutable files. Their
instructions differ; this is a two-strategy exhibition, not a comparison of two
models. This candidate pins the clock-safe Pack `0.3.0`, v4 Activity Client,
result projector `0.3.0`, and Runner Template `5`. The provider route and hard
allowances are unchanged. Check the deployment record before treating the
candidate as installed or approved.

Keep Cooperative Planner `1` (the Qwen route) and Listings `0.4.0` and
`0.5.0` as retained records. Do not change their canonical bytes or transfer
their assignments to the successor. Keep the old Qwen approval unavailable
for new assignments while its exact route does not meet the privacy policy.
Also retain Listing `0.6.0`, Cooperative Planner `2`, Skeptical Auditor `1`,
and Runner Template `1` unchanged. The clock-safe successor needs new approvals
for its actual installed records and executable. Publishing the successor
migration grants no approval and starts no provider call.

Also retain Listing `0.7.0`, Planner `3`, Auditor `2`, and Template `2` for
their historical assignments. Listing `0.8.0` uses the corrected TLS transport
and exact canonical-model response mapping. Its executable needs its own
approvals; do not reuse the previous executable's approval.

Retain Listing `0.9.0`, Planner `5`, Auditor `4`, and Template `4` as well.
The `0.10.0` successor separates the 30-second assignment operation budget
from the 750 ms health probe and lets the model host close a stale Action turn
without exiting or repeating its provider call. It needs fresh approvals for
the actual installed binary. Old assignment launch records remain unchanged;
only newly registered assignments receive the new operation budget.

Retain Listing `0.10.0`, Planner `6` and Auditor `5` unchanged. The `0.11.0`
candidate adds public Heist guidance in behavior policy revision `2`, including
clue ownership and commitment priority. It does not embed fixture answers or
increase allowances. The helper now responds to protocol heartbeats and includes
the sealed role on each observation. Verify the actual executable still matches
Template `5` before approving it; never overwrite an installed template to make
an executable mismatch pass. Candidate metadata is not evidence of live gameplay.

## What this evidence means

The Controller retains Agent Profiles and Runner Templates by name and revision.
It does not publish individual content-addressed identities for those records.
The Platform approval table needs audit digests for the exact installed records.
This tool defines **operator evidence recipe v1**, not a new core profile protocol:

- `agent_profile_revision_digest`: BLAKE3 of the complete retained profile encoded
  as `worldstream/canonical-json/v1`. The opaque vault reference participates in
  this hash but is not included in the output receipt.
- `runner_template_revision_digest`: BLAKE3 of the complete retained template
  encoded with the same canonical JSON recipe.
- `runner_executable_digest`: BLAKE3 of the actual captured executable bytes.
  It must equal the executable digest in the approved retained template.
- `approval_receipt_digest`: SHA-256 of the canonical JSON receipt, without a
  trailing newline and without the digest field itself.

The receipt binds the House Agent Revision, installation ID, named credential
ID, source commit, image digest, individual record hashes, and successful CLI
import receipt. The CLI aggregate import-review digest is recorded separately;
it is never used as an individual profile or template digest.

This is a reproducible operator observation. It is not a signed Runtime
attestation, proof of provider credit, or proof that an LLM call succeeded.
The Runtime still checks its actual execution dependencies independently.

## Capture the actual installation

Use Node.js `24.18.1` and run `pnpm install --frozen-lockfile` in the operator
checkout first. The tool uses the pinned workspace BLAKE3 library; it needs no
generated SDK build.

1. Verify the deployed source commit, Fly image digest, and installation ID.
   Do not copy these values from a different local build or a planned deployment.
2. Keep the machine in maintenance with the Controller and Runtime stopped.
   On the exact Fly machine, repeat the appliance's existing two-phase import
   with its generated declarations. Use `--preview`, review `import_review.digest`,
   then repeat the same arguments with `--approve-imports` and that exact digest.
   An exact retry reuses the installed records. Capture the successful `--json`
   output privately as `import-apply.json`. Do not pipe secrets or environment
   dumps into this file.

   The command shape on the machine is:

   ```sh
   /usr/local/bin/worldstreamctl --config /run/worldstream/worldstream.toml init \
     --state-dir /var/lib/worldstream/studio \
     --runner-template /run/worldstream/generated/openrouter-house-runner.json \
     --provider-declaration /run/worldstream/generated/openrouter-provider.json \
     --agent-profile /run/worldstream/generated/cooperative-planner.json \
     --agent-profile /run/worldstream/generated/skeptical-auditor.json \
     --preview --json
   ```

   For the apply call, replace only `--preview` with
   `--approve-imports 'blake3:<reviewed import digest>'`. Preserve the appliance's
   protected authority configuration. Never put authority values in command
   arguments, logs, or chat.
3. Capture these records from that same installation into a private local
   directory. Preserve their relative layout beneath a `studio` directory:

   - `agent-profiles/revisions/<hex profile ID>/37.json` for
     `house-cooperative-planner` revision `7`;
   - `agent-profiles/revisions/<hex profile ID>/36.json` for
     `house-skeptical-auditor` revision `6`;
   - `runner-templates/installed/openrouter-house--5.json`;
   - `model-provider-credentials/installed/hosted-openrouter.json`.

   Profile path components are the lowercase hexadecimal encoding of UTF-8;
   `35` encodes revision `5`; `36` encodes revision `6`. Capture the actual
   `/usr/local/bin/worldstream-managed-agent-host` binary separately.
   Do not capture or open vault secret files, the OpenRouter API key, controller
   credentials, or process environment. The named credential record contains
   only metadata and an opaque reference, not the API-key value.
4. Use one coherent capture. Do not combine profiles from one installation with
   a binary or import receipt from another. Use actual absolute paths without
   symlink traversal; on macOS this usually means `/private/tmp`, not `/tmp`.
   The captured state directory and output parent must be mode `0700`.
   Metadata and import-receipt files must be mode `0600`.

## Prepare private receipts and SQL

Use a **new** output directory under the private capture directory:

```sh
node scripts/hosted-house-approval.mjs \
  --controller-state /private/tmp/worldstream-approval-capture/studio \
  --runner-binary /private/tmp/worldstream-approval-capture/worldstream-managed-agent-host \
  --import-receipt /private/tmp/worldstream-approval-capture/import-apply.json \
  --installation-id fly-primary \
  --credential-id hosted-openrouter \
  --source-revision '<actual deployed Git commit>' \
  --image-digest 'sha256:<actual deployed image digest>' \
  --house-revision config/hosted/house-agents/cooperative-planner-7.json \
  --house-revision config/hosted/house-agents/skeptical-auditor-6.json \
  --output-dir /private/tmp/worldstream-approval-capture/approval
```

The tool checks the named references, successful import selections, private
file modes, duplicate-free bounded JSON, opaque credential correspondence,
and approved executable digest. It refuses symlinks and existing output
directories. It does not contact Fly, Supabase, or OpenRouter.

The private output contains:

- `receipts.json`: reviewable evidence; no opaque vault references or secret values.
- `approve.sql`: inserts approvals with `available_for_new_assignments = false`.
- `activate.sql`: separately enables only those exact, non-revoked approvals.

Both SQL files use transactions and exact identity conflict guards. Applying
`approve.sql` again is safe for an identical record. It does not silently
replace an existing approval or re-enable a revoked one. Changed source/image,
profile, template, credential binding, or import receipt produces different
evidence and must not be forced into the old primary key. Review a new Host
installation identity or a new House Agent Revision as appropriate; do not
delete or rewrite historical approval records.

## Review, publish, and activate

Review the receipt against the actual captured deployment before applying
`approve.sql` through the existing protected administrative database connection.
The selected House Agent Revisions must already exist in the Platform catalog.
There is no production approval RPC or automatic publisher.

Before applying `activate.sql`, confirm current Runner readiness, the protected
named credential, the current spending-limited provider key, ledger limits,
and the closed-launch deployment checks. SQL activation does not itself open
the platform or make a model call. Then perform the bounded live end-user test.

Verify that the actual key permits the exact model/provider route, required
parameters, and price ceilings with ZDR and data collection denied. A successful
key lookup or a started Runner is not proof of model-route eligibility. Never
enable fallback, relax privacy, or substitute a model to make this check pass.
If a route is unavailable, leave its approval unavailable.

One creator leaves two eligible seats, including the optional Broker seat.
Both distinct current House revisions must be approved and available to fill
those seats. Activating only the Auditor supports one House seat when two real
participants have already claimed the other seats; it cannot fill both seats
with copies of the same revision.

Keep these files private with the deployment evidence. Do not commit operator
receipts, capture folders, SQL filled with installation evidence, or credentials.

## Verify the tool

```sh
node --test scripts/hosted-house-approval.test.mjs
```

An optional database test uses `WORLDSTREAM_APPROVAL_TEST_DATABASE_URL` pointed
at a disposable local Supabase database with the reviewed catalog loaded. It
tests exact retry, separate activation, and conflicting evidence inside
transactions that are rolled back. Never point that test at production.
