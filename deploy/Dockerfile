ARG ALPINE_DIGEST=sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG RECEIVER_SOURCE_FINGERPRINT=unknown

# Both appliance binaries are C, built natively for the target platform on the
# same Alpine release the runtime image uses, so they link the musl, alsa-lib,
# and OpenSSL the image ships. Only the C tree enters this stage, so a deployer
# or script edit cannot force another emulated build.
# hadolint ignore=DL3029
FROM --platform=$TARGETPLATFORM alpine:3.23.5@${ALPINE_DIGEST} AS c-builder

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG RPI_OMT_CLIENT_VERSION=unknown

# Clang rather than GCC: the decoder's lane loops are written to be
# vectorized, which LLVM does and GCC does not, and on AArch64 that is the
# difference between matching the Rust decoder this replaced and trailing it
# by about 15%. llvm supplies the archiver its LTO objects need.
RUN apk add --no-cache alsa-lib-dev build-base clang llvm openssl-dev

WORKDIR /src
COPY mk/ mk/
COPY src/ src/
RUN make -f mk/c.mk CC=clang BUILD=release OMT_VERSION="${RPI_OMT_CLIENT_VERSION}" \
        -j"$(nproc)" receiver web \
    && out="$(make -s -f mk/c.mk CC=clang BUILD=release OMT_VERSION=x print-out)" \
    && install -Dm755 "${out}/bin/omt-receiver" /out/runtime/omt-receiver \
    && install -Dm755 "${out}/bin/omt-web" /out/runtime/omt-web

# Keep the testable/exportable ARM64 target limited to shipped artifacts. The
# compiler, sources, and object files never enter this image.
FROM scratch AS runtime-artifacts
ARG RECEIVER_SOURCE_FINGERPRINT
# Buildah/Podman can otherwise reuse this cross-stage COPY after the builder
# produced a different binary. Put the source fingerprint into the parent
# image configuration so a receiver change invalidates the COPY itself.
LABEL org.rpi-omt-client.receiver-source=${RECEIVER_SOURCE_FINGERPRINT}
COPY --from=c-builder /out/runtime/ /

FROM alpine:3.23.5@${ALPINE_DIGEST} AS runtime-base

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG WEB_PORT=5000
ARG RPI_OMT_CLIENT_VERSION=unknown

RUN apk add --no-cache \
        alsa-lib \
        bash \
        coreutils \
        libcrypto3 \
        libssl3 \
        openssl \
        util-linux \
    && addgroup -S omt \
    && adduser -S -D -H -G omt -h /etc/omt -s /sbin/nologin omt \
    && mkdir -p /app/legal /etc/omt /run/omt \
    && chown omt:omt /etc/omt /run/omt \
    && chmod 700 /run/omt

ARG RECEIVER_SOURCE_FINGERPRINT
# Repeat the fingerprint at the final cross-stage boundary. Buildah may cache
# this COPY independently of the changed intermediate image unless the final
# stage's own parent configuration changes too.
LABEL org.rpi-omt-client.receiver-source=${RECEIVER_SOURCE_FINGERPRINT}
COPY --from=runtime-artifacts /omt-receiver /usr/local/bin/omt-receiver
COPY --from=runtime-artifacts /omt-web /usr/local/bin/omt-web
COPY deploy/container/ /usr/local/bin/
COPY LICENSE THIRD_PARTY_NOTICES.txt /app/legal/

FROM runtime-base AS sbom-builder
ARG RPI_OMT_CLIENT_VERSION=unknown
RUN cp /lib/apk/db/installed /tmp/runtime-installed \
    && apk add --no-cache python3
COPY scripts/generate-runtime-sbom.py /tmp/
RUN python3 /tmp/generate-runtime-sbom.py \
        --output /runtime-sbom.cdx.json \
        --apk-installed /tmp/runtime-installed \
        --version "${RPI_OMT_CLIENT_VERSION}"

FROM runtime-base AS runtime
SHELL ["/bin/ash", "-eo", "pipefail", "-c"]
ARG WEB_PORT=5000
ARG RPI_OMT_CLIENT_VERSION=unknown
COPY --from=sbom-builder /runtime-sbom.cdx.json /app/legal/runtime-sbom.cdx.json

RUN chmod 0755 \
        /usr/local/bin/omt-receiver \
        /usr/local/bin/omt-web \
        /usr/local/bin/control-omt.sh \
        /usr/local/bin/start-omt.sh \
        /usr/local/bin/entrypoint.sh \
    && printf '%s\n' "${RPI_OMT_CLIENT_VERSION}" > /app/RPI_OMT_CLIENT_VERSION \
    && { \
        find /app -type f ! -name runtime-sha256.manifest -print0; \
        find /usr/local/bin -maxdepth 1 -type f -print0; \
    } | sort -z | xargs -0 sha256sum > /app/runtime-sha256.manifest

ENV HOME=/etc/omt
ENV OMT_CONFIG_DIR=/etc/omt
ENV OMT_STORAGE_PATH=/etc/omt/omt
# Per-boot state, deliberately off the SD-card-backed config volume. compose.yml
# mounts a tmpfs at /run/omt; the entrypoint owns this 0700 directory inside it.
# /run/omt is pre-created and owned above, because /run itself is root-owned and
# this image runs as omt: without it a run that does not mount the tmpfs could
# not create the directory at all.
ENV OMT_RUNTIME_DIR=/run/omt/state
ENV WEB_PORT=${WEB_PORT}

LABEL org.opencontainers.image.title="Raspberry Pi OMT Client"
LABEL org.opencontainers.image.version="${RPI_OMT_CLIENT_VERSION}"
LABEL org.opencontainers.image.licenses="MIT"

USER omt
EXPOSE ${WEB_PORT}
VOLUME ["/etc/omt"]

# Surface a wedged Web service in `docker ps` and host diagnostics. The listener is
# HTTPS with a self-signed certificate generated on first boot, so the probe
# does not verify it; GET /login is unauthenticated and unthrottled, so it
# cannot consume an operator's rate-limit budget. The start period must outlast
# first-boot key and certificate generation in entrypoint.sh.
HEALTHCHECK --interval=30s --timeout=5s --start-period=45s --retries=3 \
    CMD wget -q --spider --no-check-certificate "https://127.0.0.1:${WEB_PORT}/login"
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
