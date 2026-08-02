#!/bin/ash
# Build the NativeAOT receiver and libvmx from the stable amd64 compiler stage.

set -eu

target_arch="${1:-}"
target_sysroot="${2:-}"
version="${RPI_OMT_CLIENT_VERSION:-unknown}"

mkdir -p /out/native

case "${target_arch}" in
    amd64)
        clang++ -O3 -std=c++17 -fdeclspec -fPIC -mlzcnt -mavx2 -mbmi -shared \
            third_party/omt/libvmx/src/vmxcodec_x86.cpp \
            third_party/omt/libvmx/src/vmxcodec_avx2.cpp \
            third_party/omt/libvmx/src/vmxcodec.cpp \
            -Wl,-rpath,'$ORIGIN' -o /out/native/libvmx.so
        runtime_id=linux-musl-x64
        ;;
    arm64)
        if [ ! -f "${target_sysroot}/etc/os-release" ]; then
            echo "ERROR: ARM64 Alpine sysroot is missing." >&2
            exit 1
        fi
        clang++ --target=aarch64-alpine-linux-musl \
            --sysroot="${target_sysroot}" -fuse-ld=bfd \
            -O3 -std=c++17 -fdeclspec -fPIC -shared \
            third_party/omt/libvmx/src/vmxcodec_arm.cpp \
            third_party/omt/libvmx/src/vmxcodec.cpp \
            -Wl,-rpath,'$ORIGIN' -o /out/native/libvmx.so
        runtime_id=linux-musl-arm64
        ;;
    *)
        echo "ERROR: unsupported target architecture: ${target_arch}" >&2
        exit 1
        ;;
esac

set -- \
    --configuration Release \
    --runtime "${runtime_id}" \
    --self-contained true \
    --output /out/receiver \
    -p:InformationalVersion="${version}" \
    -p:DebugType=None \
    -p:DebugSymbols=false

if [ "${target_arch}" = "arm64" ]; then
    set -- "$@" \
        -p:SysRoot="${target_sysroot}" \
        -p:LinkerFlavor=bfd \
        -p:ObjCopyName=/usr/aarch64-alpine-linux-musl/bin/objcopy
fi

dotnet publish src/receiver/RpiOmt.Receiver/RpiOmt.Receiver.csproj "$@"

# These files are useful only while compiling. Keeping them in a completed
# builder layer previously added hundreds of MiB per test run.
find /src -type d \( -name bin -o -name obj \) -prune -exec rm -rf {} +
rm -rf /root/.nuget/packages
