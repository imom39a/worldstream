# ADR 0009: Activation Intents, Context, and Lease Fencing

Status: Accepted, 2026-08-15

WorldStream records canonical Attention Signals but manages policy decisions, five-state Activation Intents, claims, leases, and Invocation Context as durable operational state. A claim grants at most one live lease per Membership, commits an exact authorized context under complete Head/authority/integrity/policy/delivery witnesses, and uses idempotent operation receipts plus generations to fence lost replies and stale claimants; Runner control remains separate from participant Action and Session authority. This supports recoverable at-least-once external execution without pretending to provide exactly-once models or allowing Replay to cause effects, at the cost of explicit fencing and receipt retention.
