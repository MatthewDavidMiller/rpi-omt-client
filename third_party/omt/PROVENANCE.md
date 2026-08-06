# Open Media Transport provenance

The Raspberry Pi OMT Client vendors a C17 port of the MIT-licensed `libvmx`
decoder. Its
native wire transport and playback code were independently ported from the
listed MIT-licensed revisions; the former C# source snapshots are deliberately
not vendored or built.

| Component | Upstream | Commit |
|---|---|---|
| `omtplayer` playback core | https://github.com/openmediatransport/omtplayer | `c47397aa74600f998d9fa8533a26b0dbbf9e26e9` |
| `libomtnet` | https://github.com/openmediatransport/libomtnet | `bda284e86ea56166b0caa45f30136def8c893e5e` |
| `libvmx` | https://github.com/openmediatransport/libvmx | `f73569e767b9d9177519bf5765c9434dfe8af51f` |

The C17 translation preserves the public VMX ABI and SIMD algorithms from the
recorded upstream revision while replacing its C++ allocation, threading, and
callback machinery with explicit C ownership and platform synchronization.

The native implementation adds bounded frame parsing, conservative source and
target validation, direct DRM/ALSA output, lifecycle/status publication, and a
hardened command line under `src/native/`.
