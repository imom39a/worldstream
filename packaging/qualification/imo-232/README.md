# IMO-232 one-million Linux qualification image

This image runs the production SQLite fixture and warm server path at exactly
1,000,000 accepted Transitions with 1,000 warm read and claim samples. Build it
from a clean, committed checkout for `linux/amd64`, then run it on the ticket's
4 performance-vCPU, 8-GiB, local-SSD reference profile with a volume mounted at
`/data`.

The container writes `runtime-manifest.json`, `imo-232-linux-1m.json`, four
`operational-mmr-*.json` reports for 1k through 1m leaves, and `status` to the
volume. A passing process sleeps after writing the artifacts so the operator
can copy them before deleting the disposable Machine, app, and volume. The JSON remains local engineering evidence with
`release_evidence=false`; it does not substitute for IMO-235's 72-hour soak.

Both base-image indexes, Rust 1.97.1, the source revision, and the three executed
binary SHA-256 digests are bound into the image or output artifacts. The build
must pass `SOURCE_REVISION=$(git rev-parse HEAD)` and must use a clean checkout.

An interrupted operator rerun may retain a verified disposable 1m source as
`/data/fixture-1000000.sqlite.gz` with its original
`/data/fixture-1000000.json` report. When both are present, the entrypoint
decompresses a mutable working copy and repeats only the warm phase. The report
still validates the exact requested count and original fixture pass before it
can publish success.
