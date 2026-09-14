# IMO-232 one-million Linux qualification image

This image runs the production SQLite fixture and warm server path at exactly
1,000,000 accepted Transitions with 1,000 warm read and claim samples. Build it
from a clean, committed checkout for the local machine's native Linux target,
then run it with 4 CPUs, an 8-GiB memory limit, and a disposable Docker-managed
local volume mounted at `/data`.

The container writes `runtime-manifest.json`, `imo-232-linux-1m.json`, four
`operational-mmr-*.json` reports for 1k through 1m leaves, and `status` to the
mounted data directory. A passing process sleeps after writing the artifacts
so the operator can copy them before deleting the disposable container,
volume, image, and large fixture database. The JSON remains local engineering evidence with
`release_evidence=false`; it does not substitute for IMO-235's 72-hour soak.

Both base-image indexes, Rust 1.97.1, the source revision, and the three executed
binary SHA-256 digests are bound into the image or output artifacts. The build
must pass `SOURCE_REVISION=$(git rev-parse HEAD)` and must use a clean checkout.

Run the large qualification locally:

```sh
revision="$(git rev-parse HEAD)"
short_revision="$(printf '%.8s' "$revision")"
docker build \
  --build-arg "SOURCE_REVISION=$revision" \
  --file packaging/qualification/imo-232/Dockerfile \
  --tag "worldstream-imo232-local:$short_revision" .
docker volume create worldstream-imo232-local-data
docker run --detach \
  --name worldstream-imo232-local \
  --cpus 4 \
  --memory 8g \
  --memory-swap 8g \
  --mount type=volume,src=worldstream-imo232-local-data,dst=/data \
  "worldstream-imo232-local:$short_revision"
```

Wait until `/data/status` contains `pass`, copy the small JSON reports, then
stop and remove the container, volume, and image. The 1m source database exists
only inside the disposable local volume and the entrypoint deletes it before
publishing `pass`.

An interrupted operator rerun may retain a verified disposable 1m source as
`/data/fixture-1000000.sqlite.gz` with its original
`/data/fixture-1000000.json` report. When both are present, the entrypoint
decompresses a mutable working copy and repeats only the warm phase. The report
still validates the exact requested count and original fixture pass before it
can publish success, and identifies the retained fixture report separately
from the warm test binary.
