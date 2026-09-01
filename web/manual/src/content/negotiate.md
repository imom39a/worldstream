# WorldStream Negotiate

WorldStream Negotiate is the first serious public Activity Pack. It uses one
Room to govern one pinned A202 bilateral formation session while commercial
agents, one Human approver, and one external venue signer remain independently
hosted.

## Roles and flow

Genesis has exactly `buyer_agent`, `seller_agent`, `buyer_approver`, and
`venue_signer`. The success path is:

`buyer Proposal → seller counter → exact approval request → Human signed
approval → restart/reconnect → buyer acceptance → selection → independent
Agreement signatures → commitment → identical Replay/evidence export`.

The Human approval binds the exact proposed Acceptance bytes, Proposal hash,
A202 session head, approver, decision, and expiry. Any relevant byte, logical
head, mandate/status evidence, or deadline change invalidates it. The Pack
never signs, calls a network, stores private keys, or decides commercial
strategy.

## Realtime value

The Room gives every member one current authorized Projection and exact Action
Offers while preserving a single order across counters, approval, deadlines,
signatures, reconnect, and venue resolution. Spectators see status without
commercial terms by default. Operator Membership receives bounded diagnostics,
not raw Activity State.

## Evidence

The final export links exact A202 objects, hashes, signatures, mandates,
approvals, logical streams, and deadlines to the Room's Genesis, Pack Revision,
bundle digest, Heads, Transitions, timers, receipts, Outcome, and Replay result.
An offline verifier consumes that export and the referenced bundle.

The public claim is only that WorldStream runs a pinned single-session A202
compatibility profile and publishes its supported matrix. It does not claim
A202 conformance.

## What the participant sees

The retained Negotiate renderer has six server-scoped views: buyer agent,
seller agent, Human buyer approver, venue signer, operator, and spectator. It
renders only the current authorized Projection and the Pack's exact Action
Offers. It does not infer permissions from the current phase, display raw
Activity State, or cache another party's private view.

The current Studio launcher sends Negotiate to the Client Host's
`/inspector/` fallback because there is no standalone registered Negotiate
Activity Client yet. The Inspector may select this retained compatibility
renderer only after it receives an authorized Negotiate delivery. Studio does
not render the view or inspect its participant-private data.

After attach or reconnect, the Console installs a Projection Reset or every
retained observation, acknowledges the sync barrier, and only then enables an
Action. It then acknowledges delivered Frames so reconnect starts from the
durable Membership Cursor. A stale-head rejection closes that gate and requires catch-up. If the
A202 logical predecessor changed, the outside signer must also rebuild and
re-sign the protocol object.

When a user chooses an offered action, the Console asks an independently
controlled application or Runner to prepare the payload. Signing keys,
strategy, prompts, model context, and private memory never move into
WorldStream. The returned payload still passes ordinary exact-head,
Membership, Pack, signature, approval, and deadline checks before it can
produce a Transition.

Preparation is bound to one fresh request ID, Action type, payload-schema
digest, and exact Room sequence. If the Room or A202 head changes while an
external signer is working, the response is discarded; it is never silently
rebased onto the new head.

For a Studio-opened human seat, the local Supervisor keeps Room, Membership,
and bearer authority in an HttpOnly session. The authorized observation carries
the pinned Pack identity needed to choose the Negotiate renderer, but no routing
identity or credential. External preparation uses a distinct retained-session
event bound to request, Action, schema, and Room sequence.

The Replay button uses the retained participant authority and accepts only a
verified response for the displayed exact Room Head. The evidence button emits
a metadata-only request to the application integrator, validates the returned
v1 dual-proof package binding, and downloads its exact JSON without rendering
commercial bytes. `worldstream-negotiate-verify` remains the offline
cryptographic authority.

Run the packaged production-seam lane after building `worldstreamctl`:

```sh
pnpm negotiate:acceptance
```

It executes the released `.wspack` twice through fresh production Component
Host processes and runs the independent restart oracle, Console privacy and
reconnect tests, and dual-evidence verifier. The resulting diagnostic is not a
substitute for the separate live persisted-Room restart release drill.

Operator and spectator pages show bounded status and references only. Tests
hold their DOM invariant while mutating private candidate bytes, agreement
bytes, and signatures, so a visually hidden field cannot accidentally become
an information channel.

See the [normative Negotiate specification](https://github.com/imom39a/worldstream/blob/main/docs/negotiate.md)
and [A202 decision](https://github.com/imom39a/worldstream/blob/main/docs/adr/0015-a202-operated-single-session-formation-profile.md).
