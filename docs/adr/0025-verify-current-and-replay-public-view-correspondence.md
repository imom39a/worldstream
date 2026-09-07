---
status: accepted
date: 2026-09-07
---

# Verify current and Replay public-view correspondence

The Runtime deliberately labels current public views `public` and reconstructed
Replay views `historical`. Their complete canonical hashes therefore differ
even when their authorized content agrees. Verify each hash independently and
require exact correspondence at the same Complete Head; do not change kernel
Replay semantics to force hash equality. The user approved this correction on
2026-09-07.

The Host must verify the exact Room, Pack, requested sequence, Complete Head,
healthy integrity status and generation, Activity content, and Action Offers.
Both core objects must contain exactly `access_mode`, `role`, `room_status`,
`standing`, and `viewer_class`. Every shared field must exist and compare equal;
only `viewer_class: public` becoming `viewer_class: historical` is permitted.
Missing, extra, or changed fields fail closed. Both full canonical hashes must
match their respective Runtime responses. No private view or Final Reveal is
substituted.

When Replay is available and this correspondence passes, the Host supplies the
verified historical public-view bytes and their hash to the result projector.
The authenticated hosted evidence's `projection_hash` identifies those supplied
bytes, not the different current-view wrapper. The projector-input hash and
Replay hash must match exactly. Without verified Replay, current public bytes
may establish terminal disposition, but cannot authorize result publication.

This decision corrects ADR 0023's requirement that the original current-view
wrapper and historical wrapper share a hash. It changes neither Room authority
nor Pack legality, visibility, Outcome, or Replay. Hash, Head, integrity, schema,
and content mismatches still block publication.
