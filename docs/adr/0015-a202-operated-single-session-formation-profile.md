---
status: accepted
date: 2026-08-30
---

# Pin an A202 single-session formation compatibility profile

WorldStream Negotiate will target the **WorldStream A202 v0.1 Operated
Single-Session Formation Compatibility Profile**. This WorldStream label pins
A202 repository revision `fa85aa8b49bfe7b3f7ded487c98500a600e92e41`, shared
objects `a202-commercial/0.1`, state-machine rules `1.3`, operated scope
`a202-scope/operated/0.1`, required bilateral behavior from
`a202-scope/bilateral/0.1`, and demonstration profile
`a202-profile/calibration-service/0.1`. It covers exactly one A202 transaction,
one bilateral session, two commercial organizations, and one WorldStream Room.

The supported formation path imports an already-qualified signed opening
history, then supports Proposal Revisions and counters, author-only withdrawal,
validity and formation expiry, byte-exact Human approval of the buyer acceptance
act, acceptance and operated selection of the exact current seller Offer,
independent commercial-party Agreement signatures, and
`agreement.committed`. The successful WorldStream Outcome is established only
at aggregate `committed`; the pinned A202 session remains `accepted` because
the pinned schema has no truthful successful close reason. Exact A202 bytes,
logical A202 heads, signatures, mandates, approvals, resolver evidence, and
deadlines are Pack-owned facts under the Room's separate total order. Private
keys, network calls, strategy, prompts, and model execution remain in external
clients and Runners.

WorldStream will claim only that it runs this pinned compatibility profile and
publishes the supported object, transition, and fixture matrix. It will not
claim A202 conformance. Publication/discovery, invitations, qualification,
auctions, competitive or multi-supplier award, cross-Room aggregation,
post-commit obligations/settlement/disputes/amendments/termination, operator
key custody, and a WorldStream-specific negotiation protocol are outside this
profile. A2CN remains a later adapter/UX benchmark because its two-party local
authority and reconciliation model differs from WorldStream's single
authoritative Room.
