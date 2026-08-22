# syntax=docker/dockerfile:1.7@sha256:a57df69d0ea827fb7266491f2813635de6f17269be881f696fbfdf2d83dda33e
# Build context: an output directory created by `package.py oci-context`.
# The base image must be pinned by digest by the release evidence job.
ARG WORLDSTREAM_BASE_IMAGE
FROM ${WORLDSTREAM_BASE_IMAGE}

ARG VERSION=unresolved
ARG MANIFEST_SHA256=unresolved
ARG SOURCE_REVISION=unresolved
ARG BUILD_IDENTITY_SHA256=unresolved
ARG BUILD_ENVIRONMENT_BASE64=unresolved
ARG WORLDSTREAM_BASE_IMAGE

LABEL org.opencontainers.image.title="WorldStream" \
      org.opencontainers.image.description="WorldStream realtime room runtime" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${SOURCE_REVISION}" \
      org.opencontainers.image.ref.name="worldstream/storage-compatibility-manifest/v1" \
      io.worldstream.target="linux/amd64" \
      io.worldstream.manifest-sha256="${MANIFEST_SHA256}" \
      io.worldstream.build-identity="${BUILD_IDENTITY_SHA256}" \
      io.worldstream.build-environment="${BUILD_ENVIRONMENT_BASE64}" \
      io.worldstream.base-image="${WORLDSTREAM_BASE_IMAGE}" \
      io.worldstream.read-only-root="true" \
      io.worldstream.sqlite-filesystems="ext4,xfs" \
      io.worldstream.probes="/healthz,/readyz,/version"

RUN addgroup -S -g 65532 worldstream \
    && adduser -S -D -H -u 65532 -G worldstream worldstream \
    && mkdir -p /var/lib/worldstream /opt/worldstream \
    && chown -R 65532:65532 /var/lib/worldstream /opt/worldstream

COPY bin/worldstreamd /usr/local/bin/worldstreamd
COPY bin/worldstreamctl /usr/local/bin/worldstreamctl
COPY ui /opt/worldstream/ui
COPY sdk /opt/worldstream/sdk
COPY manifest /opt/worldstream/manifest
COPY metadata /opt/worldstream/metadata
COPY oci-metadata.json /opt/worldstream/metadata/oci.json
COPY examples /opt/worldstream/examples
COPY licenses /opt/worldstream/licenses
COPY entrypoint.sh /usr/local/bin/worldstream-oci-entrypoint

RUN chmod 0755 /usr/local/bin/worldstreamd /usr/local/bin/worldstreamctl /usr/local/bin/worldstream-oci-entrypoint \
    && chown -R 65532:65532 /usr/local/bin /opt/worldstream

USER 65532:65532
ENV WORLDSTREAM__STORAGE__PROFILE=sqlite-bundled \
    WORLDSTREAM__STORAGE__DATA_DIR=/var/lib/worldstream
VOLUME ["/var/lib/worldstream"]
WORKDIR /opt/worldstream
STOPSIGNAL SIGTERM

# This is deliberately a live-daemon binary check. Readiness remains the
# separate durable-storage and writer/scheduler contract at /readyz.
HEALTHCHECK --interval=5s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/worldstreamctl", "--data-dir", "/var/lib/worldstream", "health"]

ENTRYPOINT ["/usr/local/bin/worldstream-oci-entrypoint"]
