# WorldStream as a Verifiable Agent-Negotiation Operator

> - **Status:** Non-normative product research
> - **Current as of:** 2026-08-29
> - **Repository baseline reviewed:** 046c876
>
> **Decision authority:** This document does not change the frozen WorldStream requirements, protocol, Activity Pack contract, or architecture. Any adopted product or technical change must go through the repository's normal ADR and specification process.

## Executive decision

There is a credible product here, but it is narrower than “a negotiation platform” and different from “procurement with AI.”

The recommended product is:

> **WorldStream Negotiate: an open-source, self-hosted operator for live, durable, human-governed negotiations between independently running agents.**

It should run a bilateral negotiation, deliver the correct private view to each party in real time, survive disconnects and process restarts, enforce deadlines and approval gates, and export a portable evidence bundle that both parties can verify.

WorldStream should **not** create another negotiation protocol. It should implement an existing protocol-compatible profile, initially [A202](https://github.com/a202-protocol/a202), on top of the WorldStream kernel. A2CN should be treated as the closest direct collision and a possible second adapter, not ignored or reimplemented.

The first product demonstration should be one bounded, AI-native agreement:

> A buyer agent and a provider agent negotiate a committed block of AI API or inference capacity: volume, unit price, duration, rate limit, p95 latency, uptime SLA, region, data-retention policy, delivery and payment schedule, and offer expiry. A human must approve material commitments.

This is a **wedge hypothesis**, not validated demand. It needs interviews with AI gateway, inference marketplace, agent-payments, and B2B agent builders.

The licensing recommendation is to keep the existing Apache-licensed kernel, the first negotiation pack, adapters, SDK examples, verifier, and deal-room UI open. If usage appears, sell operational convenience later: managed isolated deployments, bring-your-own-cloud operation, enterprise identity, key management, retention controls, and support. SaaS-only and premature open-core licensing would make adoption harder before the category is proven.

### What is actually novel

Agent negotiation itself is not novel. Several protocols, reference implementations, and enterprise products already exist. The least occupied and most defensible layer is:

> A production-oriented, protocol-compatible **negotiation session operator** with durable ordering, scoped per-party live streams, exact reconnect and replay, crash-safe idempotency, human approval gates, and portable agreement evidence.

That description matches capabilities WorldStream already has. It also names the gap that A202 explicitly leaves to an operator or carrier.

### Confidence

| Claim | Confidence | Basis |
|---|---:|---|
| Autonomous negotiation is a real enterprise category | High | Pactum, Keelvar, SAP, Oracle, Arkestro, and Luminance publicly ship or describe first-party products |
| A protocol-compatible operator is a real technical gap | High | A202 explicitly excludes the operator; A2CN and Shaket expose adjacent but much lighter runtimes |
| WorldStream has unusually relevant kernel primitives | High | Current code and specifications already implement ordering, private projections, durable actions, catch-up, activation, timers, replay, and SQLite/PostgreSQL persistence |
| Developers will adopt a neutral open operator | Low to medium | The relevant public protocols are early and fragmented; no strong independent demand signal was found |
| AI-capacity negotiation is the best initial commercial wedge | Low to medium | It is a coherent AI-native demonstration, but it remains an interview hypothesis |
| This should immediately become a hosted commercial product | Low | Trust, two-sided adoption, protocol churn, and the current single-process deployment boundary make that premature |

## Product boundary

### One-sentence description

WorldStream Negotiate is the durable room in which buyer and seller agents exchange signed structured offers, humans approve consequential actions, and both sides leave with a replayable agreement record.

“WorldStream Negotiate” is a working application name, not a repository or trademark decision. WorldStream remains the kernel; negotiation is the first focused product built on it.

### What WorldStream owns

- Authoritative session order and lifecycle.
- Admission of typed, authorized actions.
- Idempotent commit-before-acknowledgement.
- The current offer and immutable offer history.
- Scoped buyer, seller, approver, auditor, and operator projections.
- Real-time observations, cursors, reconnect, and explicit reset.
- Deadlines, expiry, and activation intents for external agents.
- Deterministic replay and venue-integrity lineage.
- Export of the exact included negotiation evidence.

### What parties own

- Their business goals, reservation values, concession policy, and negotiation strategy.
- Their agent runtime and model provider.
- Their commercial signing key or delegated signing service.
- Whether to accept an offer, subject to their human and policy controls.
- Independent retention and verification of receipts and agreement evidence.

### What WorldStream does not own

- Agent discovery or marketplace matching.
- Procurement intake, supplier master data, sourcing analytics, or spend management.
- A negotiation strategy model.
- Legal advice, contract drafting, or a claim that an export is legally binding.
- Custody, escrow, settlement, refunds, chargebacks, or delivery verification.
- Reputation scoring or a token economy.
- Fair-price determination, anti-collusion guarantees, or a neutral-arbiter claim.
- General workflow automation.
- User-authored Rust Activity Packs as an onboarding requirement.

### Why “operator,” not “protocol”

A protocol defines the meaning and shape of messages. An operator makes a live session work under failure: it decides which valid action committed first, records the result, produces the right view for each participant, resumes after a disconnect, fires deadlines, and proves what it included.

WorldStream's valuable contribution is the second job. The protocol landscape is already crowded enough that publishing another message vocabulary would reduce interoperability rather than create differentiation.

## Research method and limits

This review used only primary sources:

- official specifications and protocol websites;
- official source repositories and reference implementations;
- first-party product pages, documentation, and vendor publications;
- the WorldStream repository's current documentation and implementation.

The direct-protocol source audit used these repository snapshots:

| Project | Reviewed commit |
|---|---|
| A202 | [fa85aa8](https://github.com/a202-protocol/a202/tree/fa85aa8) |
| A2CN | [ff6305e](https://github.com/A2CN-protocol/A2CN/tree/ff6305e) |
| Shaket | [80c6cce](https://github.com/shaketlabs/shaket/tree/80c6cce) |
| OANP specification | [a23d793](https://github.com/oanp-protocol/spec/tree/a23d793) |
| OANP reference | [94d3533](https://github.com/oanp-protocol/reference/tree/94d3533) |
| ANP | [c0da079](https://github.com/ANP-Protocol/Agent-Negotiation-Protocol/tree/c0da079) |

Vendor metrics are identified as vendor-reported. Repository popularity counts are volatile snapshots and are not used as market-size evidence. “Gap” means a comparatively under-served layer in the reviewed public material, not proof that no private or unpublished competitor exists.

No customer interviews, paid-product trials, legal review, security audit, or performance benchmark were performed. This is product research, not market validation.

## Market answer: existing idea or viable opening?

Both statements are true:

1. **The broad idea exists.** Autonomous procurement negotiation, protocol-level offer exchange, signed receipts, auctions, and agent payments all have active implementations.
2. **WorldStream can still make a distinct contribution.** Most public protocols define semantics, reference libraries, or payment handoffs. Enterprise products are vertically integrated and closed. A reusable, self-hosted operator with strong live-session and failure semantics remains comparatively open.

The project should therefore compete on **operational semantics and developer experience**, not on being the first system where two agents counteroffer.

## Direct negotiation protocols and runtimes

### Comparison

| Project | What its primary sources show | Relationship to WorldStream | Product implication |
|---|---|---|---|
| A202 | Typed, signed commercial objects and transaction/session models; explicitly leaves venue, ordering, streaming, retries, and storage to an operator or carrier | Strongest semantic fit and clearest declared operator gap | Implement one narrow A202 operated-scope profile for a two-party session first |
| A2CN | End-to-end agent negotiation protocol plus FastAPI responder, session lifecycle, offers, approvals, timeouts, audit, and persistence adapters | Closest direct collision | Differentiate on crash-safe append-only operation, private live projections, reconnect/replay, evidence UI, and conformance |
| Shaket | A2A-compatible negotiation and reverse-auction prototype with event/state coordination and LLM hooks | Similar product shape, lighter operational substrate | Learn from its agent-facing ergonomics; do not copy an in-memory coordination model |
| OANP | Proposed neutral three-party arbiter that evaluates private constraints and produces a signed agreement | Adjacent fairness/arbiter model | Do not claim fairness or build an arbiter in the MVP |
| ANP | Seller-hosted price negotiation, Ed25519 receipt, and x402 testnet payment verification | Narrow working demonstration of negotiation-to-payment | Useful demo pattern; too price-only and payment-centric for WorldStream's wedge |
| MNP | Signed agent intent and negotiation ledger, with commerce-protocol handoff | Adjacent evidence/ledger framing | Watch the schema and ecosystem; public evidence is not yet enough to choose it as the first compatibility target |

### A202: the clearest operator-shaped gap

The [A202 repository](https://github.com/a202-protocol/a202) describes an Apache-licensed, pre-1.0 protocol for interoperable agent commerce. Its canonical model covers typed commercial authority, signed objects, transactions, bilateral sessions, disclosure, and replay/conformance concepts.

The most important product evidence is negative space. The [A202 reference implementation README](https://github.com/a202-protocol/a202/blob/fa85aa8/reference/README.md) explicitly says the reference is not an operator: it has no negotiation venue, session-ordering service, network server/client, or storage system. The [A2A carrier binding](https://github.com/a202-protocol/a202/blob/fa85aa8/bindings/a2a-binding-v0.1.md) delegates turn order, timeouts, cancellation, streaming, retries, push notifications, and connection lifetime to the carrier. The [canonical commercial model](https://github.com/a202-protocol/a202/blob/fa85aa8/schemas/canonical-commercial-model-v0.1.md) allows an operator to hold authoritative ordering while parties retain their own records.

That is almost a direct requirements list for WorldStream:

- A202 defines portable commercial meaning and party evidence.
- WorldStream supplies the live, durable operator.
- A2A can carry messages to and from agents.
- AP2, ACP, UCP, or x402 can take over after an agreement when appropriate.

The recommendation is not to advertise “full A202 support” before conformance is demonstrated. Build and publish a precisely named **A202 operated-scope profile for a two-party session**, list supported objects and transitions, and publish incompatibilities. “Bilateral” describes the product shape here; it must not be confused with A202's formal `a202-scope/bilateral/0.1`, which excludes an operator. Because WorldStream assigns the session order, any future conformance claim belongs under A202's [`a202-scope/operated/0.1`](https://a202.org/conformance/conformance-role-scopes-v0.1/).

### A2CN: the direct collision that must shape the design

[A2CN v0.2](https://github.com/A2CN-protocol/A2CN/blob/ff6305e/README.md) is the closest existing public project to the proposed product. Its first-party material covers discovery, mandates, signed offer/counter/accept messages, turn and round controls, timeouts, human-approval pauses, audit records, post-commitment activity, and a FastAPI responder. The repository reports 474 tests and describes the project as a protocol rather than a platform or SaaS. A GitHub metadata snapshot on 2026-08-29 showed 13 stars and one fork; those volatile counts show that this is still an early public ecosystem, not that the technical collision is unimportant.

The source review reveals a meaningful implementation boundary. In the reviewed v0.2 [FastAPI server](https://github.com/A2CN-protocol/A2CN/blob/ff6305e/reference-implementation/python/a2cn/server.py), the primary SessionManager is process-local. The configurable SessionStore is used for post-commitment data. The [SessionStore implementations](https://github.com/A2CN-protocol/A2CN/blob/ff6305e/reference-implementation/python/a2cn/session_store.py) save whole JSON session values to memory, Redis, or PostgreSQL; they are not an append-only, compare-and-set, commit-before-ack session operator.

This does **not** make A2CN weak. It means its public reference implementation optimizes for protocol usability, while WorldStream can provide stronger operational behavior:

- atomic transition plus observation-frame persistence;
- operation identity and retry resolution;
- exact-head conflict detection;
- per-party scoped projections and cursored streams;
- process restart and deterministic replay evidence;
- timers and durable external-agent activation;
- SQLite and PostgreSQL profiles with the same room semantics;
- an operator and participant deal-room UI.

A2CN's roadmap includes neutral third-party record custody, so this overlap may grow. WorldStream should track it and seek interoperability rather than pretend the collision does not exist.

### Shaket

[Shaket](https://github.com/shaketlabs/shaket) presents an A2A-compatible negotiation layer supporting bilateral negotiation and reverse auctions, with state/event coordination and LLM integration hooks. The reviewed reference state manager uses in-memory collections, and its roadmap still lists signed commitments and formalized A2A behavior.

Shaket is evidence that agent developers want an ergonomic negotiation runtime. It also reinforces the WorldStream differentiation: operational durability, private projection boundaries, exact reconnect, and failure semantics. Reverse auctions should be a later profile, only after bilateral negotiation is useful.

### OANP

The [Open Agent Negotiation Protocol](https://oanp.dev/), its [specification repository](https://github.com/oanp-protocol/spec), and its [reference repository](https://github.com/oanp-protocol/reference) frame negotiation as a three-party process with a neutral arbiter, private constraints, and signed output.

The reviewed public spec repository still presents placeholder publication material, and the reference is an in-process implementation rather than an operated service. More importantly, neutral optimization is a different product promise. WorldStream can order and record actions without knowing either party's reservation value. It should not call the operator a fair-price arbiter unless a separately reviewed mechanism can substantiate that claim.

### ANP

The [Agent Negotiation Protocol repository](https://github.com/ANP-Protocol/Agent-Negotiation-Protocol) demonstrates seller-hosted price bargaining, a signed Ed25519 negotiation receipt, and an x402 payment-verification path. Its own documentation says the current payment step verifies but does not itself settle and does not supply delivery verification, escrow, or dispute resolution.

ANP is a useful example of a small end-to-end demo. WorldStream should borrow the clarity of “negotiate, produce receipt, optionally pay,” but the proposed AI-capacity agreement needs structured non-price terms and human approvals.

### MNP

The [Market Negotiation Protocol](https://marketnegotiationprotocol.org/) and its [protocol page](https://marketnegotiationprotocol.org/protocol) describe signed agent intent, negotiation records, and handoff into agent-commerce infrastructure.

The reviewed public material is primarily a website, schema, and white-paper presentation. It is relevant to monitor, especially for portable evidence, but it does not currently provide enough public operator implementation evidence to displace A202 as the first integration choice.

## Adjacent protocols: compose, do not compete

| Layer | Primary source | What it supplies | What it does not supply |
|---|---|---|---|
| Agent transport | [A2A protocol](https://github.com/a2aproject/A2A/blob/main/specification/a2a.proto) | Agent Cards, capabilities, tasks, messages, artifacts, streaming, and push | Commercial offer semantics or durable negotiation authority |
| Agent tools/context | [MCP architecture](https://modelcontextprotocol.io/specification/2025-06-18/architecture) | Host/client/server model with tools, resources, and prompts | Counterparty agreement state |
| Payment authorization | [AP2 specification](https://github.com/google-agentic-commerce/AP2/blob/main/docs/ap2/specification.md) | Checkout and payment mandates and receipts | General multi-term bargaining or an operated negotiation session |
| Commerce checkout | [ACP repository](https://github.com/agentic-commerce-protocol/agentic-commerce-protocol) and [capability negotiation](https://www.agenticcommerce.dev/docs/concepts/capability-negotiation) | Product, cart, checkout, order, and payment-handler flows | “Negotiation” there is technical capability intersection, not buyer/seller counteroffers |
| Universal commerce | [UCP core concepts](https://github.com/Universal-Commerce-Protocol/ucp/blob/main/docs/documentation/core-concepts.md) | Discovery and intersection of commerce capabilities, with A2A/MCP/AP2 composition | A live commercial bargaining operator |
| HTTP payment | [x402 introduction](https://docs.x402.org/introduction) | HTTP-native payment-required flow, commonly for digital resources and APIs | Negotiation, escrow, delivery, or session history |
| Agent registries | [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) | Draft identity, reputation, and validation registries | Negotiation or payment |
| Escrowed jobs | [ERC-8183](https://eips.ethereum.org/EIPS/eip-8183) | Draft job, budget, provider, evaluator, submission, completion/rejection, and escrow lifecycle | Multi-round term negotiation |

The clean stack is:

    A2A or MCP                    agent-facing access
            ↓
    A202-compatible objects      commercial meaning and party proof
            ↓
    WorldStream Negotiate        live order, state, privacy, recovery, replay
            ↓
    AP2 / ACP / UCP / x402       checkout, payment, or commerce handoff
            ↓
    ERC-8004 / ERC-8183          optional registry, validation, or escrow

Crypto should remain an optional composition layer, not the product thesis. A token is unnecessary. On-chain anchoring is also unnecessary for the MVP and would not by itself solve identity, legal authority, privacy, delivery, or operator equivocation.

## Enterprise incumbents

### What they validate

- [Pactum](https://pactum.com/) markets autonomous procurement negotiation and [procurement agents](https://pactum.com/procurement-agents) that negotiate multiple terms with suppliers. Its customer and savings figures are vendor-reported.
- [Keelvar](https://www.keelvar.com/product-overview) provides sourcing automation, bids, awards, and autonomous agents. Its 2026 publication on the [two-sided agentic procurement market](https://www.keelvar.com/documents/the-two-sided-agentic-market-in-enterprise-procurement) reports machine-to-machine supplier bidding activity; those activity figures are vendor-reported.
- [SAP's sourcing negotiation agent](https://www.sap.com/india/use-cases/joule-assistant/sourcing-assistant) creates negotiation strategies and counteroffers within the SAP procurement environment.
- [Oracle's autonomous sourcing and award assistants](https://docs.oracle.com/en/cloud/saas/readiness/scm/26a/proc26a/26A-procurement-wn-f41767.htm) automate sourcing-event and award work inside Oracle Procurement.
- [Arkestro](https://arkestro.com/wp-content/uploads/ArkestroPredictiveProcurementDatasheet.pdf) markets predictive procurement using game-theory-informed guidance.
- [Luminance](https://www.luminance.com/autonomous-negotiation/) applies AI to contract-document negotiation and redlining.

These products establish that buyers will use software in negotiation and sourcing. They do **not** validate demand for a neutral open-source operator.

### Where not to compete

WorldStream should not initially replace Pactum, Keelvar, SAP Ariba, Oracle Procurement, Coupa, Arkestro, or a contract-lifecycle platform. Those products have supplier networks, spend data, enterprise integrations, category models, approval policies, and services that a hobby project cannot credibly reproduce.

The better relationship is infrastructure or OEM:

- a procurement product could embed WorldStream as a bilateral session engine;
- a commerce-agent team could use it as its protocol-compatible reference venue;
- a standards project could use it for conformance and failure testing;
- an enterprise could self-host it for a bounded two-party pilot.

## Why WorldStream fits

### Current kernel assets

The repository describes and implements a self-hosted Rust room runtime where an Activity Pack owns deterministic activity rules and WorldStream owns core room status, membership, ordering, integrity, persistence, scoped observations, catch-up, activation, recovery, and replay. See the [README](../README.md), [architecture](architecture.md), [wire protocol](protocol.md), and [Activity Pack contract](activity-packs.md).

The current implementation already contains:

- an authoritative per-room actor and exact-head admission;
- operation identities and idempotent action resolution;
- commit-before-acknowledgement;
- canonical state and transition hashing with BLAKE3;
- public, participant, and operator projections;
- durable observation frames, cursors, catch-up, and reset;
- room and runner WebSocket streams;
- timers and terminal outcomes;
- durable activation intents for external agent runners;
- historical replay;
- bundled SQLite and PostgreSQL 17 storage profiles;
- a Python SDK;
- an operator server, Studio supervisor, and first-party web UI.

These are not aspirational negotiation requirements. They are visible in the current [server routes](../crates/worldstream-server/src/lib.rs), [room commit model](../crates/worldstream-core/src/room_commit.rs), [ActivityPackV1 implementation surface](../crates/worldstream-core/src/activity_pack.rs), [activation model](../crates/worldstream-core/src/activation.rs), and storage crates.

### Product fit

| Negotiation need | Existing WorldStream primitive | Remaining product work |
|---|---|---|
| One authoritative offer order | Room sequence and atomic transition commit | Negotiation transition rules |
| Duplicate/retry safety | Operation identity and stored resolution | Agent SDK ergonomics and protocol mapping |
| Buyer/seller confidentiality | Membership-scoped projections and observations | Negotiation-specific projection schemas and tests |
| Live participation | Cursored WebSocket room stream | Deal-room UI and agent adapter |
| Sleeping/ephemeral agents | Durable activation intents and reconnect catch-up | Negotiation activation reasons and runner examples |
| Offer expiry | Durable timers and host-clock policy | Negotiation deadlines |
| Human approval | Roles, action offers, attention, scoped views | Approval state and exact-digest binding |
| Audit and recovery | Replay, snapshots, transition lineage, SQLite/PostgreSQL | Portable verifier and agreement export |
| Self-hosting | Single Rust process plus SQLite/PostgreSQL | One-command negotiation quickstart |

### Important current gaps

The kernel is credible; the product layer is not yet present.

1. **No negotiation Activity Pack exists.** The current Counter example demonstrates mechanics, and Agent Heist demonstrates richer semantics, but neither is a useful negotiation product.
2. **Pack authoring is a trusted Rust release concern.** The [Activity Pack specification](activity-packs.md) explicitly says ActivityPackV1 is not a dynamic public plugin ABI. Users must not be asked to write Rust to start a negotiation. The first negotiation pack must ship built in.
3. **Transport identity is not commercial authority.** Current bearer/capability authorization controls who may call WorldStream. It does not prove that a buyer or seller signed a commercial offer under a mandate.
4. **No protocol adapter exists.** The current assignment MCP surface serves Studio task setup; it is not an A202/A2CN commercial interface.
5. **No exact commercial evidence format exists.** A protocol-compatible signed object must preserve its exact canonical bytes and signature, not merely a semantically similar reserialized JSON value.
6. **The generic artifact subsystem is not yet an available product surface.** The architecture specifies artifact metadata and lifecycle concepts, but the reviewed server routing does not expose general participant artifact upload/download endpoints. The MVP should not assume those endpoints already exist.
7. **No deal-room UI exists.** Studio has room and runner operations, not a terms diff, offer timeline, approval queue, and agreement export.
8. **Deployment is intentionally one WorldStream process.** The [architecture](architecture.md) does not authorize a multi-node globally available service. Managed-product claims must stay within that boundary until a later ADR.
9. **No malicious-operator non-equivocation proof exists.** BLAKE3 lineage is valuable integrity evidence, but a database hash chain alone is not a public transparency log or neutral timestamp.
10. **No legal-contract claim is supportable.** Identity, authority, applicable law, e-signature formalities, and external contract systems remain outside the kernel.

## Proposed product design

### Initial user and job

**Primary user:** an agent-commerce, AI-platform, or B2B agent developer who controls or can coordinate both endpoints of an early pilot.

**Job to be done:**

> “Let two independently running agents negotiate structured commercial terms under human and policy controls, recover safely from failures, and give me evidence of exactly what was offered and accepted.”

Secondary users are:

- procurement, CPQ, marketplace, and CLM product teams evaluating an embeddable engine;
- standards maintainers needing an operated reference and conformance venue;
- security or platform teams needing a self-hosted record of agent commitments.

The initial target is not a procurement department attempting to replace its suite, a consumer crypto marketplace, or a developer expected to author a custom Activity Pack.

### First vertical profile: AI/API capacity commitment

A posted per-call price usually does not need negotiation. A high-value capacity commitment can.

Example:

- A buyer wants 8 billion input tokens and 2 billion output tokens per month for three months.
- It needs a stated region, maximum burst rate, p95 latency, uptime SLA, and no training on submitted data.
- The provider can reduce price if the buyer accepts a longer term, lower burst ceiling, different region, or less aggressive latency.
- Each side has private constraints that never enter WorldStream.
- Agents exchange typed bundles, not prose.
- An acceptance beyond a configured spend or risk threshold pauses for a named human approver.
- When required approvals commit, both parties receive an agreement evidence bundle.
- A payment or contract system may consume that bundle later.

Candidate terms:

| Term | Representation guidance |
|---|---|
| Service | Fixed profile identifier and version |
| Committed volume | Integer amount plus exact unit and period |
| Overage | Allowed/forbidden, integer minor-unit price, and cap |
| Currency and price | ISO currency plus integer minor units; never binary floating point |
| Duration | Start/end or fixed number of periods |
| Rate limit | Integer requests/tokens per exact time unit |
| Latency | Exact percentile, threshold, unit, measurement window, and remedy reference |
| Availability | Basis points or millionths plus measurement window |
| Region | Closed list of allowed deployment regions |
| Data policy | Closed flags for retention duration, training use, and logging |
| Delivery schedule | Structured capacity activation milestones |
| Payment schedule | Structured installments or external payment-profile reference |
| Offer expiry | Trusted server deadline |
| Contract template | Versioned identifier and digest; WorldStream does not fetch arbitrary URLs |

This profile is deliberately structured and narrow. Free-form contract redlining belongs in a CLM system. The MVP can attach a fixed template identifier and digest, but its negotiation state should consist of fields the pack can validate deterministically.

### Roles

- Buyer agent
- Seller or provider agent
- Buyer approver
- Seller approver
- Read-only auditor
- Operator membership

One human may hold an approver role, and an agent may be the active buyer or seller participant. WorldStream's existing separation between principal kind, role, membership, and action authority should remain.

### Candidate lifecycle

The exact lifecycle must map to the chosen A202 operated profile rather than inventing incompatible terms. A plausible internal sequence is:

1. Draft
2. Open
3. Active
4. Paused for buyer approval, seller approval, or both
5. Accepted
6. Declined, withdrawn, expired, or failed
7. Exported or handed off

Terminal status must never imply payment, delivery, or legal enforceability. “Accepted” means the required in-room signed offer and approvals committed. An exported object should initially be called a **Provisional Agreement Record** or **Agreement Evidence Bundle**, not a contract.

### MVP containment rule

For the first product, **one WorldStream Room equals one transaction with one buyer–seller negotiation session**.

WorldStream currently gives a Room one total authoritative sequence. That is a strong match for race-safe bilateral negotiation, but it is the wrong privacy boundary for several competing suppliers: even if projections hide proposal contents, shared head and sequence movement can reveal that rival activity occurred. A later multi-supplier RFQ should use one private Room per buyer–supplier session plus a buyer-private aggregate service, or require an explicitly designed multi-stream kernel. Both options are later architecture work, not hidden MVP scope.

### Candidate actions

There is an important vocabulary boundary. WorldStream Core already uses **Action Offer** for the pack-generated representation of an action a member is currently allowed to take. In the product UI and native negotiation model, call a commercial offer a **Proposal Revision**. A protocol adapter may map that object to A202's protocol term **Offer**, but the two concepts must not share one internal name.

- Submit initial offer
- Counter an exact prior offer
- Accept an exact current offer
- Decline
- Withdraw own current offer
- Request or cancel approval where the policy allows
- Approve or reject an exact pending action digest
- Expire by trusted timer
- Close after evidence export or external handoff acknowledgment

Important invariants:

- Every offer is immutable and versioned.
- Every counter references the exact offer it supersedes.
- Accept binds the exact canonical offer digest, never “the current offer” by implication.
- Approval binds the exact pending action and policy revision.
- A stale accept or approval fails closed.
- A retry with the same operation identity resolves to the same committed result.
- Only protocol-defined public fields enter party-visible projections.
- Private reservation values, system prompts, chain-of-thought, and concession policies never enter room state, logs, activation context, or export.

### Agent behavior

The kernel should remain model-independent:

- WorldStream activates an external agent only when that membership has a reason to act.
- The activation context contains only the authorized projection and exact action offers.
- The agent proposes a typed action.
- A deterministic policy/signing boundary validates or authorizes the commercial action.
- WorldStream commits or rejects it.
- If the agent disappears, its activation can be retried; if it reconnects, the cursor restores its authorized view.

An LLM should not directly hold a raw commercial private key in the demonstration. A small policy/signing service should enforce mandate limits, schema validity, and approval requirements before signing.

### Human participation

Humans are first-class members, not spectators bolted onto an agent loop.

The deal room should let a human:

- inspect the current structured terms and differences from the prior offer;
- see which party acted and which signature was validated;
- approve or reject the exact pending digest;
- pause or close the room when authorized;
- inspect deadline and connection state;
- replay the session;
- download and independently check included evidence.

The UI must not present model reasoning as authoritative evidence. It may summarize terms, but the signed structured values and digest are the source of truth.

## Architecture

### Logical components

    Buyer agent / Buyer approver       Seller agent / Seller approver
                   \                     /
                    A2A, MCP, REST, or SDK
                              |
              protocol ingress and signature validator
                              |
              built-in bilateral negotiation pack
                  terms, offers, counters, approvals
                              |
                     WorldStream kernel
             order, commit, views, streams, timers,
                activation, recovery, and replay
                              |
                   SQLite or PostgreSQL 17
                              |
                Deal-room UI and evidence verifier
                              |
           optional AP2 / ACP / UCP / x402 / CLM handoff

### Keep protocol, operator, and settlement separate

| Plane | Authority | Evidence |
|---|---|---|
| Party/commercial plane | The buyer or seller key and mandate | Exact protocol-canonical bytes, party signature, party identity/key reference, and mandate reference |
| Venue/ordering plane | The WorldStream room operator | Room sequence, transition/state hashes, operation receipt, observation sequence, and replay |
| Payment/fulfilment plane | External commerce, payment, escrow, or contract system | External mandate, payment receipt, escrow event, order, or signed contract |

This separation prevents two dangerous claims:

- A party signature proves authorship of the signed object, not that the venue delivered every message or ordered it honestly.
- A WorldStream BLAKE3 lineage proves consistency relative to the retained/exported lineage, not that a buyer or seller consented to a commercial term.

### Two-layer cryptographic design

For the first compatibility profile:

1. Preserve the **exact protocol canonical bytes** for every signed commercial object.
2. Verify the protocol-specified digest and signature against a key pinned to the party in room Genesis.
3. Store the signed bytes or a lossless encoding in the committed transition/event evidence under strict size bounds.
4. Include their digest in the negotiation state.
5. Let the WorldStream transition commit add its existing canonical JSON and BLAKE3 lineage.
6. Export both layers without converting one into the other.

For A202 or A2CN objects using JCS-style canonical JSON and SHA-family digests, those bytes and hashes remain party evidence. WorldStream must not silently replace them with its BLAKE3 state hash. Likewise, a valid party signature must not bypass the room's exact-head, authorization, or transition rules.

For MVP key management, pin public keys at room creation and reject rotation. Key discovery, decentralized identifiers, certificates, organizational authority, revocation, and rotation need a later identity design. Existing WorldStream bearer tokens remain transport/action capabilities, not commercial signatures.

### Exact signed bytes without a generic artifact dependency

The smallest safe MVP path is:

- set a strict maximum signed-object size;
- carry the exact signed canonical object as a lossless bounded byte encoding in the action;
- validate it in trusted deterministic first-party code against the Genesis-pinned key;
- persist it through the transition/event record;
- reconstruct the evidence bundle from authorized replay.

This avoids depending on a not-yet-exposed general artifact service. If the existing transition record does not preserve those bytes exactly, the implementation must add a narrowly scoped signed-object evidence store or event field before claiming verifiable export. It should not introduce a generic blob marketplace.

### Protocol adapter boundary

The adapter should be thin:

- map an A202 object into one specific pack action;
- validate envelope/canonicalization/signature rules;
- preserve exact bytes;
- map WorldStream receipts and projections back into the supported protocol binding;
- expose a declared compatibility matrix.

It should not create a universal negotiation intermediate language in v0.1. Generalizing across A202, A2CN, OANP, and every commerce protocol before one useful flow exists would reproduce the abstraction problem the product is trying to escape.

### A2A, MCP, REST, and SDK

- **REST/SDK** is the canonical low-friction developer path and test surface.
- **MCP** should expose bounded tools such as inspect negotiation, list legal actions, submit signed offer, approve exact digest, and export evidence. It is an agent tool surface, not the commercial protocol.
- **A2A** should carry protocol messages and status/artifact updates for independently hosted agents.
- **WebSocket** remains WorldStream's live observation and activation transport.

The same room transition must result regardless of ingress. Adapters submit ordinary authenticated actions; they do not mutate state independently.

### Evidence bundle

A versioned evidence export should include:

- room and pack identity, semantic revision, and Genesis digest;
- participant identifiers and pinned public-key references;
- exact signed offer/counter/accept objects in committed order;
- approvals and the exact digests they bind;
- deadline and timer outcomes;
- WorldStream sequence, transition hashes, and final state root;
- protocol compatibility profile and verifier version;
- external handoff references, if any;
- an explicit statement of what is **not** proven.

An offline verifier should check canonical encoding, party signatures, reference chains, approval bindings, and WorldStream lineage without contacting the operator.

The verifier can prove that the included records are internally consistent and correctly signed. It cannot by itself prove that the operator did not withhold a different branch, censor a message, lie about wall-clock time, or operate under valid legal authority. Stronger non-equivocation would need party-retained receipts, cross-party receipt comparison, a transparency log, or an external timestamp/anchor. That is post-MVP.

### UI model

The first UI should be a purpose-built deal room, not generated UI and not a generic pack renderer.

Recommended layout:

| Area | Content |
|---|---|
| Header | Parties, session state, selected protocol profile, deadline, integrity status |
| Current terms | Typed term table with prior-value diff and validation status |
| Timeline | Offers, counters, approvals, withdrawals, expiry, and external handoffs |
| Action panel | Only exact currently legal actions for the signed-in membership |
| Approval inbox | Pending action digest, material changes, policy reason, approve/reject |
| Participant status | Human/agent role, connected/disconnected, last authorized sequence |
| Evidence | Replay, signature checks, final digest, export and offline verifier |

LLM-generated summaries may help humans read an offer, but should always be labeled derived and link back to exact structured terms.

## MVP definition

### Product promise

> In under fifteen minutes, a developer can start one local command, connect two independently running sample agents and one human approver, complete an AI-capacity negotiation, kill and restart the operator during the session, reconnect without duplicate actions, and verify the exported agreement evidence without writing Rust.

### Included

- One release-bundled bilateral negotiation pack.
- One exact AI/API capacity term schema.
- One precisely documented A202-compatible operated profile.
- Buyer, seller, approver, auditor, and operator roles.
- Signed immutable offers, counters, exact acceptance, withdrawal, decline, and expiry.
- One simple approval policy: require human approval above exact spend/risk thresholds.
- Deterministic sample buyer and seller agents.
- Optional LLM-driven sample agents behind a policy/signing boundary.
- REST/Python quickstart.
- Small MCP agent tool surface.
- A2A carrier example after the core quickstart works.
- Purpose-built Studio deal room.
- Evidence export and offline verifier.
- SQLite default and existing PostgreSQL profile.
- Crash/retry/replay demonstration and conformance tests.

### Explicitly excluded

- A generic visual room builder.
- User-authored or dynamically loaded packs.
- Auctions or multi-supplier sourcing.
- General contract redlining.
- Supplier discovery and marketplace matching.
- Spend analytics, procurement intake, ERP, or CLM replacement.
- Built-in negotiation strategy or “best price” claims.
- Wallet custody, escrow, settlement, delivery verification, or dispute resolution.
- Token issuance, staking, or reputation.
- Neutral fairness arbitration.
- Multi-region or multi-process WorldStream.
- General protocol translation across every negotiation standard.

### Acceptance tests

1. Two agents running in separate processes complete the flow through public interfaces.
2. Killing WorldStream after durable commit but before reply, then retrying, yields the original committed result.
3. A stale counter, accept, or approval is rejected.
4. A disconnected party resumes from its cursor or receives an explicit reset without private data leakage.
5. Buyer and seller projections reveal only the defined fields.
6. Reservation values and prompts do not appear in state, logs, observations, activation context, replay, or export.
7. A deterministic replay produces the same final agreement digest.
8. The offline verifier validates exact party signatures and venue lineage.
9. Corrupting any included signed object, reference, approval, or lineage link makes verification fail.
10. A fresh developer completes the quickstart in less than fifteen minutes without compiling or authoring an Activity Pack.

## Roadmap for a serious hobby project

The time ranges assume one experienced part-time developer and intentionally reuse the kernel.

### Stage 0: product and protocol spike — 1 to 2 weeks

- Write a product ADR that preserves the kernel boundary and names the negotiation application.
- Threat-model party, operator, runner, model, approver, signing key, and external handoff.
- Map the exact A202 bilateral objects and lifecycle to one candidate pack on paper.
- Build a throwaway compatibility spike for canonical bytes and signature verification.
- Confirm that committed transition evidence can preserve exact signed bytes.
- Interview at least eight relevant builders in parallel.

**Gate:** If mapping the supported A202 profile needs a generalized protocol engine or core semantic change before one session works, stop and reduce the profile. If the obstacle is fundamental, compare A2CN as the first adapter rather than inventing a new protocol.

### Stage 1: useful local product — 4 to 6 weeks

- Implement the built-in AI-capacity bilateral pack.
- Implement deterministic buyer and provider agents.
- Add the purpose-built deal-room UI.
- Add human approval, expiry, and scoped views.
- Export a local deterministic evidence bundle.
- Ship one-command SQLite quickstart and scripted restart demo.

**Gate:** An external developer must complete the quickstart with no Rust knowledge.

### Stage 2: protocol-compatible evidence — 4 to 6 weeks

- Finish the declared A202 operated profile.
- Add pinned party keys, exact signed-object storage, and offline verification.
- Publish a compatibility and conformance matrix.
- Add bounded MCP tools and a Python SDK example.
- Add A2A carrier integration for independently hosted agents.
- Add negative tests for replay, stale approval, signature substitution, and projection leakage.

**Gate:** Two independent agent processes must complete, reconnect to, and verify a session.

### Stage 3: public reference venue — 4 to 6 weeks

- Publish a public demo deployment within the current single-process support boundary.
- Demonstrate a crash between commit and reply.
- Add an optional AP2 or x402 sandbox handoff after agreement.
- Publish an evidence verifier and recorded failure walkthrough.
- Work with an A202 or A2CN maintainer on a real interoperability issue or fixture.

**Gate:** At least two external teams request or attempt integration for a real project, not only watch the demo.

### Stage 4: only after validation

- Add A2CN compatibility if user demand justifies the second adapter.
- Consider a reverse-RFQ or multi-supplier pack only after bilateral usage.
- Add enterprise identity, key rotation, retention policy, and external KMS/HSM.
- Harden managed isolated deployment and bring-your-own-cloud operation.
- Consider party receipt gossip or a transparency checkpoint for stronger non-equivocation.
- Decide whether a separate portable/untrusted pack ABI is warranted; negotiation does not itself justify one.

## Open source and commercial options

### Recommended: open product, paid operations later

Keep open under Apache 2.0:

- WorldStream kernel;
- the bilateral negotiation pack;
- protocol adapter and compatibility tests;
- SDK and sample agents;
- deal-room UI;
- evidence format and offline verifier;
- local SQLite and existing PostgreSQL support.

Possible paid offerings after demand:

- managed isolated WorldStream instances;
- bring-your-own-cloud deployment and upgrades;
- enterprise SSO/RBAC and organization policy;
- managed key integration, KMS/HSM, and rotation;
- retention, legal hold, export, and audit controls;
- backups, observability, incident support, and SLA;
- private ERP, procurement, commerce, or CLM connectors;
- interoperability and deployment support.

This preserves trust and adoption while leaving a credible business around operations.

### Open core

Open core could reserve enterprise controls or a hosted control plane, but it is premature. Before the project has users, a license boundary creates confusion about whether the verifier, protocol adapter, UI, or security features are trustworthy and complete. Revisit only after repeated enterprise requests reveal a natural operational boundary.

### SaaS-only

SaaS-only is the weakest starting option:

- both parties must trust an unknown operator;
- the current architecture is one process, not a multi-tenant HA control plane;
- security, privacy, key management, retention, and legal posture are not ready;
- distribution is the unsolved problem, not hosting.

A small public sandbox is useful for a demo. It should not be presented as production custody.

### Portfolio outcome

Even without commercial pull, this can be a strong portfolio project if it demonstrates:

- protocol interoperability rather than a proprietary vocabulary;
- hard failure semantics under real process restarts;
- multi-party privacy and action authorization;
- human governance of model-proposed commitments;
- independently checkable signed evidence;
- a polished no-Rust quickstart and deal-room experience.

Those are more credible AI-application systems-design signals than another agent chat UI or workflow wrapper.

## Adoption and go-to-network plan

### Positioning

Use:

> “The open operator for verifiable agent negotiations.”

Avoid:

- “AI procurement replacement”
- “agents negotiate everything”
- “fair-price oracle”
- “decentralized Fiverr for agents”
- “blockchain negotiation protocol”
- “generic multiplayer context layer”

### First audiences

1. A202 and A2CN implementers and standards contributors.
2. AI gateway, inference marketplace, and model-routing builders.
3. Agent-payment and commerce-protocol builders.
4. Procurement/CPQ/CLM engineering teams interested in embedding, not immediate suite replacement.
5. Agent framework maintainers who need a durable external session.

### Launch artifacts

- A five-minute video: two agents counteroffer, a human approves, the operator is killed, the session recovers, and the evidence verifies.
- A less-than-fifteen-minute local quickstart.
- A public A202 compatibility matrix and conformance report.
- A deterministic transcript fixture and offline verifier.
- MCP, A2A, REST, and Python examples that all drive the same session.
- A technical article explaining party proof versus venue proof.
- A direct comparison with A2CN that is respectful, source-based, and reproducible.

### Community behavior

- Contribute protocol ambiguities and fixtures upstream.
- Ask maintainers to review mappings before marketing conformance.
- Treat A2CN and Shaket as potential integrations or peers, not enemies.
- Prefer one excellent reference profile over a catalog of half-supported packs.
- Measure completed external quickstarts and integrations, not stars alone.

## Risks and mitigations

| Risk | Why it matters | Mitigation |
|---|---|---|
| No user needs negotiation | Posted pricing or ordinary checkout may be enough | Start with multi-term, high-value commitments; interview users; apply kill criteria |
| Protocol fragmentation | A202, A2CN, MNP, OANP, ANP, and commerce protocols may change | Support one named profile; preserve internal kernel boundary; publish compatibility |
| A2CN closes the operator gap | It is the strongest direct collision and may add custody | Differentiate now on operational rigor and UI; seek adapter/conformance collaboration |
| Two-sided adoption | Both counterparties must connect or accept an operator | Target teams controlling both endpoints and OEM integrations first |
| Operator trust | A central operator can censor, delay, withhold, or potentially equivocate | Signed party objects, party-retained receipts, self-hosting, explicit threat model; stronger transparency later |
| Commercial authority | An agent may exceed its mandate | Pinned identity, deterministic policy/signing boundary, exact human approval |
| Key compromise | A valid signature can still be unauthorized in the real organization | External KMS/HSM and revocation later; no legal guarantee |
| Privacy leakage | Reservation values or prompts would destroy trust | Never accept them; typed actions; projection tests; log and activation-context audits |
| Prompt injection/model error | An LLM may submit harmful terms or wrong references | Closed schema, exact action offers, policy validator, digest-bound approvals, no raw signing key |
| Legal overclaim | A signed record may not be an enforceable contract | Call it evidence; integrate external CLM/e-sign; obtain legal review before commercial claims |
| Fairness/collusion/regulation | Automated pricing can create antitrust and discrimination risk | Do not optimize across competitors, claim fairness, or share private constraints; scoped legal review |
| Procurement-suite scope | Integrations and category intelligence would swamp the project | Stay an operator/OEM layer and one AI-native profile |
| Current deployment ceiling | One process is not global HA or multi-tenant SaaS | Self-hosted/sandbox first; no unsupported availability claims |
| Pack development cost | A trusted Rust pack is not user-friendly | Bundle exactly one product pack; users configure terms and roles, not code |
| Evidence size and exactness | JSON reserialization can invalidate signatures | Preserve exact bounded canonical bytes and verify them offline |
| Settlement creep | Crypto/payment work can consume the project | Export/handoff only; no custody or token |

## Validation plan and kill criteria

### Interview questions

Ask builders for a recent or planned workflow, not whether the idea sounds interesting:

1. Which commercial terms would two agents actually bargain over instead of reading a posted price?
2. Who controls each agent, and why would both sides trust or self-host the same operator?
3. What failure, audit, approval, or privacy problem does the current stack leave unsolved?
4. Would exact replay and signed evidence change whether the agent may commit?
5. Which existing protocol or commerce API must the result interoperate with?
6. Is payment/checkout sufficient, or is multi-round negotiation genuinely required?
7. Who would run the service and who would pay for operational support?

Interview targets:

- AI API gateway and inference-routing teams;
- inference exchanges or GPU-capacity brokers;
- B2B agent and agent-wallet builders;
- A202/A2CN maintainers and implementers;
- procurement/CPQ platform engineering teams;
- enterprise AI governance teams.

### Kill or narrow the product if

- After 8–10 relevant interviews, fewer than three teams describe a current or planned need for durable negotiation state, independent evidence, human approval, or private live views.
- Interviewees consistently need only posted pricing, checkout, or payment authorization.
- Users want the A2CN reference library but not a separate durable operator.
- Both parties consistently reject any common operator even when it is self-hosted and produces signed portable records.
- No external developer can finish the no-Rust quickstart in fifteen minutes within six weeks of public release.
- Two independent agent processes cannot complete, disconnect, reconnect, and verify a session.
- Protocol-adapter work forces a generalized protocol abstraction before the first useful profile.
- Users do not value crash/retry, replay, approval, or scoped visibility—the capabilities that justify WorldStream.

### Keep it open-source-only if

- Developers use the verifier, conformance venue, or local runtime but do not ask for managed operation.
- There are no two or three design partners willing to pilot managed or bring-your-own-cloud deployment.
- Enterprise requirements would require building a procurement suite rather than operating sessions.

That is not failure. An adopted protocol operator and conformance reference is a strong open-source and portfolio outcome.

## Decisions to make next

1. **Approve or reject the product boundary:** operator, not protocol/procurement/payment.
2. **Approve the first wedge as a validation hypothesis:** AI/API capacity commitment.
3. **Run the A202 mapping spike:** prove one exact operated-scope profile for a two-party session can fit ActivityPackV1 without core changes.
4. **Audit exact-byte retention:** establish whether the current transition/event log can support signed evidence.
5. **Interview eight to ten builders:** do this before committing to hosted-product work.
6. **Build one vertical slice:** two deterministic agents, one human approval, restart/retry, and offline verification.
7. **Compare A202 and A2CN at the gate:** if A202 mapping is impractical, choose A2CN compatibility; do not publish a new protocol.

## Final recommendation

Proceed, but proceed as an evidence-driven open-source product experiment:

- keep the WorldStream kernel and its existing invariants;
- ship one built-in negotiation pack so users write configuration, not Rust;
- use A202 as the first operated compatibility target and treat A2CN as the direct benchmark;
- demonstrate an AI-capacity commitment rather than generic procurement or trivial price bargaining;
- separate party signatures, venue lineage, and external payment evidence;
- provide a purpose-built deal-room UI and offline verifier;
- keep Apache open source and delay monetization architecture;
- stop or narrow the idea if interviews and external quickstarts do not validate the operator layer.

The compelling version of this project is not “a room where agents talk.” It is:

> **The place where agents can make a structured commitment, humans can safely govern it, failures do not corrupt it, and everyone can verify what happened.**
