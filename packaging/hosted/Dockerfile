# syntax=docker/dockerfile:1.7
# Build from the repository root. Both base images are mandatory digest-pinned
# build arguments; the deployment command records them with the image digest.
ARG WORLDSTREAM_RUST_BUILDER_IMAGE
ARG WORLDSTREAM_NODE_RUNTIME_IMAGE

FROM ${WORLDSTREAM_RUST_BUILDER_IMAGE} AS builder
ARG SOURCE_REVISION
WORKDIR /src
COPY . .
RUN WORLDSTREAM_BUILD_REVISION="${SOURCE_REVISION}" cargo build --locked --release \
      -p worldstream-server \
      -p worldstream-studio-supervisor \
      -p worldstream-hosted-gateway \
      --bins \
    && mkdir -p /out \
    && cp target/release/worldstreamd /out/ \
    && cp target/release/worldstreamctl /out/ \
    && cp target/release/worldstream-studio-supervisor /out/ \
    && cp target/release/worldstream-assignment-mcp /out/ \
    && cp target/release/worldstream-managed-agent-host /out/ \
    && cp target/release/worldstream-hosted-gateway /out/ \
    && target/release/worldstream-hosted-artifact-digest \
      target/release/worldstream-managed-agent-host \
      > /out/managed-agent-host.blake3

FROM ${WORLDSTREAM_NODE_RUNTIME_IMAGE}

ARG SOURCE_REVISION=unresolved
ARG WORLDSTREAM_RUST_BUILDER_IMAGE
ARG WORLDSTREAM_NODE_RUNTIME_IMAGE

LABEL org.opencontainers.image.title="WorldStream Hosted Preview" \
      org.opencontainers.image.description="Single-authority WorldStream Hosted Gateway appliance" \
      org.opencontainers.image.revision="${SOURCE_REVISION}" \
      io.worldstream.topology="single-authority-fly-preview" \
      io.worldstream.public-listener="hosted-gateway:8080" \
      io.worldstream.rust-builder="${WORLDSTREAM_RUST_BUILDER_IMAGE}" \
      io.worldstream.node-runtime="${WORLDSTREAM_NODE_RUNTIME_IMAGE}"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 worldstream \
    && useradd --uid 65532 --gid 65532 --no-create-home --shell /usr/sbin/nologin worldstream \
    && mkdir -p /opt/worldstream/hosted /var/lib/worldstream /run/worldstream \
    && chown -R 65532:65532 /opt/worldstream /var/lib/worldstream /run/worldstream

COPY --from=builder /out/worldstreamd /usr/local/bin/worldstreamd
COPY --from=builder /out/worldstreamctl /usr/local/bin/worldstreamctl
COPY --from=builder /out/worldstream-studio-supervisor /usr/local/bin/worldstream-studio-supervisor
COPY --from=builder /out/worldstream-assignment-mcp /usr/local/bin/worldstream-assignment-mcp
COPY --from=builder /out/worldstream-managed-agent-host /usr/local/bin/worldstream-managed-agent-host
COPY --from=builder /out/worldstream-hosted-gateway /usr/local/bin/worldstream-hosted-gateway
COPY --from=builder /out/managed-agent-host.blake3 /opt/worldstream/hosted/managed-agent-host.blake3
COPY scripts/hosted-runtime.mjs /opt/worldstream/hosted/hosted-runtime.mjs
COPY scripts/hosted-volume-capture.mjs /opt/worldstream/hosted/hosted-volume-capture.mjs
COPY config/activity-clients/releases/agent-heist-web-v2.json /opt/worldstream/hosted/agent-heist-web.json
COPY config/activity-clients/releases/inspector-web.json /opt/worldstream/hosted/inspector-web.json
COPY config/activity-clients/hosted-local-bindings.json /opt/worldstream/hosted/activity-client-bindings.json
COPY packaging/hosted/entrypoint.sh /usr/local/bin/worldstream-hosted-entrypoint

RUN chmod 0755 \
      /usr/local/bin/worldstreamd \
      /usr/local/bin/worldstreamctl \
      /usr/local/bin/worldstream-studio-supervisor \
      /usr/local/bin/worldstream-assignment-mcp \
      /usr/local/bin/worldstream-managed-agent-host \
      /usr/local/bin/worldstream-hosted-gateway \
      /usr/local/bin/worldstream-hosted-entrypoint \
    && chmod 0644 /opt/worldstream/hosted/*.json /opt/worldstream/hosted/*.mjs \
    && chmod 0444 /opt/worldstream/hosted/managed-agent-host.blake3 \
    && chown -R 65532:65532 /opt/worldstream

ENV NODE_ENV=production \
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT=production \
    WORLDSTREAM_HOSTED_VOLUME_ROOT=/var/lib/worldstream \
    WORLDSTREAM_HOSTED_ASSET_ROOT=/opt/worldstream/hosted \
    WORLDSTREAM_HOSTED_BINARY_ROOT=/usr/local/bin \
    WORLDSTREAM_HOSTED_EPHEMERAL_ROOT=/run/worldstream

VOLUME ["/var/lib/worldstream"]
EXPOSE 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=15s --timeout=3s --start-period=30s --retries=3 \
  CMD ["node", "-e", "fetch('http://127.0.0.1:8080/healthz').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"]

ENTRYPOINT ["/usr/local/bin/worldstream-hosted-entrypoint"]
