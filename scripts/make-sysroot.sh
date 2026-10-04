#!/bin/sh
# Install an Alpine sysroot for one target architecture.
#
# Usage: make-sysroot.sh <apk-arch> <directory>
#
# The appliance's C is compiled on the build machine with clang
# --target=<arch>-alpine-linux-musl --sysroot=<directory> instead of being
# compiled under QEMU, which took tens of minutes per build. The sysroot holds
# the target's headers, C library, compiler runtime, and the ALSA and OpenSSL
# the appliance links, taken from the same Alpine release and repositories as
# the image running this script -- deploy/Dockerfile and tools/toolbox both
# call it from the digest-pinned base -- so the cross build links exactly what
# a native one on that release would.
#
# gcc is in the list for its crtbegin/crtend objects and libgcc, which Alpine's
# clang links against natively; leaving it out would change the compiler
# runtime rather than only where the compiler runs. fortify-headers is what
# _FORTIFY_SOURCE means on musl: a native build-base pulls it in, and without
# it the define compiles to nothing and the hardening is silently lost.
#
# Package scripts are not run: they would execute target binaries, and nothing
# they do matters to a tree that is only ever read by a compiler and a linker.
# The runtime
# libraries the -dev packages pull in also let qemu-<arch> -L <directory> run
# the cross-built test suites.
#
# POSIX sh, because the Alpine stages that run it have no bash.
set -eu

[ "$#" -eq 2 ] || { echo "Usage: $0 <apk-arch> <directory>" >&2; exit 2; }
arch="$1"
root="$2"
case "${arch}" in
    aarch64 | x86_64) ;;
    *) echo "ERROR: unsupported sysroot architecture ${arch}" >&2; exit 2 ;;
esac

mkdir -p "${root}/etc/apk"
cp /etc/apk/repositories "${root}/etc/apk/repositories"
apk add --root "${root}" --arch "${arch}" --initdb --no-cache --no-scripts \
    --keys-dir "/usr/share/apk/keys/${arch}" \
    --repositories-file "${root}/etc/apk/repositories" \
    alsa-lib-dev fortify-headers gcc linux-headers musl-dev openssl-dev >/dev/null
# The C library's loader is what qemu-<arch> -L looks up; its absence would
# surface later as an opaque "No such file or directory" from every test.
[ -e "${root}/lib/ld-musl-${arch}.so.1" ] || {
    echo "ERROR: ${root} has no musl loader for ${arch}" >&2
    exit 1
}
[ -e "${root}/usr/include/fortify/string.h" ] || {
    echo "ERROR: ${root} has no fortify headers; _FORTIFY_SOURCE would do nothing" >&2
    exit 1
}
