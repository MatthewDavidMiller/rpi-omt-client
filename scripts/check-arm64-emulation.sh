#!/bin/bash
# Verify the container engine can execute ARM64 containers on this build host.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${PROJECT_ROOT}/scripts/docker-test-env.sh"
CHECK_IMAGE="docker.io/library/debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d"
# The same pinned installer scripts/install-arm64-emulation.sh takes its
# emulator from, run here for its other mode: registering the handler in the
# kernel it is pointed at.
BINFMT_IMAGE="docker.io/tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0"

# shellcheck disable=SC2310
if ! ensure_test_container_engine; then
    exit 1
fi

# buildx is a Docker-only prerequisite; Podman builds multi-arch natively once
# the binfmt handler is registered.
if [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]] && \
   ! "${CONTAINER_ENGINE}" buildx version >/dev/null 2>&1; then
    echo "ERROR: Docker buildx is not available." >&2
    exit 1
fi

# What the last probe printed, kept for the failure message. A probe whose
# output went to /dev/null reported "emulation is missing" for every possible
# cause, including the ones that are not that.
PROBE_OUTPUT=""

# `uname -m` is the whole probe, and it is passed as the container's command
# rather than through `--entrypoint /bin/sh`.
#
# Git Bash rewrites an argument that looks like an absolute POSIX path into a
# Windows one before the native docker.exe is executed, so `/bin/sh` reached
# Docker as `C:/Program Files/Git/usr/bin/sh` -- a path no Linux image has.
# That made this check fail on Windows workstations whose emulation was
# registered and working, which is exactly what it exists to tell apart. An
# argument with no leading slash cannot be rewritten.
arm64_runs() {
    PROBE_OUTPUT="$("${CONTAINER_ENGINE}" run --rm --platform linux/arm64 \
        "${CHECK_IMAGE}" uname -m 2>&1 | tr -d '\r')"
    # Pull progress precedes the answer on the first run, so only the last line
    # is the machine name.
    [[ "$(printf '%s\n' "${PROBE_OUTPUT}" | tail -n 1)" == "aarch64" ]]
}

# shellcheck disable=SC2310
if arm64_runs; then
    exit 0
fi

# On Windows the engine's Linux kernel is a VM this build cannot install into,
# and the registration it holds is lost whenever that VM restarts, so there is
# no one-time host setup to point at: `make setup-arm64-emulation` installs a
# systemd binfmt unit and does not run here at all. Register it in the VM and
# ask again. On Linux the handler belongs in the host kernel, where that script
# installs it persistently and verifies it as root -- silently registering it
# from a build would be a less accountable install of the same thing.
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        echo "Registering ARM64 emulation in the ${CONTAINER_ENGINE_KIND} VM..." >&2
        # shellcheck disable=SC2310
        if "${CONTAINER_ENGINE}" run --rm --privileged "${BINFMT_IMAGE}" \
               --install arm64,arm >&2 && arm64_runs; then
            echo "ARM64 emulation is registered." >&2
            exit 0
        fi
        cat >&2 <<EOF
ERROR: ${CONTAINER_ENGINE_KIND} cannot execute linux/arm64 containers, and
registering the emulator from the pinned binfmt image did not change that.

What the probe reported:
${PROBE_OUTPUT}

If that says the emulator is already registered, the engine is running but
cannot run this image; if Docker Desktop is in Windows-container mode, switch
it to Linux containers under Settings > General and run the build again.
EOF
        exit 1
        ;;
esac

cat >&2 <<EOF
ERROR: ${CONTAINER_ENGINE_KIND} cannot execute linux/arm64 containers on this build host.

What the probe reported:
${PROBE_OUTPUT}

The ARM64 runtime image runs package installation steps during the container
build, so ARM64 emulation must be registered. Install it once, then rerun the
build:

  make setup-arm64-emulation
EOF
exit 1
