# Open Media Transport provenance

The Raspberry Pi OMT Client carries a clean, decode-only C port of the
MIT-licensed `libvmx` decoder. Its wire transport and playback code were
independently ported from the listed MIT-licensed revisions -- first to Rust,
and from that translation to the C17 in `src/`; upstream source snapshots are
deliberately not vendored or built.

| Component | Upstream | Commit |
|---|---|---|
| `omtplayer` playback core | https://github.com/openmediatransport/omtplayer | `c47397aa74600f998d9fa8533a26b0dbbf9e26e9` |
| `libomtnet` | https://github.com/openmediatransport/libomtnet | `bda284e86ea56166b0caa45f30136def8c893e5e` |
| `libvmx` | https://github.com/openmediatransport/libvmx | `f73569e767b9d9177519bf5765c9434dfe8af51f` |

The translation preserves the VMX1 decoder behavior and SIMD algorithms from
the recorded upstream revision behind bounded, length-checked interfaces and a
fixed worker pool; the conformance vectors in `tests/vectors/vmx/` prove it
bit-exact with the reference decoder.

The implementation adds bounded frame parsing, conservative source and target
validation, direct DRM/ALSA output, lifecycle/status publication, and a
hardened command line.
