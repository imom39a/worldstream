# Deterministic Mira Runner

This external Python process answers one `companion_plan_requested` Activation
through the supported Python Runner and Membership SDK connections. It uses no
provider and holds no human or Host authority. Run it from the repository root:

```sh
uv run --project sdk/python --python 3.14.7 python -m examples.midnight_archive.run_mira \
  --membership-file /protected/mira-membership.json \
  --runner-file /protected/mira-runner.json \
  --pack-revision blake3:EXACT_APPROVED_MIRA_REVISION
```

Replace the revision placeholder with the exact approved IMO-202 Pack revision.
The executable accepts the two separately exported credential files; no bearer
belongs in an argument or environment variable. Existing strict credential
loaders require owner-only regular files. The example checks the exact Pack,
Mira Role, agent Principal, Runtime origin and single permitted Membership.

Each invocation keeps its Runner connection open only for its bounded wait
window. It claims only the first matching reason for that exact
Membership, compares the claimed Invocation Context with a fresh authorized
Projection at the same Head, and submits at most one `submit_companion_plan`
Action with an explicit `expected_room_seq`. It never submits human Actions,
commits a turn, rebases, or retries an Action. Its bounded wait loop only polls
for that matching opportunity. A lost reply or
stale Head ends the process without claiming successful work. Its JSON result
contains only a closed status and an action count, not credentials, Projections,
discoveries, plans, or raw protocol diagnostics. Runtime Canonical History
retains the accepted plan; the process keeps no private memory file.

## Integration recipe

Use the existing local `worldstreamctl` Room Setup and credential-export seam,
as used by `scripts/verify-midnight-archive-browser.mjs`. The assignment MCP
helper is an alternative consumer of the same separately sealed authorities;
this example connects through the SDK directly and does not launch that helper.

1. Install and approve the exact new Component Bundle. Create a
   `worldstream/room-setup/v1` specification using that Pack's exact digest and
   `{"scenario_id":"standard-v1"}`. Give `lead` a required human seat with
   `principal: {"reference":"lead","kind":"human"}`. Give `mira` a seat
   with `principal: {"reference":"mira","kind":"agent"}` and
   `assignment: {"mode":"external"}`. Mark Mira as required
   (`required: true`) because this bounded external Runner attaches before
   Activity Start and waits for the first planning opportunity. Mira is still a real starting Room
   Member. Set `operator_view: false`.
2. Run `room validate --file SETUP` and `room create --file SETUP`. Retain the
   returned operation and Room IDs; do not create a replacement Room on retry.
   Inspect the setup status and assert that lead and Mira have different
   `principal_id` and `member_id` values and kinds `human` and `agent`.
3. In an owner-only directory, export only Mira's capabilities for this process,
   start it before launching the Room, and wait for Mira's Runner readiness:

   ```sh
   worldstreamctl client export-credentials --operation OPERATION --seat mira --output /protected/mira-membership.json
   worldstreamctl runner export-credentials --operation OPERATION --seat mira --output /protected/mira-runner.json
   ```

   Start `run_mira` once with its default bounded wait before launching the
   Room, then connect and synchronize the human client separately.
4. Through the human client, assign Mira the structured investigation task and
   stage a personal Action, then submit `request_mira_plan` to open the Pack's
   planning opportunity. Record the human Head. The already-attached Runner
   claims it once. Assert one agent-authored plan Action
   was accepted, the Head advanced, and the Activity Turn did not advance.
5. Submit `prepare_mira_contribution`, then commit the human personal Action.
   Stage and prepare each later contribution before committing another turn;
   the recorded plan advances one eligible step per Activity Turn. Keep the
   Runner stopped. Assert recorded origin
   Membership, task revision, progress, Mira's Location, and authorized evidence
   disclosure. Then change or cancel the task and prove remaining steps cannot
   execute. A no-plan continuation uses the Pack's explicit human deferral.
6. Close the Runner and reconstruct the same final Head using the existing
   authorized `Client.replay(room_id, at_room_seq)` endpoint (or the current
   Component Host replay assertion). Assert `verification == "verified"` and
   exact current/replayed Projection equality under the same Membership. The
   Runner must not be invoked during Replay. Keep private payloads and exported
   credentials out of test receipts and committed artifacts.

Run the focused checks from the repository root:

```sh
uv run --project sdk/python --python 3.14.7 python -m pytest examples/midnight_archive/test_run_mira.py sdk/python/tests/test_runner_client.py
uv run --project sdk/python --python 3.14.7 ruff check examples/midnight_archive
```

These are integration instructions, not a claim that the live witness has run.
The existing Rust Component Host and browser acceptance harnesses remain the
appropriate places to retain the resulting live and Replay evidence.
