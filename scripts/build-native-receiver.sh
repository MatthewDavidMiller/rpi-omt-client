#!/bin/ash
# Build the native C/C++ receiver from the stable amd64 compiler stage.

set -eu

target_arch="${1:-}"
target_sysroot="${2:-}"
version="${RPI_OMT_CLIENT_VERSION:-unknown}"

build_dir=/tmp/receiver-build
rm -rf "${build_dir}"
mkdir -p "${build_dir}" /out/receiver

case "${target_arch}" in
    amd64)
        cmake -S /src -B "${build_dir}" -G Ninja \
            -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_C_COMPILER=clang \
            -DCMAKE_CXX_COMPILER=clang++ \
            -DOMT_BUILD_RECEIVER=ON \
            -DOMT_BUILD_DEPLOYER=OFF \
            -DOMT_BUILD_TESTS=OFF \
            -DOMT_CLIENT_VERSION="${version}"
        ;;
    arm64)
        if [ ! -f "${target_sysroot}/etc/os-release" ]; then
            echo "ERROR: ARM64 Alpine sysroot is missing." >&2
            exit 1
        fi
        export OMT_ARM64_SYSROOT="${target_sysroot}"
        export PKG_CONFIG_SYSROOT_DIR="${target_sysroot}"
        export PKG_CONFIG_LIBDIR="${target_sysroot}/usr/lib/pkgconfig:${target_sysroot}/usr/share/pkgconfig"
        cmake -S /src -B "${build_dir}" -G Ninja \
            -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_TOOLCHAIN_FILE=/src/cmake/toolchains/alpine-aarch64.cmake \
            -DCMAKE_EXE_LINKER_FLAGS=-fuse-ld=lld \
            -DOMT_BUILD_RECEIVER=ON \
            -DOMT_BUILD_DEPLOYER=OFF \
            -DOMT_BUILD_TESTS=OFF \
            -DOMT_CLIENT_VERSION="${version}"
        ;;
    *)
        echo "ERROR: unsupported target architecture: ${target_arch}" >&2
        exit 1
        ;;
esac

cmake --build "${build_dir}" --target omt-receiver --parallel 2
install -m 0755 "${build_dir}/src/native/receiver/omt-receiver" /out/receiver/omt-receiver
llvm-strip /out/receiver/omt-receiver
rm -rf "${build_dir}"
