# Outside-adopter clean-environment kit v1

This directory is an observer aid carried by the released documentation
subject. It does not contain a passing receipt, a participant identity, or a
signature. Local runs, maintainer dry runs, and generated fixtures do not count
as outside-adopter evidence.

Use only the files extracted from one cryptographically verified Starter
Distribution. Do not give the participant a repository checkout, ADRs, Linear,
chat history, a Rust build, database access, paid dependency, or private
maintainer instructions.

## Before the clock starts

The observer, not the participant, records the pseudonymous participant ID and
confirms that the participant has never contributed to WorldStream. Create a
new empty checkpoint directory. From the extracted documentation subject,
capture the exact release identities without copying paths into evidence:

```sh
python3 qualification/outside-adopter-v1/checkpoint.py artifact \
  --name release_manifest \
  --artifact release/release-manifest.json \
  --output checkpoints/00-release-manifest.json
python3 qualification/outside-adopter-v1/checkpoint.py artifact \
  --name starter_distribution \
  --artifact worldstream-starter-0.1.0-linux-x86_64.tar.gz \
  --output checkpoints/01-starter-distribution.json
```

The checkpoint records only SHA-256, byte count, and a safe name. It refuses
symlinks, empty files, changed-during-read files, and output overwrite.

## During the timed journey

Wrap each documented checkpoint command as an argv vector. The helper invokes
no shell, records no arguments or paths, and stores only argv/stdout/stderr
SHA-256 values, sizes, duration, exit code, timeout status, and executable
basename:

```sh
python3 qualification/outside-adopter-v1/checkpoint.py command \
  --name install \
  --output checkpoints/10-install.json \
  -- worldstreamctl pack install worldstream-negotiate.wspack
```

The helper returns the wrapped command's exit code. A failed command remains a
checkpoint for diagnosis but cannot appear in a passing receipt. Raw output is
not copied into the checkpoint. The participant follows only the public
journey; the observer must not troubleshoot or intervene.

Pack Author receipts require one command row for every sorted checkpoint:
`build`, `check`, `inspect`, `prove`, `real_room`, `scaffold`, and `test`, plus
`prompt_scaffold` for the prompt-assisted route. Application Integrator
receipts require `approve`, `attach_independent_agents`, `create_room`,
`evidence_verify`, `forced_reconnect`, `install`, `replay`, and
`restart_readiness`.

## After the journey

The observer copies each checkpoint's `receipt_row` into an independently
authored receipt based on `receipt.template.json`, fills canonical UTC-second
times and constraints, then changes the schema to
`worldstream/outside-adopter-trial/v1` only if the real trial passed. The
template intentionally contains an extra `notice` field, `unrun` outcome, null
attestations, and empty inventories; the validator rejects it unchanged.

Receipts contain no command arguments, paths, URLs, credentials, prompt/model
content, commercial data, or raw logs. Store the checkpoint files alongside
the observer's private trial record; publish only the bounded receipt unless a
review policy explicitly requires the checkpoint bytes.

The project validator consumes six externally authored receipts and the exact
release manifest. It never creates or signs them:

```sh
python3 adopter-trial.py \
  --receipt-dir outside-adopter-receipts \
  --release-manifest release/release-manifest.json \
  > outside-adopter-qualification.json
```

Qualification still requires three distinct non-contributor Pack Authors,
three distinct non-contributor Application Integrators, both Pack Author
routes, at least two supported binary/OCI profiles, the time limits, and zero
maintainer interventions.
