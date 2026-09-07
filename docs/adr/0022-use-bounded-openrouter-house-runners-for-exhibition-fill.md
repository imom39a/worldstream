---
status: accepted
date: 2026-09-04
---

# Use bounded OpenRouter House Runners for exhibition fill

## Context

[ADR 0021](0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md)
allows an Activity Listing Revision to offer a Platform Operator-owned House
Agent for an eligible unclaimed seat. It intentionally leaves the pool, selection,
Runner, provider, allowance, secret, readiness, and failure contracts
unresolved.

The Hosted Activity Platform needs enough supplied participation for a hobby
deployment to demonstrate Agent Heist when invited people do not fill every
seat. That convenience must not turn WorldStream into a model gateway, give the
platform authority over Pack outcomes, create user-editable reusable agent
profiles, or expose provider credentials through Rooms, Packs, browsers, or
Supabase.

For the initial deployment, the project host separately holds both Platform
Operator and Host Operator roles. Platform publication, Host approval, and
secret provisioning remain separate acts even when the same organization
performs all three. A House Agent becomes an ordinary Agent Principal and
Participant from WorldStream's perspective; it is not a special kernel
participant kind.

## Decision

Offer one deliberately small **exhibition-only House Agent fill path**. The
Platform Operator publishes immutable House Agent Revisions and pays for their
model calls. A creator may opt into the published allowance but cannot set a
budget, provide a credential, edit a House Agent, select an arbitrary model,
or purchase execution.

OpenRouter is the MVP model gateway. It is used only behind a replaceable
`ProviderPort` inside an external House Runner. OpenRouter is not part of the
WorldStream protocol, Activity Pack contract, Activity Client contract, Room
state, or Replay contract.

### Immutable revisions and assignments

One House Agent Revision pins all of the following:

- one canonical OpenRouter model slug and one exact full provider slug;
- one Platform Operator-authored behavior policy revision;
- one exact Host-approved Agent Profile Revision and Runner Template Revision;
- the permitted tool set, which is empty in the MVP; and
- one exact local accounting-tokenizer revision; and
- the fixed execution allowance defined below.

The House Agent Revision resolves these existing Host records; it does not
replace or implicitly approve them. The Host independently verifies the Agent
Profile, Runner Template, executable digest, Pack compatibility, and named
credential reference. Catalog publication alone grants no execution approval.

The OpenRouter integration uses a new, narrow managed host-contract revision.
Its adapter has the OpenRouter HTTPS API origin and completion path compiled
in, rejects redirects and caller-selected upstreams, and exposes only the
request controls in this decision through an internal `ProviderPort`. It does
not relax the existing managed-reference adapter's loopback-only contract and
requires no kernel change.

An Activity Listing Revision allowlists exact House Agent Revisions for exact
eligible seats. The MVP pool contains two inexpensive, behaviorally distinct
revisions. One Run may receive at most two House Agent Assignments.

Selection first freezes the draw, selected revision, and stable Runner-capacity
reservation operation for each seat. After every retained reservation succeeds,
one completion transaction creates the new immutable, Launch Request-scoped
House Agent Assignments and fixes their server-derived setup-local Principal
references. It creates no partial assignment set. Room setup allocates the
actual run-scoped Agent Principals, which Genesis records in the Run Membership
Correspondences. Each assignment records the Listing Revision, Launch Request,
seat, selected House Agent Revision, exact model and full provider slug,
behavior-policy revision, Agent Profile Revision, Runner Template Revision,
execution allowance, selection evidence, and authenticated reservation receipt.
A terminal pre-Genesis failure creates no Assignment. An Assignment is not a
reusable Principal or mutable profile.

### Creator choice and fill timing

Launch Request creation offers exactly these modes when its Listing Revision
permits fill:

- `humans_only`; or
- `fill_with_house_agents` under the displayed Platform Operator-funded
  allowance.

The selected mode and allowance are visible to invitees and become immutable
after the first Seat Claim. Choosing fill does not create a Room or fill a seat
immediately. The creator may invite humans or their run-scoped browser agents
and later request **Start with House Agents**.

That request creates one durable fill operation and opens one final 30-second
human-claim window. When it closes, the operation takes the exact
still-unclaimed, House-eligible seats and the exact compatible, Host-approved,
available House Agent Revision candidate set. It selects without replacement
using operating-system cryptographic randomness and atomically retains the
candidate set, exclusion set, random draw, selected revisions, and capacity
reservation operation identities. Vercel reconciles those exact operations and
retains their authenticated receipts before one transaction creates the full
Assignment set. An identical retry resumes or reads this operation; an
ambiguous result is reconciled and never authorizes another draw. A selected
revision may appear only once in the launch lineage. This is auditable platform
coordination, not a claim of cryptographically fair matchmaking.

If the required eligible seats cannot be filled from the two-revision pool or
the required Runner capacity cannot be reserved, the Launch Request ends as
`failed_pre_genesis` and no Room is created. Otherwise, the platform freezes
the complete roster and House Agent Assignments before Genesis and maps the
Launch Request to its one Room Setup Operation under ADR 0021.

### Eligible Activities

House Agent fill is allowed only when the exact Activity Listing Revision
proves all of the following:

- every House Agent opportunity has a bounded response deadline;
- submitting no Action has an explicit Pack-defined consequence;
- the Invocation Context contains exact current Action Offers;
- the number and size of possible Activations are bounded; and
- the authorized Projection and Action Offers are suitable for transmission to
  the configured external model provider.

The Pack remains responsible for legality, deadlines, Activity Phase, and
Outcome. Agent Heist is eligible for the first House Agent proof. The current
Negotiate revision is not eligible until a later review proves these
properties and its safe Lobby-compatible start contract.

### Runner boundary and capacity

Each House Agent Assignment receives one separately executing, managed House
Runner unit on the same Fly Machine as the single WorldStream deployment. A
unit preserves the existing managed-agent-host split:

1. `worldstream-assignment-mcp` receives the exact assignment launch reference
   and resolves sealed Runner and participant authority for only that seat.
2. `worldstream-managed-agent-host` receives only the bridged MCP stdio channel,
   immutable behavior/model configuration, and OpenRouter credential. It
   receives no Controller path, Launch Request, Room ID, Membership ID,
   participant authority, or Runner authority.

Both processes run outside `worldstreamd`; same-Machine placement is a
hobby-deployment choice rather than a kernel contract. The deployment admits
at most four concurrent House Runner units and at most two in one Run.

Before Genesis, the platform reserves one unit for every selected assignment.
After Genesis, the Controller supplies the authority-holding helper with the
exact run-and-seat-scoped launch reference. The helper alone revalidates the
Activation, Action Offer, payload schema, Room Head, and participant authority
before submitting an Action. Neither scoped authority grants operator access
or access to another Room, Run, seat, or Membership. Runner authority alone
never authorizes an Action.

The OpenRouter bearer begins as a dedicated operating-organization-owned Fly
secret and is imported into the Controller's private named-credential vault.
Immediately before launch, the Controller resolves it and sends it only to the
authority-free model-host process through private stdin framing, then zeroizes
its local buffer. The secret is never stored in Supabase, a Room, an Activity
Pack, an Activity Client, a House Agent Assignment, a process argument, or a
log, and it is never given to `worldstreamd` or the assignment helper.
Management credentials used to create or rotate the key are not deployed.

Each Invocation starts without hidden retained conversation state. Its model
input consists only of:

- the immutable Platform Operator behavior policy;
- the current bounded, authorized Projection from its Invocation Context; and
- the exact current Action Offers.

The MVP supplies no tools, creator-written prompt, durable agent-private
memory, cross-Invocation conversation, or cross-Run memory. World and Pack
text is untrusted data and cannot change Runner policy, credentials, model
routing, allowance, or tool availability.

The model host accepts only one schema-valid proposed Action that matches one
current Action Offer and supplies only fields allowed by that offer. It rejects
arbitrary model-authored commands. The assignment helper independently
revalidates the proposal and submits it with the sealed participant authority
and current WorldStream operation and head fences.

### Hard execution allowance

Every House Agent Assignment receives this fixed hard allowance:

| Limit | MVP value |
| --- | ---: |
| Model-call attempts | 10 |
| Total input tokens | 120,000 |
| Total output tokens | 10,000 |
| Input tokens per call | 12,000 |
| Output tokens per call | 1,000 |
| Concurrent calls per assignment | 1 |
| Call timeout | 60 seconds |

These are platform accounting-token limits measured by the exact tokenizer
pinned in the House Agent Revision. They are distinct from OpenRouter's billing
units. Before dispatch, the model host proves that the complete serialized
input is within the per-call accounting limit and sets the provider's hard
completion-token limit. An oversized input fails without a provider call.

The maximum allowance for a Run is the sum of its assignments; there is no
separate creator-controlled pool. A Controller-owned durable allowance ledger
on the Fly volume, outside Room state and authoritative WorldStream storage,
atomically reserves one call attempt and the maximum per-call token exposure
before dispatch. The stable attempt identity derives from the exact Activation,
lease generation, and Invocation Context digest. An unambiguous completed
response reconciles the reservation conservatively against local counts and
provider-reported usage. A timeout, lost response, interrupted stream, or
otherwise ambiguous provider result consumes the full reservation. A crash or
protocol retry cannot restore consumed allowance or dispatch the model call a
second time. Supabase may later receive a non-authoritative usage mirror, but
it is not in the model-call admission path. Any such mirror is pulled and
written by the Vercel platform backend; Fly never receives a Supabase database
key.

The dedicated OpenRouter key and its guardrail also receive a small Platform
Operator-configured monetary limit as an aggregate blast-radius control. That
limit is not a precise per-Run escrow and does not replace the platform's
per-assignment token ledger. The exact aggregate monetary cap belongs to the
deployment contract.

### OpenRouter routing and privacy

Every request is non-streaming and uses one concrete canonical model slug and
one exact full provider slug, including any selected variant or region. The
model host never uses `openrouter/auto`, a `~...latest` alias, a model array, a
routing preset, or another mutable/fallback model reference. It sets the
equivalent of all of these routing constraints:

```json
{
  "model": "<exact-canonical-model>",
  "max_completion_tokens": 1000,
  "provider": {
    "order": ["<exact-full-provider-slug>"],
    "only": ["<exact-full-provider-slug>"],
    "allow_fallbacks": false,
    "require_parameters": true,
    "zdr": true,
    "data_collection": "deny",
    "max_price": {
      "prompt": "<operator-ceiling>",
      "completion": "<operator-ceiling>"
    }
  }
}
```

The request also sends `X-OpenRouter-Metadata: enabled` so the model host can
audit the router's reported selection. Its decoder ignores unknown additive
metadata fields and never treats metadata as Activity truth. Tools, plugins,
presets, routing variants, prompt transforms, response caches, and provider
sessions are absent.

The dedicated key is attached to an OpenRouter guardrail that allowlists only
the two exact model and provider routes. Account-level zero-data-retention is
enabled, provider data collection is denied, and request/response logging,
response caching, and data-use opt-ins remain disabled. If the exact route no
longer satisfies its provider, privacy, parameter, or price policy, the House
Agent Revision becomes unavailable; the model host does not substitute another
route.

Before accepting a model response, the model host requires and checks the
reported model, router metadata identifying exactly one eligible and selected
provider corresponding to the configured full provider slug, exactly one
successful upstream attempt, response schema, generation ID, and usage.
Missing, ambiguous, or mismatched routing evidence is a failed paid attempt and
produces no Action. Generation and usage metadata may later reconcile
operational cost, but neither OpenRouter metadata nor the platform ledger is
canonical Activity evidence.
Raw prompts, responses, and model reasoning are not retained in ordinary
platform logs; logs retain bounded identifiers, hashes, usage, cost, timing,
and typed failure metadata.

OpenRouter and its upstream providers remain third parties. These controls
record and constrain the declared route but do not cryptographically prove
immutable model weights or upstream execution. House-filled Runs therefore
remain exhibitions.

### Failure and Activity authority

After a model request may have left the process, there is no model-call
redispatch, provider retry, provider fallback, model fallback, model swap,
replacement House Agent, or selection reroll. This does not prohibit
idempotent Room Setup reconciliation, WorldStream protocol retries that submit
no new provider request, or restart of the same exact Runner unit and
assignment.

- Before Genesis, an unavailable exact Agent Profile Revision, Runner Template
  Revision, provider route, secret, guardrail, allowance ledger, or capacity
  ends the Launch Request as `failed_pre_genesis` and creates no Room.
- After Genesis but before the Pack leaves its Lobby, the same exact Runner and
  assignment may recover. If readiness does not return before the Listing
  Revision's pre-start Room deadline, the platform uses ADR 0021's bounded
  abandonment path. It does not substitute another assignment.
- After the Pack starts, a timeout, provider rejection, rate limit, transport
  ambiguity, invalid output, route mismatch, or Runner failure produces no
  invented Action. The Runner records one typed operational failure and the
  Pack's existing deadline and no-Action rules determine what happens.

The platform may display a non-authoritative `execution degraded` condition.
It never declares a generic forfeit, winner, score, or Outcome. A `failed`
Activation disposition is not a Pack result.

Every Run that contains a House Agent Assignment is permanently labelled
**Exhibition — platform-supplied agents** in its waiting room, live view,
result, and Replay entry. Public attribution identifies the exact immutable
House Agent Revision, declared provider/model route, and allowance, but never a
secret, prompt, provider response, or mutable model claim. A later integrity
failure suppresses the public result while retaining this internal assignment
evidence. It is excluded from verified benchmark or ranked claims unless a
future accepted decision defines stronger execution evidence. The MVP creates
no leaderboard.

## Consequences

- A mostly empty hobby deployment can demonstrate Agent Heist without asking
  a user to configure provider credentials or reusable agent profiles.
- The provider dependency, spend, policy, and secrets stay in a small external
  Runner boundary rather than expanding the kernel or Pack API.
- Fixed limits, no tools, no fallback, and Pack-defined no-Action behavior make
  failures bounded and explainable.
- A two-revision pool is intentionally too small for general matchmaking,
  broad model evaluation, or provider-independent benchmark claims.
- Same-Machine processes and a four-Runner cap are adequate only for the
  single-Machine hobby deployment.
- This decision does not authorize user-supplied credentials or prompts,
  reusable profiles, arbitrary hosted models, remote MCP, tool execution,
  persistent agent memory, paid execution, rankings, or a general agent-hosting
  service.

This decision extends
[ADR 0021](0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md)
and is indexed publicly only under the evidence rules in
[ADR 0023](0023-use-supabase-for-platform-coordination-and-replay-verified-results.md).
Its aggregate provider-spend and restored-assignment operating limits are
frozen by [ADR 0024](0024-operate-a-single-authority-hobby-preview.md).

## External references

The OpenRouter-specific controls were verified against its official
documentation on 2026-09-04. Implementations must preflight current behavior
and fail closed rather than assume these mutable service capabilities:

- [Provider routing and exact endpoint selection](https://openrouter.ai/docs/guides/routing/provider-selection)
- [Router metadata](https://openrouter.ai/docs/guides/features/router-metadata)
- [Model identities and canonical slugs](https://openrouter.ai/docs/guides/overview/models)
- [Guardrails](https://openrouter.ai/docs/guides/features/guardrails/overview)
- [Zero Data Retention](https://openrouter.ai/docs/guides/features/zdr)
- [API key limits](https://openrouter.ai/docs/api/api-reference/api-keys/create-a-new-api-key)
