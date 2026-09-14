# Operational MMR qualification

These reports exercise the storage-neutral MMR primitive with the exact
length-prefixed leaf encoding used by the adapters. Each tier builds the full
immutable node inventory, samples 1,000 evenly distributed leaves, fetches only
the Core-produced sibling and peak coordinates, verifies against the final
root, and confirms that changing the leaf bytes fails verification.

| Leaves | Immutable nodes | Maximum proof nodes | Verification p50/p95/p99 | Build | SHA-256 |
| ---: | ---: | ---: | ---: | ---: | --- |
| 1,000 | 1,994 | 14 | 4/5/5 us | 2 ms | `e16ba1e0d6b7ea440a385b9f34413c2f78db5e8165f21b0d522c99f0fe2bda53` |
| 10,000 | 19,995 | 17 | 6/6/6 us | 22 ms | `a68f6f14a98f96cc759440071caf4a7583e3a07be77d668ac1fb3e3f4d559c30` |
| 100,000 | 199,994 | 21 | 7/7/7 us | 267 ms | `8703f2904e29f0b413ef2a57306f0d14334d37cbb9905f0399f786454624e076` |
| 1,000,000 | 1,999,993 | 25 | 8/8/8 us | 2,943 ms | `e9505e12f86a5366d600bd6ac1fede2349260dfaf5329ae051a530f476163749` |

All four local tiers verified exactly and rejected tampering. The million-leaf
proof is 25 nodes even though the retained inventory contains 1,999,993 nodes.
The local environment manifest records revision
`3a0b1d6549e81f4dcd7e4896155547b2d8f5e0fd`, Rust 1.97.1, Darwin/arm64,
1,000 samples per tier, and `release_evidence=false`.

The provider-neutral measurements establish logarithmic proof size and bounded
verification allocations. SQLite and PostgreSQL adapter tests separately prove
atomic row/leaf/node/receipt updates, current-root admission, authenticated
serving reads, retention behavior, and full verification. The 100k public
SQLite-to-PostgreSQL transfer report proves that three count-matched roots,
three receipts, and 199,994 nodes survive transfer exactly.

The local Linux/Docker qualification reproduces all four tiers alongside the
million-transition production SQLite and warm Gateway run. Its source-bound
reports are retained in the
[local Docker evidence directory](../imo-232-local-docker-2026-09-14/README.md):

| Leaves | Maximum proof nodes | Verification p50/p95/p99 | Build | SHA-256 |
| ---: | ---: | ---: | ---: | --- |
| 1,000 | 14 | 4/5/8 us | 2 ms | `1b5e47bfc0a80266fde14d2e4ced8bc228d54992ea91eadfd826a505b28774c7` |
| 10,000 | 17 | 6/6/8 us | 24 ms | `509f177c909fabfeb2bcc62d7c2245fcb8a874c8b773b8d6f7b0f9585cdeb037` |
| 100,000 | 21 | 7/7/7 us | 277 ms | `5030e91555afb6cfa1d2e38e53b09bc4582fce8487b02bcbbe79de205d3e2a8e` |
| 1,000,000 | 25 | 8/8/9 us | 3,667 ms | `2eb0cb55aed5a979d2048402fe7bb05ca600d20eb639dbfdfa114fe572119aa5` |
