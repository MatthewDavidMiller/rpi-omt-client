ARG ALPINE_DIGEST=sha256:5c216c9767869f6ec7c4718977601d52f087c5c7bf2fdfb198e403f6b48ec40d
ARG RECEIVER_SOURCE_FINGERPRINT=unknown
# The linux/arm64 and linux/amd64 manifests inside ALPINE_DIGEST's index. The
# runtime stages copy the target's Alpine userland from the one that matches
# TARGETARCH. They are pinned individually because container storage resolves a
# digest reference to a single image: with the index digest named for two
# platforms at once, Podman finds one and rejects the other ("image not known").
# Bump all three digests together.
ARG ALPINE_DIGEST_ARM64=sha256:5dda816445facf5f7282dc7689a333278834c74265ba25db2e2eb90509e3a9dd
ARG ALPINE_DIGEST_AMD64=sha256:644c1ad8fc45cc581ab1b5ea7ec1bebc9b037fd4801ad697ca124deb3f9c9332
ARG TARGETARCH

# The appliance binaries are cross-compiled on the build machine for the
# target platform against a sysroot of the same Alpine release the runtime
# image uses, so they link the musl, alsa-lib, and OpenSSL the image ships.
# omt-receiver and omt-web are Rust, built by the pinned stable toolchain (only
# that and the one target, never the gate tooling) and linked by clang and lld
# through scripts/cargo-linker.sh. Compiling them under QEMU instead took tens
# of minutes per ARM64 build; no stage of this file runs a target binary, so the
# whole image builds under rootless Podman with no emulation. Only the Rust
# workspace enters this stage, so a script or documentation edit cannot force another build.
#
# lld links: Alpine's cross binutils is built without the LTO plugin. Rust's
# standard library asks for libgcc_s for unwinding, which the runtime image
# does not ship; the linker wrapper substitutes the static libgcc_eh, and the
# check below keeps it that way.
# hadolint ignore=DL3029
FROM --platform=$BUILDPLATFORM alpine:3.23.8@${ALPINE_DIGEST} AS builder

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

# The runtime image is assembled without executing a single target binary, so
# building the ARM64 appliance needs no QEMU and no binfmt_misc handler, and
# therefore no root: a rootless Podman on a native Linux host, under WSL, or on
# a workstation without systemd builds it the same way. The target
# architecture's pristine Alpine userland is only copied from, never run.
# hadolint ignore=DL3029
FROM --platform=linux/arm64 docker.io/library/alpine@${ALPINE_DIGEST_ARM64} AS target-base-arm64
# hadolint ignore=DL3029
FROM --platform=linux/amd64 docker.io/library/alpine@${ALPINE_DIGEST_AMD64} AS target-base-amd64
# The stage name is chosen by TARGETARCH, which hadolint cannot resolve.
# hadolint ignore=DL3006
FROM target-base-${TARGETARCH} AS target-base

# Everything that would have run inside the target image runs here instead, on
# the build machine's own architecture, against a root directory: apk installs
# the target's packages into it with --arch and --no-scripts (the same way
# scripts/make-sysroot.sh builds the compile sysroot), and the account, the
# directories, the manifest and the SBOM are written as plain file operations.
# Package scripts are skipped because they would execute target binaries; the
# only one these packages carry that matters is bash registering itself in
# /etc/shells, done below. The `omt` ids are the ones busybox adduser assigned
# when this ran inside the image (uid 100, gid 101), kept so a /etc/omt volume
# written by an older image stays writable.
# hadolint ignore=DL3029
FROM --platform=$BUILDPLATFORM alpine:3.23.8@${ALPINE_DIGEST} AS runtime-rootfs

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG TARGETARCH
ARG RPI_OMT_CLIENT_VERSION=unknown

COPY --from=target-base / /rootfs
RUN case "${TARGETARCH}" in \
        arm64) arch=aarch64 ;; \
        amd64) arch=x86_64 ;; \
        *) echo "unsupported TARGETARCH ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && apk add --root /rootfs --arch "${arch}" --no-cache --no-scripts \
        --keys-dir "/usr/share/apk/keys/${arch}" \
        --repositories-file /rootfs/etc/apk/repositories \
        alsa-lib bash coreutils libcrypto3 libssl3 openssl util-linux \
    && [ -e "/rootfs/lib/ld-musl-${arch}.so.1" ] \
    && { grep -qx /bin/bash /rootfs/etc/shells || echo /bin/bash >> /rootfs/etc/shells; } \
    && printf '%s\n' 'omt:x:101:omt' >> /rootfs/etc/group \
    && printf '%s\n' 'omt:x:100:101::/etc/omt:/sbin/nologin' >> /rootfs/etc/passwd \
    && printf '%s\n' 'omt:!::0:::::' >> /rootfs/etc/shadow \
    && mkdir -p /rootfs/app/legal /rootfs/etc/omt /rootfs/run/omt \
    && chown 100:101 /rootfs/etc/omt /rootfs/run/omt \
    && chmod 700 /rootfs/run/omt

ARG RECEIVER_SOURCE_FINGERPRINT
# Buildah/Podman can otherwise reuse the cross-stage COPY below after the
# builder produced a different binary. Put the source fingerprint into this
# stage's parent configuration so a receiver change invalidates the COPY.
LABEL org.rpi-omt-client.receiver-source=${RECEIVER_SOURCE_FINGERPRINT}
COPY --from=runtime-artifacts /omt-receiver /rootfs/usr/local/bin/omt-receiver
COPY --from=runtime-artifacts /omt-web /rootfs/usr/local/bin/omt-web
COPY deploy/container/ /rootfs/usr/local/bin/
COPY LICENSE THIRD_PARTY_NOTICES.txt /rootfs/app/legal/

# The SBOM inventories the package database the target will ship, read from the
# rootfs; python3 is installed in this builder only, never in the image.
RUN apk add --no-cache python3
COPY scripts/generate-runtime-sbom.py rust-toolchain.toml /tmp/
RUN python3 /tmp/generate-runtime-sbom.py \
        --output /rootfs/app/legal/runtime-sbom.cdx.json \
        --apk-installed /rootfs/lib/apk/db/installed \
        --rust-toolchain /tmp/rust-toolchain.toml \
        --version "${RPI_OMT_CLIENT_VERSION}"

RUN chmod 0755 \
        /rootfs/usr/local/bin/omt-receiver \
        /rootfs/usr/local/bin/omt-web \
        /rootfs/usr/local/bin/control-omt.sh \
        /rootfs/usr/local/bin/start-omt.sh \
        /rootfs/usr/local/bin/entrypoint.sh \
    && printf '%s\n' "${RPI_OMT_CLIENT_VERSION}" > /rootfs/app/RPI_OMT_CLIENT_VERSION \
    && { \
        find /rootfs/app -type f ! -name runtime-sha256.manifest -print0; \
        find /rootfs/usr/local/bin -maxdepth 1 -type f -print0; \
    } | sort -z | xargs -0 sha256sum \
        | sed 's# /rootfs/# /#' > /rootfs/app/runtime-sha256.manifest

# hadolint ignore=DL3029
FROM --platform=$TARGETPLATFORM scratch AS runtime
# scratch has no identity of its own, so a config-only first instruction would
# be matched in the layer cache against the same instruction from a build for
# the other architecture, and the image would carry that build's platform over
# this one's contents. The platform in the first instruction keeps them apart.
ARG TARGETPLATFORM
LABEL org.rpi-omt-client.platform=${TARGETPLATFORM}
SHELL ["/bin/ash", "-eo", "pipefail", "-c"]
COPY --from=runtime-rootfs /rootfs/ /
ARG WEB_PORT=5000
ARG RPI_OMT_CLIENT_VERSION=unknown
ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

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
