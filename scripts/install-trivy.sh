#!/bin/bash
# Install the repository-pinned Trivy binary after checksum verification.
# The pre-commit gate scans the tree and the runtime image with it, so a
# workstation without Trivy cannot complete a commit.

set -euo pipefail

TRIVY_VERSION="0.73.0"
TRIVY_LINUX_X86_64_SHA256="2edd39da482bb4e9831962487b68f68e3928ec3137794757f54d00383d79547b"
TRIVY_LINUX_ARM64_SHA256="13833d97e8a1a5367471c372a173180157f593bece570e20d5d925fef552f5dd"

# The toolbox image runs this as root during its build, where there is no sudo
# to call. Escalate only when the caller is not already root.
if [[ "$(id -u)" -eq 0 ]]; then
    sudo() { "$@"; }
fi

case "$(uname -s)/$(uname -m)" in
    Linux/x86_64)
        archive_name="trivy_${TRIVY_VERSION}_Linux-64bit.tar.gz"
        expected_sha256="${TRIVY_LINUX_X86_64_SHA256}"
        ;;
    Linux/aarch64|Linux/arm64)
        archive_name="trivy_${TRIVY_VERSION}_Linux-ARM64.tar.gz"
        expected_sha256="${TRIVY_LINUX_ARM64_SHA256}"
        ;;
    *)
        echo "ERROR: pinned binary installation supports Linux x86_64 and arm64 only." >&2
        echo "Install Trivy with the platform package manager instead." >&2
        exit 1
        ;;
esac

download_dir="$(mktemp -d)"
trap 'rm -rf "${download_dir}"' EXIT
archive="${download_dir}/${archive_name}"

curl -fsSLo "${archive}" \
    "https://github.com/aquasecurity/trivy/releases/download/v${TRIVY_VERSION}/${archive_name}"
printf '%s  %s\n' "${expected_sha256}" "${archive}" | sha256sum -c -
tar -xzf "${archive}" -C "${download_dir}" trivy
sudo install -m 0755 "${download_dir}/trivy" /usr/local/bin/trivy
