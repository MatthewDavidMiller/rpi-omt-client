#!/bin/bash
# Verify that Docker can execute ARM64 containers on the current build host.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${PROJECT_ROOT}/scripts/docker-test-env.sh"
CHECK_IMAGE="docker.io/library/debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d"

# shellcheck disable=SC2310
if ! ensure_docker_daemon; then
    exit 1
fi

if ! docker buildx version >/dev/null 2>&1; then
    echo "ERROR: Docker buildx is not available." >&2
    exit 1
fi

if docker run --rm --platform linux/arm64 --entrypoint /bin/sh "${CHECK_IMAGE}" -c 'test "$(uname -m)" = "aarch64"' >/dev/null 2>&1; then
    exit 0
fi

cat >&2 <<'EOF'
ERROR: Docker cannot execute linux/arm64 containers on this build host.

The ARM64 runtime image runs package installation steps during the Docker build,
so buildx needs ARM64 emulation registered. Install it once, then rerun the
build:

  make setup-arm64-emulation

On Docker Desktop, also make sure the Linux engine is running before retrying.
EOF
exit 1
