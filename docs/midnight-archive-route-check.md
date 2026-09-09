# Midnight Archive route check

Date: 2026-09-09. This is a check of the proposed solo rules, not a test of
WorldStream, the Pack, the browser client or an LLM. The temporary model ran
in memory and did not change product code.

## Model and assumptions

Locations: Atrium (A), Records (R), Conservation (C), Plant (P), Vault (V).

Undirected edges: A–R, A–C, R–C, R–P, C–V, P–V. C–V requires the accepted
preservation agreement and completed collection work. P–V requires a powered
service hatch. Open gates stay open.

The human starts at A. Standard has three charges; Low Reserve has two.
The budget is 16 turns. Each adjacent move and each listed work step costs
one turn. Extraction is an explicit final turn at A.

Evidence authentication requires an inspection at R and one at C. The model
assumes the player correctly combines those sources and identifies the
corresponding candidate when at V. It does not model reading difficulty,
incorrect deductions or information leaks.

Powered verification at R costs one charge. Negotiated access requires an
agreement action and two collection-work steps at C; the second work step
costs one charge. Technical access at P costs two charges. Recovery at V
requires the selected authentication method to have supplied sufficient
evidence in this check. In the actual game, an unverified selection is also
legal and can produce a partial outcome.

Protecting the source record is an optional action at P after recovery,
costing one charge. Preserving the collection is the other optional objective
and can be completed even when the service route is used.

The model explored location, remaining power, discovered sources,
verification, agreement, collection progress, hatch status, ledger recovery
and record protection using breadth-first search through the turn budget.
Each comparison enables one authentication method and one access method.
Results are shortest witnesses under these simplified rules, not a claim
that the methods have equal value.

## Primary objective

| Authentication | Access | Standard: turns / power left | Low Reserve: turns / power left |
| --- | --- | --- | --- |
| Evidence | Agreement | 12 / 2 | 12 / 1 |
| Evidence | Service hatch | 12 / 1 | 12 / 0 |
| Powered verification | Agreement | 11 / 1 | 11 / 0 |
| Powered verification | Service hatch | 10 / 0 | Infeasible with two charges |

The fastest standard route spends the entire reserve and leaves the
collection unsecured. The negotiated routes take more time than that fastest
route and secure the collection. Reduced power removes the solo
powered-verification/service-hatch combination while preserving three routes.

Evidence plus the service hatch is not Pareto-best in the base primary-only
comparison. It remains a legal alternative, but should not be sold as an
equally attractive route without further circumstances or player goals.
Testing useful alternatives is different from claiming that every legal
combination is balanced.

## Both optional objectives

| Authentication | Access | Standard | Low Reserve |
| --- | --- | --- | --- |
| Evidence | Agreement | 16 turns, 1 charge left | 16 turns, 0 charges left |
| Powered verification | Agreement | 15 turns, 0 charges left | Infeasible |
| Evidence | Service hatch | Infeasible | Infeasible |
| Powered verification | Service hatch | Infeasible | Infeasible |

In this solo model, ordinary service access plus preservation plus record
protection exceeds the reserve. Jonah's cheaper method and Mira's
zero-charge assay are intended to change that possibility when companions
are present; no multi-actor solver or live companion test has established
their balance yet.

## Witnesses

Standard, powered verification and service hatch, primary objective:

1. A → R.
2. Use catalog verifier.
3. R → P.
4. Power service hatch.
5. P → V.
6. Recover the identified ledger.
7. V → P.
8. P → R.
9. R → A.
10. Extract.

Evidence, preservation agreement and both optional objectives, valid in
Standard and Low Reserve:

1. A → R.
2. Inspect intake evidence.
3. R → C.
4. Inspect restoration evidence.
5. Accept preservation agreement.
6. Prepare collection.
7. Energize preservation equipment.
8. C → V.
9. Recover the identified ledger.
10. V → C.
11. C → R.
12. R → P.
13. Protect the source record.
14. P → R.
15. R → A.
16. Extract.

The standard powered-verification/agreement witness with both optional goals
uses the same latter route, replaces the two evidence inspections with one
catalog-verification action, and finishes in 15 turns.

## What remains to prove during implementation

- Actual candidate generation and clue consistency, including what the human
  and each companion can discover at each point.
- All four party configurations, simultaneous task steps, resource
  reservations, follow/regroup behavior and extraction with missing crew.
- Replanning after new information, stale/cancelled plans and membership
  changes.
- Missteps, wrong candidates, late or unavailable model replies and explicit
  manual continuation.
- Whether players understand costs and consequences, enjoy making the
  decisions, and want to retry. Sixteen turns is an initial tuning value,
  not a measured 15–20-minute playtime guarantee.

Use these witnesses as inputs to new Pack fixtures, then validate them against
the actual reducer and production Component Host. This note cannot substitute
for that conformance evidence.

See the [game design](midnight-archive-design.md).
