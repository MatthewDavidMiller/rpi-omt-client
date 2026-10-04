# The gate toolbox: every linter, test runner, compiler, and scanner the
# repository gates depend on, so a workstation needs nothing but Docker or
# Podman. scripts/toolbox.sh runs the gates inside this image.
#
# The base is the same digest-pinned Alpine release deploy/Dockerfile builds
# the appliance on, so the gates and the shipped appliance resolve one
# compiler and one OpenSSL rather than two that may drift. Its musl host is
# also what makes the static Linux deployer a native build instead of a
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

ARG ALPINE_DIGEST=sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40

# hadolint ignore=DL3029,DL3006
FROM alpine:3.23.5@${ALPINE_DIGEST}

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

# alsa-lib-dev is a build dependency, not a runtime one: the C gates compile
# omt-receiver, which binds ALSA. mingw-w64-gcc carries both the Windows
# cross-compiler and the objdump that scripts/verify-windows-deployer.sh reads
# PE headers with. openssl-dev and openssl-libs-static are the OpenSSL the
# appliance links dynamically and the static Linux deployer links statically. The docker CLI and
# its buildx and compose plugins talk to the host engine's socket: the
# appliance image build, the dev compose stack, and the Trivy image scan all
# run on the host daemon, driven from in here.
#
# podman-remote is the second client for that same socket, and it is not
# redundant. A Podman server asked to build through the Docker CLI stores the
# result as OCI, whose image config has nowhere to keep a HEALTHCHECK, so the
# appliance image comes out without the probe deploy/Dockerfile declares and
# nothing downstream notices. `podman build --format docker` keeps it, and
# only Podman's own client can ask for that. scripts/docker-test-env.sh
# selects between the two on what serves the socket, not on what the gate is
# named. Alpine's subpackage installs that client as `podman-remote` alone, so
# the symlink gives it the name every caller already spells.
#
# Deleting containers-common's seccomp.json is what makes that client usable
# against a server it does not share a filesystem with. Buildah picks the
# profile to apply by looking for /etc/containers/seccomp.json and falling back
# to /usr/share/containers/seccomp.json, and for a build it does that lookup
# on the *client* and sends the winning path to the server. Alpine ships the
# profile at both paths; Arch, Debian, and Fedora ship it only under
# /usr/share. So the file's presence in here made every build fail on the host
# with `opening seccomp profile failed: /etc/containers/seccomp.json`, naming a
# path that exists in this image and not on the machine that has to open it.
# Removing the override leaves the fallback, which is Podman's own documented
# default and the one path a containers-common host is known to have.
#
# This weakens nothing: the profile is applied by the server either way, and
# the one now named is the default that server would otherwise pick. The
# alternative fixes are worse -- `seccomp=unconfined` drops the sandbox, and
# passing an explicit --security-opt would hardcode one distribution's path
# into every build call. containers.conf's `seccomp_profile` does not help,
# because buildah's override lookup runs ahead of it.
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
# The C tree's gates: clang (with compiler-rt and llvm-ar) builds the
# sanitizer and libFuzzer configurations -- Alpine's GCC ships no libasan for
# musl -- while GCC provides -fanalyzer; clang-extra-tools carries the
# clang-format the formatting gate pins, and cppcheck is the third static
# check. linux-headers supplies the DRM uapi the receiver includes. dbus,
# py3-dbus, and py3-gobject3 run the receiver's D-Bus interop test, which needs
# a real bus daemon and a scriptable fake Avahi on the system interpreter.
#
# openssh-server and openssh-sftp-server are the reference the deployer's
# first-party SSH client is held to: tests/integration/test_ssh_client.sh runs
# an unprivileged sshd on loopback and restricts it to each algorithm in turn.
# Nothing in here listens on a port the gate did not open itself.
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
        clang \
        clang-extra-tools \
        compiler-rt \
        coreutils \
        cppcheck \
        curl \
        dash \
        dbus \
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
        linux-headers \
        llvm \
        make \
        mingw-w64-gcc \
        musl-dev \
        nftables \
        openssh-client \
        openssh-keygen \
        openssh-server \
        openssh-sftp-server \
        openssl \
        openssl-dev \
        openssl-libs-static \
        perl \
        pkgconf \
        podman-remote \
        py3-dbus \
        py3-gobject3 \
        py3-pip \
        python3 \
        python3-dev \
        qemu-aarch64 \
        sed \
        shellcheck \
        tar \
        util-linux-misc \
        xz \
    && ln -s /usr/bin/podman-remote /usr/local/bin/podman \
    && rm -f /etc/containers/seccomp.json



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

# HOME for whichever uid runs the gates: a named volume scripts/toolbox.sh
# mounts here, world-writable because that uid is not known at build time.
# /run/netns is where `ip netns` bind-mounts namespace handles and where the
# firewall gate mounts its stage directory: Alpine's base image has none, and
# a bind mount cannot create its own target. The git setting is for the
# bind-mounted repository, which looks foreign-owned to a mapped user; the
# gates call git for version detection.
RUN mkdir -p /toolbox-home /run/netns \
    && chmod 1777 /toolbox-home \
    && git config --system --add safe.directory '*'

WORKDIR /work
