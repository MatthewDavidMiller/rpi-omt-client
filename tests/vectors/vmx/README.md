# VMX conformance vectors

These streams pin the Rust decoder in `crates/vmx-decoder` to the output of the
Open Media Transport VMX reference decoder, which this repository carried in C
until commit `f066284` and no longer ships.

`vectors.json` indexes each stream with its geometry, colour space, and the
SHA-256 of the reference decoder's UYVY and BGRX output. Only the compressed
streams are committed; the expected images are pinned by digest so the fixtures
stay small while still asserting bit exactness.

## Coverage

The set spans the four content classes that exercise different decoder paths —
smooth gradients, a flat field (the DC-broadcast path), high-frequency noise
(long Golomb codes), and hard edges — across both colour spaces, the 16-pixel
minimum, heights that do and do not align to the 16-row slice grid, and the
appliance's real 1920x1080 geometry. It also covers both stream envelopes: the
extended form that carries a DC shift, which the encoder emits below 720 lines,
and the plain progressive form it emits at 1080.

## Regenerating

The vectors were produced by building the C reference encoder and decoder from
`third_party/omt/libvmx` at commit `f066284`, forcing the 128-bit SSE2 path
(the reference codec's AVX2 dispatch is a bit-exact alternative, not a
different result), encoding each synthetic image at `VMX_PROFILE_OMT_SQ`, and
capturing `VMX_SaveTo`, `VMX_DecodeUYVY`, and `VMX_DecodeBGRX`.

Regenerating requires checking that tree back out of Git history. The vectors
are frozen deliberately: they are the record of the C implementation's
behaviour, so they should change only if the wire format itself changes.
