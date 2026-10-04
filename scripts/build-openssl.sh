#!/bin/bash
# Build the pinned OpenSSL the Windows deployer links statically.
#
# OpenSSL is the one third-party library in the project. The appliance links
# Alpine's shared libssl/libcrypto, so apk security updates reach it; the
# static Linux deployer links Alpine's openssl-libs-static from the toolbox.
# Windows has no system OpenSSL, so the deployer carries this one, built from
# the release tarball OpenSSL publishes and refused unless its SHA-256 is the
# one pinned here.
#
# Usage: scripts/build-openssl.sh <target-triple>     (x86_64-w64-mingw32)
# Prints the install prefix on success. Builds are cached by version.
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OPENSSL_VERSION="3.5.9"
OPENSSL_SHA256="603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a"
OPENSSL_URL="https://github.com/openssl/openssl/releases/download/openssl-${OPENSSL_VERSION}/openssl-${OPENSSL_VERSION}.tar.gz"

TARGET="${1:-}"
case "${TARGET}" in
    x86_64-w64-mingw32) CONFIGURE_TARGET=mingw64 ;;
    *) echo "Usage: $0 x86_64-w64-mingw32" >&2; exit 2 ;;
esac

CACHE="${PROJECT_ROOT}/.build/openssl"
TARBALL="${CACHE}/openssl-${OPENSSL_VERSION}.tar.gz"
PREFIX="${CACHE}/${OPENSSL_VERSION}/${TARGET}"
mkdir -p "${CACHE}"

if [[ -f "${PREFIX}/lib/libcrypto.a" && -f "${PREFIX}/lib/libssl.a" ]]; then
    echo "${PREFIX}"
    exit 0
fi

if [[ ! -f "${TARBALL}" ]]; then
    curl -fsSL --retry 3 --max-time 300 -o "${TARBALL}.partial" "${OPENSSL_URL}" >&2
    mv "${TARBALL}.partial" "${TARBALL}"
fi
actual="$(sha256sum "${TARBALL}" | cut -d' ' -f1)"
if [[ "${actual}" != "${OPENSSL_SHA256}" ]]; then
    echo "ERROR: ${TARBALL} has SHA-256 ${actual}, expected ${OPENSSL_SHA256}" >&2
    rm -f "${TARBALL}"
    exit 1
fi

WORK="$(mktemp -d "${CACHE}/build.XXXXXX")"
trap 'rm -rf "${WORK}"' EXIT
tar -xzf "${TARBALL}" -C "${WORK}"
cd "${WORK}/openssl-${OPENSSL_VERSION}"
# Library only: no shared objects, no engines or dynamic loading, no tests,
# docs, or command-line tool. The deprecated APIs stay out too, so nothing
# the deployer calls can lean on them.
./Configure "${CONFIGURE_TARGET}" \
    --cross-compile-prefix="${TARGET}-" \
    --prefix="${PREFIX}" --libdir=lib \
    no-shared no-dso no-engine no-module no-tests no-docs no-apps \
    no-deprecated no-legacy no-ssl3 no-comp >&2
make -j"$(nproc)" build_libs >&2
make install_dev >&2
echo "${PREFIX}"
