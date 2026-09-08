# Retire a completed House Runner unit

This is an offline Host Operator procedure for the hobby preview. It is not
an automatic timeout, an Activity Outcome, or a provider allowance reset.

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
Automatic, cross-store retirement is not implemented by this runbook.
