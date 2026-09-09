# Retire a completed House Runner unit

The hosted preview automatically retires a House Runner after the Platform has
stored verified Run Terminal Evidence. This is operational cleanup, not an
Activity Outcome, timeout, provider allowance reset, or a way to rewrite an
Assignment or reservation receipt.

The automatic S6 path is deliberately narrow:

1. The independent Result Projector records immutable terminal evidence.
2. The Platform database returns retirement material only for that terminal
   Run, its exact successful reservation, and its exact House Assignment.
3. The Host writes a durable `retiring.json` intent fence before it drains the
   exact managed helper/model pair. Spawn paths reject the intent, final
   `retired.json`, and legacy operator fence.
4. Only after the pair is stopped and reaped does the Host write its signed
   retirement receipt. The Platform retains that receipt and then changes the
   matching `succeeded` reservation to `released` in the same transaction.

A crash after intent installation keeps capacity occupied and prevents a late
spawn; the bounded reconciler retries the same evidence-bound request. Result
publication is not held behind this cleanup: terminal/result persistence is
durable first, and a separate terminal-retirement candidate lane continues
automatic retry until the immutable receipt exists. The Host never accepts a
browser request or a generic process-control command.

For this MVP, the service-authenticated Gateway is the trust boundary for the
Host receipt. The database validates its exact Run, Assignment, reservation,
unit, disposition, digest, and signed shape; it does not receive a second Host
signing secret. A mismatched or conflicting receipt cannot release capacity.

Pre-Genesis failure and verified pre-start abandonment use the same Host
operation but are not automatic S6 triggers. They remain reserved for the
separate S7 recovery/expiry policy.

## Offline repair procedure

1. Close platform launches and House fill. Verify retained Run Terminal Evidence
   for each exact launch and reservation. Do not release pending or ambiguous
   reservations with this procedure.
2. Enter Fly maintenance. Stop the Controller, Runtime, assignment helpers and
   model hosts. Verify the managed listeners are closed and no helper or model
   host process remains. Capture the populated volume before changing it.
3. For each verified completed unit, retain a private operator receipt at
   `studio/hosted-house-runners/units/<runner_unit_id>/operator-retired.json`.
   Record the installation, source revision, reservation operation, launch, Run,
   unit, terminal-evidence digest, checkpoint and stop-verification time. Use
   exclusive creation, mode `0600`, and sync the file and parent directory.
   Verify the unit directory is the real protected directory, not a link.
4. Deploy a Host with retirement-fence support before reopening admission.
   The production process launcher must reject the retained marker before
   spawning either child. A link or inspection error must also fail closed.
   Its reservation coordinator must exclude only exact, private retirement
   records from global concurrent capacity. Retained per-launch limits and
   original reservation receipts remain unchanged.
   Never remove the marker or run a pre-fence binary after releasing capacity.
5. In one reviewed platform transaction, lock the installation capacity gate
   and exact reservation rows. Recheck maintenance and exact terminal evidence.
   Change only matching `succeeded` rows to `released`, retaining original
   reservation receipts and recording the actual operator-receipt SHA-256 and
   release timestamp. A retry must match the same receipt; conflicts fail closed.
6. Verify the released rows, retained markers and unchanged allowance ledger
   before reopening launches. Keep the private receipts off the public site.

The receipt states operator observation, not a Runtime attestation. No Room,
Membership, Assignment, Action, result or allowance is deleted or rewritten.
If a step cannot be verified, keep the reservation held. Do not increase the
four-unit capacity limit to work around a failed release.

The marker does not stop an already running child. This procedure therefore
requires a closed maintenance interval; it is not safe as an online cleanup.
Use this repair procedure only when the automatic path cannot establish the
required evidence or durable fence. It must not bypass the automatic receipt
and reservation rules above.
