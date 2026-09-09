---
status: accepted
date: 2026-09-08
---

# Defer populated disaster recovery for the hobby preview

The user accepted Q10 of the platform-stability interview: qualify ordinary
same-volume restart and Run Re-entry now, but defer full populated
Runtime–Controller–platform disaster-recovery qualification. This lets the
hobby preview establish a usable platform while explicitly accepting possible
history loss after catastrophic storage failure. It does not establish a
commercial durability guarantee or waive the generic WorldStream release
contract.

This narrowly amends ADR 0024's deployment and public-launch prerequisites.
A successful populated isolated restore drill is not a prerequisite for this
stabilization preview. The original populated-recovery goal remains separate,
unfinished work; do not mark it passed or use an older zero-history drill as
proof. The ordinary restart, authority, privacy, result-integrity, compatible
artifact/schema, and provider-accounting gates remain mandatory.

The protected offline paired capture before deployment remains required.
Preserve source identities, archive/dump checksums, allowance evidence, and
the closed capture interval. Retain the existing backup cadence and private
off-Machine storage. A successful capture or checksum check is not a semantic
restore proof: label an unqualified populated set as an unverified capture,
not a verified Hosted Recovery Checkpoint. Never manufacture or upgrade a
verification receipt to get through a release command.

If catastrophic storage loss prevents verified continuity, an explicitly
approved fresh preview may be created with a clear history-loss disclosure.
No reset is automatic or authorized for the current installation by this ADR.
Never serve a partial or independently timed restore as coherent history, and
never use recovery or a reset to repeat an ambiguous provider call or refill
an externally consumed spending allowance. A populated restore still requires
the deferred correspondence and restored-assignment fencing verification
before any continuity claim or cutover.

The platform stabilization release must pass the local and deployed journey
checks in the [support contract](../hosted-preview-support.md), including
ordinary restart/re-entry and repeat play. The existing IMO-184 acceptance
issue remains incomplete until its scope and evidence are reconciled with
this decision. Removing the populated-restore release prerequisite is a scope
decision, not a successful test or deployment.
