#!/bin/bash
# Install the repository-pinned Hadolint binary after checksum verification.

set -euo pipefail

HADOLINT_VERSION="v2.14.0"
HADOLINT_LINUX_X86_64_SHA256="6bf226944684f56c84dd014e8b979d27425c0148f61b3bd99bcc6f39e9dc5a47"

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
    echo "ERROR: pinned binary installation supports Linux x86_64 only." >&2
    echo "Install Hadolint with the platform package manager instead." >&2
    exit 1
fi

download_dir="$(mktemp -d)"
trap 'rm -rf "${download_dir}"' EXIT
binary="${download_dir}/hadolint-Linux-x86_64"

curl -fsSLo "${binary}" \
    "https://github.com/hadolint/hadolint/releases/download/${HADOLINT_VERSION}/hadolint-Linux-x86_64"
printf '%s  %s\n' "${HADOLINT_LINUX_X86_64_SHA256}" "${binary}" | sha256sum -c -
sudo install -m 0755 "${binary}" /usr/local/bin/hadolint
