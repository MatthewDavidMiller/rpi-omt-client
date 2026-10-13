#!/bin/sh
# The linker cargo uses for the musl targets built against an Alpine sysroot:
#   cargo-linker.sh <arch> <linker args...>
# (.cargo/config.toml names scripts/cargo-linker-aarch64.sh; the appliance's
# builder stage points the target it builds at the matching wrapper.)
#
# The appliance links dynamically against Alpine's own musl, OpenSSL, and
# ALSA, so Rust objects are linked by clang against an Alpine sysroot
# (scripts/make-sysroot.sh) with lld.
# OMT_SYSROOT names it: /opt/sysroot-aarch64 in the toolbox, /sysroot in the
# appliance's builder stage.
#
# rustc asks for the shared unwinder (-lgcc_s) on a dynamically linked musl
# target. The runtime image carries no libgcc, so the static unwinder GCC
# installs beside libgcc.a is linked in its place, and deploy/Dockerfile
# checks that nothing needs libgcc_s. A static build (the Linux deployer)
# asks for LLVM's libunwind instead, which Alpine ships no static copy of;
# GCC's static unwinder implements the same _Unwind interface.
arch="$1"
shift
first=1
for arg in "$@"; do
    if [ "${first}" = 1 ]; then
        set --
        first=0
    fi
    case "${arg}" in
        -lgcc_s | -lunwind) set -- "$@" -lgcc_eh ;;
        *) set -- "$@" "${arg}" ;;
    esac
done
exec clang --target="${arch}-alpine-linux-musl" \
    --sysroot="${OMT_SYSROOT:-/opt/sysroot-${arch}}" \
    -fuse-ld=lld "$@"
