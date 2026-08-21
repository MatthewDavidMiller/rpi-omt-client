ARG ALPINE_DIGEST=sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG RUST_DIGEST=sha256:c4a364ddbf684fe038e6fa6a4f25b30c8dc85247423e0e660676ece0d17be4a2
ARG RECEIVER_SOURCE_FINGERPRINT=unknown

# Build with the exact stable compiler required by rust-toolchain.toml. The
# official image digest is a multi-architecture manifest, so each output is
# compiled against its native musl target without a cross-linker ambiguity.
# hadolint ignore=DL3029,DL3006
FROM --platform=$TARGETPLATFORM rust:1.97.1-alpine3.23@${RUST_DIGEST} AS receiver-builder

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG RPI_OMT_CLIENT_VERSION=unknown

RUN apk add --no-cache alsa-lib-dev build-base musl-dev perl pkgconf

WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo/ .cargo/
# Cargo parses every declared workspace member before selecting `-p`, and
# `--locked` requires that workspace shape to remain intact. Copy the deployer
# and test-sender manifests with inert targets, while keeping their real source
# out of this build context; a GUI/SSH-only or test-tooling edit then cannot
# force another slow emulated ARM64 receiver build, and neither the deployer
# nor the test sender can reach the appliance image.
COPY crates/omt-deployer-core/Cargo.toml crates/omt-deployer-core/Cargo.toml
COPY crates/rpi-omt-deploy/Cargo.toml crates/rpi-omt-deploy/Cargo.toml
COPY crates/rpi-omt-deployer/Cargo.toml crates/rpi-omt-deployer/Cargo.toml
COPY crates/rpi-omt-deploy-tui/Cargo.toml crates/rpi-omt-deploy-tui/Cargo.toml
COPY crates/omt-test-sender/Cargo.toml crates/omt-test-sender/Cargo.toml
RUN mkdir -p \
        crates/omt-deployer-core/src \
        crates/rpi-omt-deploy/src \
        crates/rpi-omt-deployer/src \
        crates/rpi-omt-deploy-tui/src \
        crates/omt-test-sender/src \
    && : > crates/omt-deployer-core/src/lib.rs \
    && printf 'fn main() {}\n' > crates/rpi-omt-deploy/src/main.rs \
    && printf 'fn main() {}\n' > crates/rpi-omt-deployer/src/main.rs \
    && printf 'fn main() {}\n' > crates/rpi-omt-deploy-tui/src/main.rs \
    && printf 'fn main() {}\n' > crates/omt-test-sender/src/main.rs
COPY crates/omt-protocol/ crates/omt-protocol/
COPY crates/omt-receiver-core/ crates/omt-receiver-core/
COPY crates/vmx-decoder/ crates/vmx-decoder/
COPY crates/omt-receiver/ crates/omt-receiver/
COPY crates/omt-web/ crates/omt-web/
ENV RPI_OMT_CLIENT_VERSION=${RPI_OMT_CLIENT_VERSION}
RUN cargo build --locked --release -p omt-receiver -p omt-web \
    && install -Dm755 target/release/omt-receiver /out/runtime/omt-receiver \
    && install -Dm755 target/release/omt-web /out/runtime/omt-web

# Keep the testable/exportable ARM64 target limited to shipped artifacts. The
# Compiler, source, and Cargo intermediates never enter this image.
FROM scratch AS runtime-artifacts
ARG RECEIVER_SOURCE_FINGERPRINT
# Buildah/Podman can otherwise reuse this cross-stage COPY after the builder
# produced a different binary. Put the source fingerprint into the parent
# image configuration so a receiver change invalidates the COPY itself.
LABEL org.rpi-omt-client.receiver-source=${RECEIVER_SOURCE_FINGERPRINT}
COPY --from=receiver-builder /out/runtime/ /

FROM alpine:3.23.5@${ALPINE_DIGEST} AS runtime-base

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG WEB_PORT=5000
ARG RPI_OMT_CLIENT_VERSION=unknown

RUN apk add --no-cache \
        alsa-lib \
        bash \
        coreutils \
        libgcc \
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
COPY Cargo.lock /tmp/Cargo.lock
COPY scripts/generate-runtime-sbom.py scripts/cargo_lock.py /tmp/
RUN python3 /tmp/generate-runtime-sbom.py \
        --output /runtime-sbom.cdx.json \
        --cargo-lock /tmp/Cargo.lock \
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

# Surface a wedged Rust Web service in `docker ps` and host diagnostics. The listener is
# HTTPS with a self-signed certificate generated on first boot, so the probe
# does not verify it; GET /login is unauthenticated and unthrottled, so it
# cannot consume an operator's rate-limit budget. The start period must outlast
# first-boot key and certificate generation in entrypoint.sh.
HEALTHCHECK --interval=30s --timeout=5s --start-period=45s --retries=3 \
    CMD wget -q --spider --no-check-certificate "https://127.0.0.1:${WEB_PORT}/login"
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
