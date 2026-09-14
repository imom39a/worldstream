# syntax=docker/dockerfile:1.7
ARG WORLDSTREAM_RUST_IMAGE="rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97"
ARG WORLDSTREAM_RUNTIME_IMAGE="debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171"

FROM ${WORLDSTREAM_RUST_IMAGE} AS builder
ARG SOURCE_REVISION=unresolved
WORKDIR /src
COPY . .
RUN test "$(rustc --version | awk '{print $2}')" = "1.97.1" \
    && cargo build --locked --release --jobs 1 \
      -p worldstream-sqlite --example history_qualification_fixture \
    && cargo build --locked --release --jobs 1 \
      -p worldstream-core --example operational_mmr_qualification \
    && cargo test --locked --release --jobs 1 \
      -p worldstream-server --lib \
      source_backed_warm_activation_claim_qualification --no-run \
    && mkdir -p /out \
    && cp target/release/examples/history_qualification_fixture /out/ \
    && cp target/release/examples/operational_mmr_qualification /out/ \
    && find target/release/deps -maxdepth 1 -type f -perm -111 \
      -name 'worldstream_server-*' -print -quit \
      | xargs -I '{}' cp '{}' /out/worldstream-server-tests \
    && test -x /out/worldstream-server-tests \
    && sha256sum /out/history_qualification_fixture /out/operational_mmr_qualification /out/worldstream-server-tests \
      > /out/artifact-sha256.txt \
    && printf '%s\n' "${SOURCE_REVISION}" > /out/source-revision.txt

FROM ${WORLDSTREAM_RUNTIME_IMAGE}
ARG SOURCE_REVISION=unresolved
ARG WORLDSTREAM_RUST_IMAGE
ARG WORLDSTREAM_RUNTIME_IMAGE
LABEL org.opencontainers.image.title="WorldStream IMO-232 one-million qualification" \
      org.opencontainers.image.revision="${SOURCE_REVISION}" \
      io.worldstream.rust-image="${WORLDSTREAM_RUST_IMAGE}" \
      io.worldstream.runtime-image="${WORLDSTREAM_RUNTIME_IMAGE}" \
      io.worldstream.release-evidence="false"

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates procps python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /out/history_qualification_fixture /usr/local/bin/
COPY --from=builder /out/operational_mmr_qualification /usr/local/bin/
COPY --from=builder /out/worldstream-server-tests /usr/local/bin/
COPY --from=builder /out/artifact-sha256.txt /opt/worldstream/artifact-sha256.txt
COPY --from=builder /out/source-revision.txt /opt/worldstream/source-revision.txt
COPY scripts/imo-220-warm-claim-qualification.sh /opt/worldstream/imo-220-warm-claim-qualification.sh
COPY packaging/qualification/imo-232/entrypoint.sh /usr/local/bin/worldstream-imo232-entrypoint
RUN chmod 0755 \
      /usr/local/bin/history_qualification_fixture \
      /usr/local/bin/operational_mmr_qualification \
      /usr/local/bin/worldstream-server-tests \
      /usr/local/bin/worldstream-imo232-entrypoint \
      /opt/worldstream/imo-220-warm-claim-qualification.sh

ENV WORLDSTREAM_HISTORY_FIXTURE_BIN=/usr/local/bin/history_qualification_fixture \
    WORLDSTREAM_WARM_TEST_BIN=/usr/local/bin/worldstream-server-tests \
    WORLDSTREAM_IMO220_PYTHON=/usr/bin/python3 \
    WORLDSTREAM_IMO220_TEMP_ROOT=/data/tmp
VOLUME ["/data"]
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/worldstream-imo232-entrypoint"]
