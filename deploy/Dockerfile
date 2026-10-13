ARG ALPINE_DIGEST=sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG RECEIVER_SOURCE_FINGERPRINT=unknown

# The appliance binaries are cross-compiled on the build machine for the
# target platform against a sysroot of the same Alpine release the runtime
# image uses, so they link the musl, alsa-lib, and OpenSSL the image ships.
# omt-receiver and omt-web are Rust, built by the pinned stable toolchain (only
# that and the one target, never the gate tooling) and linked by clang and lld
# through scripts/cargo-linker.sh. Compiling them under QEMU instead took tens
# of minutes per ARM64 build; only the runtime stages below still run target
# binaries, and those only install packages. Only the Rust workspace enters
# this stage, so a script or documentation edit cannot force another build.
#
# lld links: Alpine's cross binutils is built without the LTO plugin. Rust's
# standard library asks for libgcc_s for unwinding, which the runtime image
# does not ship; the linker wrapper substitutes the static libgcc_eh, and the
# check below keeps it that way.
# hadolint ignore=DL3029
FROM --platform=$BUILDPLATFORM alpine:3.23.5@${ALPINE_DIGEST} AS builder

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG RPI_OMT_CLIENT_VERSION=unknown
ARG TARGETARCH

RUN apk add --no-cache bash clang curl lld llvm

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH="/opt/cargo/bin:${PATH}"
COPY scripts/install-rust.sh /usr/local/bin/install-rust.sh
RUN case "${TARGETARCH}" in \
        arm64) triple=aarch64-unknown-linux-musl ;; \
        amd64) triple=x86_64-unknown-linux-musl ;; \
        *) echo "unsupported TARGETARCH ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && install-rust.sh --build-only "${triple}"

COPY scripts/make-sysroot.sh /usr/local/bin/make-sysroot.sh
RUN case "${TARGETARCH}" in \
        arm64) arch=aarch64 ;; \
        amd64) arch=x86_64 ;; \
        *) echo "unsupported TARGETARCH ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && make-sysroot.sh "${arch}" /sysroot \
    && printf '%s\n' "${arch}" > /sysroot/.omt-arch

WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo/ .cargo/
COPY crates/ crates/
COPY scripts/cargo-linker.sh scripts/cargo-linker-aarch64.sh scripts/cargo-linker-x86_64.sh scripts/
# RUSTUP_TOOLCHAIN overrides rust-toolchain.toml, whose lint components this
# stage does not install.
RUN arch="$(cat /sysroot/.omt-arch)" \
    && triple="${arch}-unknown-linux-musl" \
    && env_triple="$(printf '%s' "${triple}" | tr 'a-z-' 'A-Z_')" \
    && toolchain="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)" \
    && export "CARGO_TARGET_${env_triple}_LINKER=/src/scripts/cargo-linker-${arch}.sh" \
        RUSTUP_TOOLCHAIN="${toolchain}" \
        OMT_SYSROOT=/sysroot OMT_VERSION="${RPI_OMT_CLIENT_VERSION}" \
        RUSTFLAGS="-C target-feature=-crt-static -C link-self-contained=no" \
    && cargo build --release --locked --offline --target "${triple}" -p omt-receiver -p omt-web \
    && for bin in omt-receiver omt-web; do \
        if llvm-readelf -d "target/${triple}/release/${bin}" | grep -q libgcc_s; then \
            echo "${bin} depends on libgcc_s, which the runtime image lacks" >&2; exit 1; \
        fi; \
        install -Dm755 "target/${triple}/release/${bin}" "/out/runtime/${bin}"; \
    done

# Keep the testable/exportable ARM64 target limited to shipped artifacts. The
# compiler, sources, and object files never enter this image.
FROM scratch AS runtime-artifacts
ARG RECEIVER_SOURCE_FINGERPRINT
# Buildah/Podman can otherwise reuse this cross-stage COPY after the builder
# produced a different binary. Put the source fingerprint into the parent
# image configuration so a receiver change invalidates the COPY itself.
LABEL org.rpi-omt-client.receiver-source=${RECEIVER_SOURCE_FINGERPRINT}
COPY --from=builder /out/runtime/ /

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
COPY scripts/generate-runtime-sbom.py rust-toolchain.toml /tmp/
RUN python3 /tmp/generate-runtime-sbom.py \
        --output /runtime-sbom.cdx.json \
        --apk-installed /tmp/runtime-installed \
        --rust-toolchain /tmp/rust-toolchain.toml \
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
