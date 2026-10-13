#!/bin/bash
# Enforce the supply-chain policy of the Rust tree.
#
# The project is first-party Rust with no third-party crates (scripts/
# check-rust.sh holds Cargo.lock to the workspace). The only third-party
# library is OpenSSL; ALSA (on the appliance), the C library, the Linux uapi,
# and Win32 are the platform. This gate holds the tree to that:
#
#   * every native library a crate links -- a #[link(name = ...)] attribute
#     or a build script's cargo:rustc-link-lib -- is on the allowlist below,
#     so a new dependency cannot arrive as a stray link line;
#   * the OpenSSL the Windows deployer builds is pinned by version and SHA-256
#     to the 3.5 LTS series, and the toolbox's own OpenSSL is on that series;
#   * every base image is pinned by digest;
#   * the Unicode data the NFC tables are generated from is the pinned release.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"
failures=0
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# OpenSSL, ALSA, the C library's threads, and the Win32 system libraries.
ALLOWED_LIBRARY='^(ssl|crypto|asound|pthread|kernel32|ws2_32|bcrypt|crypt32|gdi32|user32|advapi32)$'

echo "Checking that crates link only platform and OpenSSL libraries..."
libraries="$(
    {
        grep -rhoE '#\[link\(name = "[^"]+"' crates --include='*.rs' | sed 's/.*"\(.*\)"/\1/'
        grep -rhoE 'rustc-link-lib=([a-z-]+=|\{kind\}=)?[A-Za-z0-9_]+"' crates --include='*.rs' |
            sed 's/.*=//; s/"$//'
        # A build script that links a list names it in a `for lib in [...]` loop.
        grep -rhoE 'for lib in \[[^]]*\]' crates --include='build.rs' | grep -oE '"[^"]+"' | tr -d '"'
    } | sort -u
)"
[[ -n "${libraries}" ]] || fail "no linked library was found; the patterns no longer match the crates"
while IFS= read -r library; do
    [[ "${library}" =~ ${ALLOWED_LIBRARY} ]] || fail "unexpected native library ${library}"
done <<<"${libraries}"
# A templated name the patterns above cannot read would slip through them.
if grep -rnE 'rustc-link-lib=([a-z-]+=)?\{' crates --include='*.rs' | grep -v 'for lib in' |
    grep -vE 'rustc-link-lib=dylib=\{lib\}|rustc-link-lib=\{kind\}=(ssl|crypto)' >/dev/null; then
    fail "a build script links a library through a template this gate cannot read"
fi

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
