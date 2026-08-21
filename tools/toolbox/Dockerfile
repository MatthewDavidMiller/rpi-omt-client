# The gate toolbox: every linter, test runner, compiler, and scanner the
# repository gates depend on, so a workstation needs nothing but Docker or
# Podman. scripts/toolbox.sh runs the gates inside this image.
#
# The base is the same digest-pinned official Rust Alpine image that
# deploy/Dockerfile compiles the receiver with, so the gates and the shipped
# appliance resolve one compiler rather than two that may drift. Its musl host
# is also what makes the static Linux deployer a native build instead of a
# cross-compile.
# This image is a developer tool, not a shipped artifact: it is never published,
# never serves traffic, and exists only to run this repository's own gates.
#
# It declares no USER on purpose, which .trivyignore.yaml records as a scoped
# exception rather than silencing the rule everywhere. scripts/toolbox.sh supplies the uid at run
# time -- the invoking user's, so a gate leaves no root-owned files in the tree
# -- and that uid is not known when the image is built. Baking one in would be
# overridden on Docker and would collide with Podman's --userns=keep-id
# mapping, where the caller's uid is whatever it happens to be.

ARG RUST_DIGEST=sha256:c4a364ddbf684fe038e6fa6a4f25b30c8dc85247423e0e660676ece0d17be4a2

# hadolint ignore=DL3029,DL3006
FROM rust:1.97.1-alpine3.23@${RUST_DIGEST}

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

# alsa-lib-dev is a lint dependency, not a runtime one: `cargo clippy
# --workspace` compiles omt-receiver, which binds ALSA. mingw-w64-gcc carries
# both the Windows cross-linker and the objdump that
# scripts/verify-windows-deployer.sh reads PE headers with. The docker CLI and
# its buildx and compose plugins talk to the host engine's socket: the
# appliance image build, the dev compose stack, and the Trivy image scan all
# run on the host daemon, driven from in here.
#
# coreutils, findutils, gawk, grep, sed, and diffutils replace busybox's
# applets with the GNU ones. The gates and their scripts are written for the
# GNU userland every developer Linux host has, and busybox rejects extensions
# like `grep --include`. The suites that deliberately hold appliance scripts to
# POSIX/busybox behaviour invoke `sh` or `dash` explicitly, so they still test
# what they mean to.
#
# qemu-aarch64 runs the appliance's NEON decoder under emulation, so the
# kernel that ships on the Pi is the one the differential test checked.
#
# fakeroot and util-linux-misc's `unshare` are what let the host-event-watcher
# and diagnostics suites exercise root-owned paths: the toolbox runs as the
# invoking user so the tree it writes to stays theirs, and those tests need a
# root that is not the host's.
RUN apk add --no-cache \
        alsa-lib-dev \
        bash \
        binutils \
        build-base \
        coreutils \
        curl \
        dash \
        diffutils \
        docker-cli \
        docker-cli-buildx \
        docker-cli-compose \
        fakeroot \
        file \
        findutils \
        gawk \
        git \
        grep \
        iproute2 \
        make \
        mingw-w64-gcc \
        musl-dev \
        nftables \
        openssh-client \
        openssl \
        perl \
        pkgconf \
        py3-pip \
        python3 \
        python3-dev \
        qemu-aarch64 \
        sed \
        shellcheck \
        tar \
        util-linux-misc \
        xz

# The official image ships clippy-driver and rustfmt as binaries but does not
# register them as toolchain components, so `cargo clippy` and `cargo fmt` --
# which is what scripts/lint.sh actually runs -- both fail until this adds
# them.
#
# x86_64-pc-windows-gnu cross-builds the Windows deployer. aarch64 musl runs
# the appliance's NEON decoder under emulation. The x86_64 musl host target is
# already present; the static Linux deployer builds against it explicitly.
RUN rustup component add clippy rustfmt \
    && rustup target add x86_64-pc-windows-gnu aarch64-unknown-linux-musl

# scripts/check-supply-chain.sh gates every commit on these.
RUN cargo install --locked cargo-deny cargo-vet

# Reuse the repository's own pinned installers rather than restating the
# versions and checksums here, so the toolbox cannot drift from what a host
# install of the same tool would produce.
COPY scripts/install-hadolint.sh scripts/install-trivy.sh /tmp/installers/
RUN /tmp/installers/install-hadolint.sh \
    && /tmp/installers/install-trivy.sh \
    && rm -rf /tmp/installers

# Alpine marks its Python installation externally managed, so the tooling lives
# in a virtualenv exactly as it does on a host -- baked into the image here
# instead of into the mounted tree, so a gate run never writes to tests/.venv.
COPY tests/requirements-dev.txt /tmp/requirements-dev.txt
RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/python -m pip install --no-cache-dir --upgrade pip \
    && /opt/venv/bin/python -m pip install --no-cache-dir -r /tmp/requirements-dev.txt \
    && rm /tmp/requirements-dev.txt

# scripts/lint.sh and scripts/test-local.sh resolve the Python tooling through
# this, falling back to tests/.venv when it is unset for a host run.
ENV OMT_PYTHON_VENV=/opt/venv
ENV PATH="/opt/venv/bin:${PATH}"

# scripts/toolbox.sh runs a command directly when it sees this rather than
# starting another container. The gates call each other -- the pre-commit hook
# runs test-local.sh, which runs lint.sh -- and each of those goes through the
# toolbox, so without this a single commit would nest containers several deep.
ENV OMT_IN_TOOLBOX=1

# Cargo's registry cache lives here, on a named volume scripts/toolbox.sh
# mounts, so downloads survive between gate runs.
#
# CARGO_TARGET_DIR is deliberately NOT set. Leaving it at its default puts the
# build tree at <repo>/target, exactly where a host build puts it, so the
# artifacts a gate produces are the ones a developer finds afterwards and the
# scripts and tests that name `target/debug/...` are not quietly reading a
# directory that does not exist in here.
ENV CARGO_HOME=/cargo

# /cargo is world-writable because the container runs as whichever uid invoked
# it. /run/netns is where `ip netns` bind-mounts namespace handles and where
# the firewall gate mounts its stage directory: Alpine's base image has none,
# and a bind mount cannot create its own target. The git setting is for the
# bind-mounted repository, which looks foreign-owned to a mapped user; the
# gates call git for version detection.
RUN mkdir -p /cargo /run/netns \
    && chmod 1777 /cargo \
    && git config --system --add safe.directory '*'

WORKDIR /work
