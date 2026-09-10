# Retained executor artifacts

These files are immutable source artifacts for built-in Pack revisions whose
revision locks predate later, backwards-compatible descriptor fields. They are
data consumed by `include_bytes!`; Rust does not compile them as current
modules.

The files preserve the exact bytes reviewed for these retained revisions:

- `agent_heist-v1.rs`: Git blob `9ec2f2d5d05d06d6d0aa7b094825fac1ae37b9f4`
- `counter-v1-v2.rs`: Git blob `2c3e52138494c529dc95bf31a5c742aedbad8c69`
- `counter-attention-v3.rs`: Git blob `81d5f33bd778e38ab13c61c57e1de7af50b49ddd`
- `counter-attention-v4.rs`: Git blob `07d012d121094cb8dba07e2bb3f6a3d13b052a4c`

Changing any artifact changes its derived `rule_source_digest` and must fail
the checked-in revision-lock and behavioral-corpus tests. New Pack revisions
should identify a separate, versioned executor artifact instead of hashing a
mutable implementation module.
