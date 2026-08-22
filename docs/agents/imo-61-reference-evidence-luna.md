# IMO-61 Linux reference evidence aggregator

This lane adds a bounded, fail-closed aggregation command for the final
acceptance packet. It is an evidence reader, not a benchmark runner: it reads
only the five paths explicitly supplied on the command line and never opens a
database, starts a daemon, changes a manifest, or promotes a fixture to
release evidence.

## Interface

```sh
python3 scripts/reference-evidence.py \
  --counter-report /path/to/counter.json \
  --heist-report /path/to/heist.json \
  --sqlite-report /path/to/sqlite.json \
  --postgres-report /path/to/postgres.json \
  --soak-report /path/to/one-hour-soak.json \
  --output /path/to/reference-summary.json
```

All five reports are required explicitly. Inputs must be regular, non-symlink
UTF-8 JSON files no larger than 8 MiB. JSON depth, object size, string size,
and sample count are bounded. The output is canonical sorted-key JSON with no
input paths, credentials, payloads, Room IDs, or raw samples.

Each report must include:

```json
{
  "schema": "<accepted producer schema>",
  "status": "<accepted terminal status>",
  "release_evidence": false,
  "performance_class": "reference_non_release",
  "identity": {
    "product": "worldstream",
    "profile": "linux-reference",
    "version": "reviewed-build-label",
    "artifact_sha256": "sha256:<64 lowercase hex characters>"
  }
}
```

The five reports must all use exactly `identity.product=worldstream`,
`identity.profile=linux-reference`, and the same `identity.version`. Their
artifact SHA-256 values may differ because each report is a distinct subject.
Any version or profile mismatch blocks aggregation; the tool never combines
unrelated platform/build artifacts.

The explicit negative release/SLA labels are mandatory. A prose limitation or
an omitted field cannot satisfy them. Accepted producer schemas are the live
Counter and Heist schemas, the existing SQLite soak/native-restore schemas,
the PostgreSQL live/native-restore schemas, and
`worldstream/soak-evidence/v1` for the one-hour input. Status, schema, identity
and labels are checked before any measurement is accepted.

## Measurements and gaps

Reports may provide a bounded `measurements` object containing `latency_ms`
(raw samples or a nearest-rank `{definition,sample_count,p50_ms,p95_ms,p99_ms}`
object), `load`, `fan_out`, `memory`, `database_growth`, and `recovery`.
The aggregator also understands the existing soak report's
`statistics.command_duration_ms` and `statistics.memory` fields. It recomputes
nearest-rank p50/p95/p99 only from supplied samples; it never derives a
percentile from a mean or from another percentile. Multiple source percentile
triplets remain separate under the source name; the tool does not pretend that
percentiles from different populations can be combined.

The summary keeps source-specific values for:

- load: connections, active rooms, actions/transition rate;
- observation fan-out parameters;
- measured peak RSS and method;
- database and WAL growth;
- recovery duration or recovery percentile triplets; and
- p50/p95/p99 latency.

`status: "pass"` additionally requires this complete source-specific matrix:

| Source | Required coverage |
| --- | --- |
| Counter | latency, load, observation fan-out |
| Heist | latency, load, observation fan-out |
| SQLite | latency, measured memory, database growth, recovery timing |
| PostgreSQL | latency, measured memory, database growth, recovery timing |
| one-hour soak | latency, measured memory, database growth, completed one-hour window |

Values from another source cannot satisfy a missing backend row. Missing
coverage yields `status: "incomplete"` with a source-specific gap; malformed
coverage is blocked. The pre-existing local soak report without packaged
Linux-reference identity labels remains honestly ineligible and is rejected
before aggregation.

Missing dimensions, an incomplete one-hour window, an unconfigured database,
or unavailable recovery timing are listed as deterministic `gaps`. Malformed,
misidentified, release-labelled, or otherwise invalid reports make the whole
summary `status: "blocked"`. Valid reports with honest missing measurements
make it `status: "incomplete"`; only a complete supplied measurement set is
`status: "pass"`. Every result remains `release_evidence: false` and
`performance_class: "reference_non_release"`.

## Exact input inventory

The output contains one sorted inventory item per readable supplied input
(including a readable JSON report rejected for identity/schema reasons) with
its logical kind, byte count, schema, terminal status, and SHA-256 of the exact
file bytes. `input_sha256_inventory.sha256` hashes the canonical inventory
representation. This makes the packet reproducible without persisting local
paths or report contents.

## Verification

These are fixture-only boundary tests and do not constitute measurements:

```sh
python3 -m unittest -v tests/reference_evidence.py
python3 -m py_compile scripts/reference-evidence.py tests/reference_evidence.py
python3 -m unittest discover -s tests -p 'reference_evidence.py'
ruff check scripts/reference-evidence.py tests/reference_evidence.py
ruff format --check scripts/reference-evidence.py tests/reference_evidence.py
git diff --check -- scripts/reference-evidence.py tests/reference_evidence.py docs/agents/imo-61-reference-evidence-luna.md
```

Parent verification on 2026-08-21: 8/8 fixture tests passed; `py_compile`,
Ruff lint, Ruff format check, and `git diff --check` passed. The fixture suite
also proves deterministic output, nearest-rank recomputation, accepted soak
nearest-rank statistics, common-version/Linux-profile identity enforcement,
fail-closed schema/identity/release-label handling, source-specific coverage,
honest incomplete-soak/database gaps, exact input inventory retention for
rejected JSON, and absence of signature-bundle fields.

No commit was created. The aggregator intentionally does not claim the
one-hour soak, native Linux packaging, PostgreSQL provider conformance, or
any release/SLA result unless those facts are present in the supplied reports.
The final signed release manifest may hash this summary and its input reports;
signature-bundle digests are deliberately outside this command's interface.
