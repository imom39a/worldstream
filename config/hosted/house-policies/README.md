# Archive policy sources

These reviewed behavior-policy source objects populate the exact
`behavior_policy` field of the Mira and Jonah House Agent Revisions in
`../house-agents/mira-1.json` and `../house-agents/jonah-1.json`. The revisions
pin the Archive-specific Profiles in `../house-agent-profiles/`, the separate
`openrouter-house-archive` Runner Template revision `1`, the reviewed Granite
route, the empty tool set, byte accounting revision and unchanged allowance.
The Host still has to import the Profiles and an exact executable-bound Runner
Template, approve their exact digests, and prove route readiness before either
revision is available for a new Assignment. Checked-in source grants none of
those operational approvals. Retained House Agent Revisions are unchanged.

The canonical House revision identities are:

- Mira: `blake3:7e0b07b386009d509d605c9efdbe491a035f219d10ef7ebebc6f71e99461cdde`
- Jonah: `blake3:b88da2260f391c91593996c5913961469619b53a9e457c5ea0783cd3cac59db0`

Policy IDs `worldstream.house.mira` and `worldstream.house.jonah`, revision `1`,
select the managed model boundary's Archive context contract. It accepts only
the current v4 participant Projection or Observation at its advertised Frame
Head, copies reviewed current Activity facts, the speaking companion's task
facts, and its bounded audience-scoped accepted dialogue. It omits accumulated
observations, other companion state, and unreviewed fields. The Pack determines
visibility. There is no external conversation memory or cross-run memory.

Exactly one offered `submit_companion_plan` can be proposed. The response is
checked against the offered closed schema and independently checked for current
task/opportunity revisions and the dialogue byte/privacy bounds before sealed
participant submission. The Pack still decides eligibility and every effect.

The existing ten-attempt allowance, one in-flight call, aggregate accounting,
12,000-byte complete serialized input and 1,000-byte reply-content ceilings,
exact provider/model pin, no fallback, privacy constraints and timeout charging
remain in force. There is no new provider transport, tool or chat invocation.

The retained `archive-v4-model-context.json` fixture is generated from the actual
Pack helpers and schema: both authored scenarios, both Roles, both sources
shared, an offered hatch task, four accepted utterances at the full 192-byte
double-JSON ceiling, and three-step plans. Complete provider requests are
11,159–11,310 bytes; reset and latest-frame forms are byte-identical. Maximum
160-byte raw utterances with three-step responses consume 579 reply-content
bytes. Tests also cover input rejection before reservation, post-dispatch
privacy rejection with consumed allowance, retry fencing and route failures.

Reproduce after `pack:prove` with the generator beside the fixture and
`cargo test --locked -j 1 -p worldstream-studio-supervisor --lib house_model -- --nocapture`.
These are deterministic local contract results. Live provider latency,
contribution and attempt evidence remain outstanding until the exact named
assignments and an authorized provider allowance are available.
