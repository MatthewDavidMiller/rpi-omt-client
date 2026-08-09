#!/bin/bash
# Contract tests for the published ARM64 deployment artifact.
#
# The artifact is named omt-client-arm64.tar.gz and is uploaded over SSH to a
# Raspberry Pi on every deploy. Both container engines emit an *uncompressed*
# tar, so for a long time the name was simply wrong and each deploy pushed
# ~86 MiB where ~27 MiB would do. `docker load` sniffs the archive rather than
# trusting the name, so the compression belongs in the build.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
BUILD="${ROOT}/scripts/build-arm64.sh"
INSTALL="${ROOT}/deploy/host/install.sh"
ARTIFACT="${ROOT}/omt-client-arm64.tar.gz"

failures=0
fail() {
    echo "FAIL: $1" >&2
    failures=$((failures + 1))
}

[[ -f "${BUILD}" ]] || {
    echo "FAIL: scripts/build-arm64.sh is missing" >&2
    exit 1
}
bash -n "${BUILD}"

grep -Eq 'gzip -9 -n' "${BUILD}" || \
    fail "the ARM64 archive must be gzip compressed, matching its .tar.gz name"
# -n keeps the gzip header free of a name and timestamp, so an unchanged image
# still produces a byte-identical artifact.
grep -Eq 'gzip .*-n' "${BUILD}" || \
    fail "gzip must omit the name and timestamp so rebuilds stay reproducible"
grep -Eq 'tar -tzf' "${BUILD}" || \
    fail "the compressed archive must be verified as gzip before it is published"

# The appliance side must not have grown a decompression step of its own:
# `docker load` detects gzip, and a pipeline through gunzip would break the
# moment the artifact format changed again.
grep -Eq 'docker load < "\$\{TARBALL\}"' "${INSTALL}" || \
    fail "the installer must feed the archive straight to docker load"
if grep -Eq 'gunzip|zcat' "${INSTALL}"; then
    fail "the installer must let docker load sniff the archive, not decompress it"
fi

# When a build has been run in this tree, the bytes must agree with the name.
if [[ -f "${ARTIFACT}" ]]; then
    if [[ "$(head -c 2 "${ARTIFACT}" | od -An -tx1 | tr -d ' ')" != "1f8b" ]]; then
        fail "omt-client-arm64.tar.gz is present but is not gzip data"
    elif ! tar -tzf "${ARTIFACT}" >/dev/null 2>&1; then
        fail "omt-client-arm64.tar.gz is gzip but not a readable tar archive"
    fi
fi

if ((failures > 0)); then
    echo "${failures} ARM64 artifact contract test(s) failed" >&2
    exit 1
fi

echo "ARM64 artifact contract tests passed"
