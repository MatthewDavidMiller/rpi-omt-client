//! The VMX decoder: the envelope and every output path, from one input whose
//! first byte picks the geometry and the worker count.

#![no_main]

use omt_vmx::{ColorSpace, Decoder, Placement, PlaneMut};

const SHAPES: [(usize, usize); 4] = [(16, 16), (64, 32), (128, 80), (320, 176)];

fn samples(destination: usize, source: usize) -> Vec<u32> {
    (0..destination)
        .map(|i| {
            u32::try_from((((2 * i + 1) * source) / (2 * destination)).min(source - 1)).unwrap_or(0)
        })
        .collect()
}

/// # Safety
///
/// Called by libFuzzer with `size` readable bytes at `data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn LLVMFuzzerTestOneInput(data: *const u8, size: usize) -> i32 {
    // SAFETY: libFuzzer's contract for this call.
    let input = unsafe { omt_fuzz::input(data, size) };
    let Some((&selector, stream)) = input.split_first() else { return 0 };
    let (width, height) = SHAPES[usize::from(selector % 4)];
    let workers = 1 + usize::from(selector >> 6) % 3;
    let Ok(mut d) = Decoder::new(width, height, ColorSpace::Bt709, workers) else { return 0 };
    let mut out = vec![0_u8; width * height * 4];
    if d.load(stream).is_err() {
        return 0;
    }
    let _ = d.decode_bgrx(&mut out, width * 4);
    if d.load(stream).is_ok() {
        let _ = d.decode_uyvy(&mut out, width * 2);
    }
    if d.load(stream).is_ok() {
        let enlarge = selector & 0x10 != 0;
        let (pw, ph) =
            if enlarge { (width * 3 / 2, height * 3 / 2) } else { (width / 2 + 1, height / 2 + 1) };
        let stride = (pw + 8) * 4;
        let mut placed = vec![0_u8; stride * (ph + 3)];
        let (columns, rows) = (samples(pw, width), samples(ph, height));
        let p = Placement { x: 4, y: 2, columns: &columns, rows: &rows };
        let _ = d.decode_bgrx_placed(&mut placed, stride, &p);
    }
    if d.load(stream).is_ok() {
        // The planar 4:2:2 output, in the one buffer it fits.
        let half = width / 2;
        let (y, rest) = out.split_at_mut(width * height);
        let (u, v) = rest.split_at_mut(half * height);
        let _ = d.decode_yuv422p([
            PlaneMut { data: y, stride: width },
            PlaneMut { data: u, stride: half },
            PlaneMut { data: &mut v[..half * height], stride: half },
        ]);
    }
    0
}
