# Outside-adopter trial kit

WorldStream 0.1.0 is not adoption-qualified until six distinct
non-contributors pass the released-artifact journeys required by IMO-124. Local
tests, maintainer dry runs, generated fixtures, and this validator do not count
as people or evidence.

The qualification set is exactly:

- three Pack Authors in at most 60 minutes each, covering both deterministic
  and prompt-assisted scaffolding; and
- three Application Integrators in at most 30 minutes each, using the unchanged
  official Negotiate Pack with independently controlled agents.

At least two supported installation profiles must be represented. Every trial
must start from a clean environment and use only one exact released
distribution and its public documentation. Source, ADRs, Linear, chat history,
Rust/runtime rebuilds, manual database access, paid dependencies, and
maintainer intervention are forbidden.

The only eligible v1 profiles are `native-linux-x86_64`,
`native-windows-x64`, and `oci-linux-amd64`. A source checkout or source
archive is not an installation profile for this trial because the frozen
constraints forbid rebuilding the Runtime. The signed documentation release
subject must carry the complete
`qualification/outside-adopter-v1/` directory. A maintainer must send that
extracted release artifact and the verified Starter, not a checkout copy of
the same files.

## Clean-environment journey

Before timing starts, the observer verifies the Starter and records its exact
release-manifest identity. The participant receives a fresh machine, VM, or
container with no WorldStream source/cache/state, then follows only the public
Starter documentation. The observer may state the goal and operate the timer;
answering a product question, fixing a command, accessing a database, or
supplying an undocumented step is a maintainer intervention and invalidates
the trial.

Use the released `checkpoint.py` helper to capture both the release manifest
and Starter archive, then wrap every documented checkpoint command. It uses an
argv vector without a shell, refuses symlinks and overwrite, and records only
safe names, SHA-256 identities, byte counts, durations, exit codes, timeout
status, and output digests. It never records arguments, paths, URLs, raw logs,
prompts, model output, credentials, or commercial data. The exact commands and
checkpoint inventories are documented in the kit's README.

## Receipt boundary

Each observer writes one bounded `worldstream/outside-adopter-trial/v1` JSON
receipt. It contains a pseudonymous participant ID, journey/route/profile,
start/completion time, exact release-manifest SHA-256, closed constraints,
completed checkpoint names, exact release-artifact rows, redacted command
checkpoint rows, and bounded diagnostics. The artifact inventory must contain
the release manifest and Starter Distribution. Each command checkpoint has an
argv digest, stdout/stderr digests, duration, and exit code; its name inventory
must exactly equal the journey checkpoint inventory. It contains no command
arguments, paths, URLs, credentials, prompt text, model output, commercial
data, or raw logs.

`qualification/outside-adopter-v1/receipt.template.json` is deliberately an
unrun template with a different schema, null attestations, empty inventories,
an extra notice, and `outcome: unrun`. The checkpoint helper never edits it.
Only the real observer authors a v1 receipt after a real trial. The repository
does not create or self-sign one.

Receipts are independent evidence inputs. The repository never creates a
passing receipt. Run the validator only after real observers provide all six:

```sh
python scripts/adopter-trial.py \
  --receipt-dir ./outside-adopter-receipts \
  --release-manifest ./release/release-manifest.json \
  > outside-adopter-qualification.json
```

The validator rejects duplicate participants/trials, a mixed release identity,
missing routes/checkpoints/artifacts/commands, artifact substitution,
time-limit violations, forbidden assistance, secret-like diagnostics, fewer
than six people, or fewer than two profiles. A successful canonical summary
can then be bound by `scripts/release-qualification.py` into the separately
signed `release-qualification-manifest.json`. It is not inserted into the
primary manifest that the trials consumed, and it cannot by itself prove that
the named people were external or turn the compatibility specification into a
release. The release reviewer separately checks the observer-held participant
and clean-environment provenance.

The qualification verifier recomputes the exact six receipt-summary rows,
requires both authoring routes and at least two supported installation
profiles, authenticates official and custom Starters against the same primary
release, and then verifies both signature levels. Missing primary/qualification
signatures return an explicit non-success result; a template, local maintainer
run, or structural-only Starter never becomes adopter evidence.
