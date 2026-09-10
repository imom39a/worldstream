# IMO-208 local real-Host qualification

Passed on 2026-09-10 with the explicit controlled local fixture. Authenticated
browser selection crossed the real platform API, retained SQL coordination,
Gateway, Host, component Runtime, managed Runner, and controlled OpenRouter HTTP
endpoint. This is a local integration witness, not a clean production release.

The retained Archive v3 Pack and v10 browser client were unchanged. The optional
House seat uses Role `mira` with a separate test policy/Profile/Runner identity;
this does not qualify the production Mira policy or IMO-209's four roster choices.

## Exact artifacts

| Artifact | Digest |
| --- | --- |
| Fixture Listing | `blake3:61913551e45b01ffb99ec0fb85b78d92efc9fb49dfd0ec6788b87ed76d93454f` |
| Fixture House Agent | `blake3:92d2c28744483cd38384b993f8fdf2e6ee52e9f9830287e2c4ca485939a2aec2` |
| Archive Pack | `blake3:f40e0a287fcaac6e6bc56629d361ede079d6c3c60aa0068caa3a451dfb8c0b64` |
| Archive client | `sha256:9be6d6548f2d62baba805ec461b27fbd1021ed0d61fbd0e8ffd1d779616bc534` |
| Imported Profile revision | `blake3:1cc22c3bffbfbc73fce6a906e4ed24df87c5b53089512eedc6ab5c0b1e25c1e1` |
| Installed Runner Template | `blake3:34a0c09f255aa2c54877d4984a215a29b86c94b2cc4bf5d97a22d4cfc1f61f97` |
| Retained Runner executable | `blake3:4c754b0229d31f54ebb4aee3d5be83e502a8cde87366341de4a40bbd87903c23` |

The published Profile digest above includes its retained Host binding; it is
distinct from the import-source document digest. Credential references and
authentication tags are excluded from the attached evidence.

## Browser and durable evidence

[The fresh journey](evidence/journey.json) verifies these Runs:

| Option | Activity Run | Assignments | Outcome |
| --- | --- | --- | --- |
| Solo | `54118515-e084-4e2a-9d1a-cf5c7d082f16` | 0 | Empty-handed extraction, terminal re-entry |
| Supplied | `234da3cb-3184-482d-826b-0563442ac773` | 1 | Two specialist steps, empty-handed extraction, terminal re-entry |

Both journeys prove immutable selected inputs, changed-option retry refusal,
idempotent launch/start, one Genesis, exact playable Memberships, original
terminal Membership re-entry, released Run capacity, and no public result.
The supplied journey additionally proves exact House/Profile/Template lineage,
one completed provider attempt (10,895 accounted input units; 331 output units),
and unchanged consumed allowance plus durable Runner retirement after re-entry.
Neither journey claims authentic-ledger recovery or live model quality.

The first supplied Run exposed two terminal recovery bugs: the SQL recovery
reader omitted original Assignments once their reservations became `released`,
and the platform adapter rejected that reservation state. After the generic
fixes, [the original retained Run](evidence/supplied-terminal-recovery.json)
`9c5f18ca-9c64-4dd6-a9a5-ff4645c673d9` passed authenticated GET and browser re-entry
across an ordinary services restart. Its Memberships, Assignments, setup hashes,
completed provider attempt, allowance, and retirement remained identical.

## Reproduction and limits

Use the explicit startup and browser commands in [README.md](README.md).
The helper/provider/startup suite passes 33 focused checks, including local
registration replay across seed-disabled availability in a rolled-back SQL
transaction and evidence redaction. The production Heist Runner keeps its
original retained executable when this fixture is enabled. Ordinary restarts
reuse the fixture's exact retained Runner revision; they do not repin binaries,
reset Runs, or replenish consumed allowance.

The controlled provider made no paid calls. This witness does not replace the
formation suite's concurrent claims, uncertain Genesis, or active crash recovery
checks; it adds real authenticated browser/Host and terminal restart evidence.
Screenshots remain in `.worldstream/evidence/imo-208-roster-fixture/` alongside
the complete local diagnostic history.
