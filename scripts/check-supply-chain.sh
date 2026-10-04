#!/bin/bash
# Enforce the supply-chain policy of the C tree.
#
# The project is first-party C. The only third-party library is OpenSSL; ALSA
# (on the appliance), the C library, the Linux uapi, and Win32 are the
# platform. This gate holds the tree to that:
#
#   * every <system> header a source includes is on the allowlist below, so a
#     new dependency cannot arrive as a stray #include;
#   * the OpenSSL the Windows deployer builds is pinned by version and SHA-256
#     to the 3.5 LTS series, and the toolbox's own OpenSSL is on that series;
#   * every base image is pinned by digest;
#   * the Unicode data the NFC tables are generated from is the pinned release.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"
failures=0
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# The C standard library, POSIX, Linux, Win32, OpenSSL, and ALSA headers.
ALLOWED_HEADER='^(alloca|errno|fcntl|inttypes|limits|math|netdb|poll|process|pthread|signal|spawn|stdarg|stdatomic|stdbool|stddef|stdint|stdio|stdlib|string|termios|time|unistd|wchar|dirent|arm_neon)\.h$|^(sys|arpa|netinet)/[a-z_]+\.h$|^drm/drm(_mode|_fourcc)?\.h$|^(windows|winsock2|ws2tcpip|wincrypt|bcrypt)\.h$|^openssl/[a-z0-9_]+\.h$|^alsa/asoundlib\.h$'

echo "Checking that sources include only platform and OpenSSL headers..."
while IFS= read -r header; do
    [[ "${header}" =~ ${ALLOWED_HEADER} ]] || fail "unexpected system header <${header}>"
done < <(grep -rhoE '^#include <[^>]+>' src tests/c tests/fuzz | sed 's/^#include <\(.*\)>$/\1/' | sort -u)

echo "Checking the OpenSSL pin..."
version="$(sed -n 's/^OPENSSL_VERSION="\(.*\)"$/\1/p' scripts/build-openssl.sh)"
checksum="$(sed -n 's/^OPENSSL_SHA256="\(.*\)"$/\1/p' scripts/build-openssl.sh)"
[[ "${version}" =~ ^3\.5\.[0-9]+$ ]] || fail "the Windows OpenSSL pin (${version}) is not on the 3.5 LTS series"
[[ "${checksum}" =~ ^[0-9a-f]{64}$ ]] || fail "the Windows OpenSSL pin has no SHA-256"
# The toolbox's OpenSSL is the one the static Linux deployer links; a host
# run checks only the pin.
if [[ -n "${OMT_IN_TOOLBOX:-}" ]]; then
    system="$(openssl version | awk '{print $2}')"
    [[ "${system}" == 3.5.* ]] || fail "the toolbox OpenSSL (${system}) is not on the 3.5 LTS series"
fi

echo "Checking base image pins..."
for dockerfile in deploy/Dockerfile tools/toolbox/Dockerfile; do
    while IFS= read -r line; do
        image="$(awk '{for (i = 2; i <= NF; i++) if ($i !~ /^--/) {print $i; exit}}' <<<"${line}")"
        [[ "${image}" == scratch || "${image}" == *@* || ! "${image}" =~ [:/] ]] ||
            fail "${dockerfile}: ${image} is not pinned by digest"
    done < <(grep -E '^FROM ' "${dockerfile}")
done

echo "Checking the Unicode data pin..."
[[ -s third_party/unicode/16.0.0/nfc-data.txt ]] || fail "the pinned Unicode 16.0.0 extract is missing"

if ((failures > 0)); then
    echo "${failures} supply-chain finding(s)" >&2
    exit 1
fi
echo "Supply-chain gates passed."
