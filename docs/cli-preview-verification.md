# CLI preview verification

Date: 2026-09-03

This record covers IMO-141, IMO-143, IMO-148, and IMO-149. It is local development
evidence. It is not a signed release, a production qualification, or proof of
the complete CLI-only first-success flow. IMO-144 owns that flow.

## Changes under test

- Direct participants qualify for launch after an acknowledged synchronization.
- Browser participants qualify after receipt of an authorized delivery. A
  single-use acknowledgement renews a five-second presence window. Cookies,
  health checks, and sending a response do not establish presence.
- Only required seats gate launch. Launch retries use the original saved input.
- CLI credential exports keep Membership and Runner authority in separate
  owner-protected files. They do not print credentials.
- CLI browser launch uses the existing approved client selection and one-use
  handoff. External Runners cannot be started or stopped by managed controls.
- SQLite automatically advances due timers through the existing authorization
  and commit paths. The scheduler publishes authorized live updates.
- PostgreSQL keeps lease maintenance and manual timers. Automatic PostgreSQL
  timers are not part of this MVP.
- Managed startup can select a separate local browser-client origin. It retains
  that choice, reuses it when the flag is omitted, and rejects conflicting or
  unusable values before starting another Controller.

## Verification results

- 34 Python Room and Runner SDK tests passed.
- 20 browser SDK tests and its TypeScript check passed on Node 24.18.1.
- Heist: 14 tests and its TypeScript check passed.
- Negotiate: 9 tests and its TypeScript check passed.
- Inspector: 83 tests and its TypeScript check passed.
- Supervisor: 8 control-admission tests, 17 handoff tests, 7 setup-operation
  tests, and 18 setup/launch tests passed. The final setup-operation run also
  checks browser receipt expiry, wrong Origin, missing cookie, wrong frame,
  one-use acknowledgement, and renewal after a new authorized delivery.
- The final focused Rust run passed: 18 CLI unit tests, 10 daemon unit tests,
  and 6 CLI/stream integration tests. The latter cover credential export,
  browser opening, launch and setup recovery, external Runner inspection, and
  direct participant synchronization through disconnect.
- Five Server timer tests passed, including automatic scheduler publication to
  a synchronized participant and no duplicate transition on a repeated tick.
- Eight SQLite timer tests and the fixed-cutoff recovery test passed.
- Ten CLI contract tests passed in each of the default and preview builds.
  The public process-launch test passed for retained client-origin reuse,
  conflict rejection, and an unchanged older record with the default origin.
- Strict Clippy checks passed for both affected crates, their libraries and
  binaries, and the changed CLI/setup tests with `cli-operator-preview`.

The commands used the locked dependency set, one Cargo job, and
`CARGO_INCREMENTAL=0`. CLI integration used `cli-operator-preview`.
These are focused regression results, not the final full-workspace run.

## Review corrections

### Standards

The independent Standards review found no mandatory standards violation.
Tool-enforced lint issues were corrected before the final test run.

### Spec

The independent Spec review found that browser polling did not supply the
persistent stream evidence used by direct clients. The receipt acknowledgement
above fixes that mismatch.

Timer review found a repeated bootstrap-source read. SQLite bootstrap and
timer authentication now share one protected read. A separate, pre-existing
inherited-handle read in Runtime configuration validation is recorded in
IMO-147. This work does not claim that full foreground input path qualified.

Review totals: zero mandatory Standards findings; two Spec/correctness findings
corrected in this slice. Full-flow verification remains separate.

### Separate client-origin review

IMO-149's independent Standards review found no mandatory violation. Its Spec
review confirmed retained selection and conflict handling. Cross-checks found
two unusable inputs: the reserved legacy Studio origin, and origin spellings
that do not match the browser's canonical Origin header. Managed startup now
rejects both before retaining a setting. CLI regressions cover the reserved
origin, trailing slash, explicit default port, and noncanonical port spelling.
The general legacy handoff validator keeps its existing behavior.

## Delivery boundary

New operator commands still require the internal `cli-operator-preview`
build feature. Studio removal, the complete live walkthrough, and the final
getting-started rewrite remain separate steps. Existing state and independent
Activity Clients remain in place.
