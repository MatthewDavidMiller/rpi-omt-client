# VMX Codec

VMX is video codec designed for very high encoding/decoding performance in software.
VMX began as the "vMix Video Codec" used for the vMix Instant Replay feature.
It has since been expanded with alpha channel support and incorporated into the Open Media Transport protocol.

## Features
* Low latency intra-frame only
* 4:2:2:4 with alpha channel support
* 10 bit support
* Highly optimized AVX2 code capable of encoding 2160p60 on a single Intel i7 core
* Cross platform. (Windows, Mac, Linux) with ARM NEON support provided via sse2neon

## License
VMX is distributed under a permissive MIT license. Further details can be found in the LICENSE file

## System Requirements

### x86_64
Minimum: 64bit CPU with SSE4.2 and SSSE3
Recommended: 64bit CPU with AVX2 support (Intel Haswell (2013) and newer)

### ARM64
64bit ARM CPU with NEON instructions (ARMv8)

## Documentation

Documentation can be found at: https://www.openmediatransport.org/docs/

## Getting Started

Binaries with Library files are available in the Releases section of the libomtnet repo.
These are available for Windows and MacOS and are the recommended way to get started.

### C/C++
Include vmxcodec.h, reference libvmx.lib and you're ready to go.

## Compiling

This repository builds the required decoder sources through its root CMake
project. Use the documented native receiver or deployer build commands rather
than invoking the upstream project files directly.
