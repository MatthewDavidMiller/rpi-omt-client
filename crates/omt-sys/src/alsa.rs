//! The ALSA PCM calls the receiver's audio output uses (alsa-lib).

use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::marker::{PhantomData, PhantomPinned};

macro_rules! opaque {
    ($($name:ident),* $(,)?) => {$(
        #[repr(C)]
        #[derive(Debug)]
        pub struct $name {
            _data: [u8; 0],
            _marker: PhantomData<(*mut u8, PhantomPinned)>,
        }
    )*};
}

opaque!(snd_pcm_t, snd_pcm_hw_params_t, snd_pcm_sw_params_t);

pub type snd_pcm_uframes_t = c_ulong;
pub type snd_pcm_sframes_t = c_long;

pub const SND_PCM_STREAM_PLAYBACK: c_int = 0;
pub const SND_PCM_NONBLOCK: c_int = 0x0001;
pub const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
pub const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

unsafe extern "C" {
    pub fn snd_pcm_open(
        pcm: *mut *mut snd_pcm_t,
        name: *const c_char,
        stream: c_int,
        mode: c_int,
    ) -> c_int;
    pub fn snd_pcm_close(pcm: *mut snd_pcm_t) -> c_int;
    pub fn snd_pcm_drop(pcm: *mut snd_pcm_t) -> c_int;
    pub fn snd_pcm_hw_params_malloc(ptr: *mut *mut snd_pcm_hw_params_t) -> c_int;
    pub fn snd_pcm_hw_params_free(obj: *mut snd_pcm_hw_params_t);
    pub fn snd_pcm_sw_params_malloc(ptr: *mut *mut snd_pcm_sw_params_t) -> c_int;
    pub fn snd_pcm_sw_params_free(obj: *mut snd_pcm_sw_params_t);
    pub fn snd_pcm_hw_params_any(pcm: *mut snd_pcm_t, params: *mut snd_pcm_hw_params_t) -> c_int;
    pub fn snd_pcm_hw_params_set_channels(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: c_uint,
    ) -> c_int;
    pub fn snd_pcm_hw_params_set_rate_near(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: *mut c_uint,
        dir: *mut c_int,
    ) -> c_int;
    pub fn snd_pcm_hw_params_set_format(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        format: c_int,
    ) -> c_int;
    pub fn snd_pcm_hw_params_set_access(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        access: c_int,
    ) -> c_int;
    pub fn snd_pcm_hw_params_set_buffer_time_near(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: *mut c_uint,
        dir: *mut c_int,
    ) -> c_int;
    pub fn snd_pcm_hw_params_set_period_time_near(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: *mut c_uint,
        dir: *mut c_int,
    ) -> c_int;
    pub fn snd_pcm_hw_params(pcm: *mut snd_pcm_t, params: *mut snd_pcm_hw_params_t) -> c_int;
    pub fn snd_pcm_hw_params_get_buffer_size(
        params: *const snd_pcm_hw_params_t,
        val: *mut snd_pcm_uframes_t,
    ) -> c_int;
    pub fn snd_pcm_hw_params_get_period_size(
        params: *const snd_pcm_hw_params_t,
        frames: *mut snd_pcm_uframes_t,
        dir: *mut c_int,
    ) -> c_int;
    pub fn snd_pcm_sw_params_current(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_sw_params_t,
    ) -> c_int;
    pub fn snd_pcm_sw_params_set_avail_min(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_sw_params_t,
        val: snd_pcm_uframes_t,
    ) -> c_int;
    pub fn snd_pcm_sw_params_set_start_threshold(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_sw_params_t,
        val: snd_pcm_uframes_t,
    ) -> c_int;
    pub fn snd_pcm_sw_params_set_stop_threshold(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_sw_params_t,
        val: snd_pcm_uframes_t,
    ) -> c_int;
    pub fn snd_pcm_sw_params(pcm: *mut snd_pcm_t, params: *mut snd_pcm_sw_params_t) -> c_int;
    pub fn snd_pcm_avail_update(pcm: *mut snd_pcm_t) -> snd_pcm_sframes_t;
    pub fn snd_pcm_writei(
        pcm: *mut snd_pcm_t,
        buffer: *const c_void,
        size: snd_pcm_uframes_t,
    ) -> snd_pcm_sframes_t;
    pub fn snd_pcm_wait(pcm: *mut snd_pcm_t, timeout: c_int) -> c_int;
    pub fn snd_pcm_recover(pcm: *mut snd_pcm_t, err: c_int, silent: c_int) -> c_int;
    pub fn snd_strerror(errnum: c_int) -> *const c_char;
}

/// alsa-lib's description of an error code.
pub fn strerror(code: c_int) -> String {
    // SAFETY: snd_strerror returns a static, NUL-terminated string for any
    // code.
    let text = unsafe { snd_strerror(code) };
    if text.is_null() {
        return format!("error {code}");
    }
    // SAFETY: non-null and NUL-terminated, as above.
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

/// Why a configuration step failed: the alsa-lib function and its code.
pub type StepError = (&'static str, c_int);

/// An open playback PCM, dropped and closed on drop.
#[derive(Debug)]
pub struct Pcm(*mut snd_pcm_t);

// SAFETY: the handle is owned exclusively; alsa-lib allows using a PCM from
// one thread at a time, which `&mut self` enforces.
unsafe impl Send for Pcm {}

struct HwParams(*mut snd_pcm_hw_params_t);

impl Drop for HwParams {
    fn drop(&mut self) {
        // SAFETY: allocated by snd_pcm_hw_params_malloc, freed once.
        unsafe { snd_pcm_hw_params_free(self.0) }
    }
}

struct SwParams(*mut snd_pcm_sw_params_t);

impl Drop for SwParams {
    fn drop(&mut self) {
        // SAFETY: allocated by snd_pcm_sw_params_malloc, freed once.
        unsafe { snd_pcm_sw_params_free(self.0) }
    }
}

fn check(step: &'static str, rc: c_int) -> Result<(), StepError> {
    if rc < 0 { Err((step, rc)) } else { Ok(()) }
}

impl Pcm {
    /// Opens `device` for non-blocking playback.
    pub fn open(device: &str) -> Result<Self, c_int> {
        let name = std::ffi::CString::new(device).map_err(|_| -22)?;
        let mut pcm = std::ptr::null_mut();
        // SAFETY: `pcm` is a valid out-pointer and `name` NUL-terminated.
        let rc = unsafe {
            snd_pcm_open(&raw mut pcm, name.as_ptr(), SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK)
        };
        if rc < 0 { Err(rc) } else { Ok(Self(pcm)) }
    }

    /// Hardware parameters: interleaved float at `rate` (nearest) with
    /// `channels`, and the ring and period times asked for. Returns the
    /// buffer and period sizes in frames the device chose.
    pub fn configure_hw(
        &mut self,
        channels: u32,
        rate: u32,
        buffer_us: u32,
        period_us: u32,
    ) -> Result<(snd_pcm_uframes_t, snd_pcm_uframes_t), StepError> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: `raw` is a valid out-pointer.
        check("snd_pcm_hw_params_malloc", unsafe { snd_pcm_hw_params_malloc(&raw mut raw) })?;
        let hw = HwParams(raw);
        let (pcm, p) = (self.0, hw.0);
        let (mut urate, mut buffer, mut period, mut dir) = (rate, buffer_us, period_us, 0);
        // SAFETY: `pcm` is an open PCM, `p` a live hw_params, and every
        // out-pointer in the calls below is a live local.
        check("snd_pcm_hw_params_any", unsafe { snd_pcm_hw_params_any(pcm, p) })?;
        // SAFETY: as above.
        check("snd_pcm_hw_params_set_channels", unsafe {
            snd_pcm_hw_params_set_channels(pcm, p, channels)
        })?;
        // SAFETY: as above.
        check("snd_pcm_hw_params_set_rate_near", unsafe {
            snd_pcm_hw_params_set_rate_near(pcm, p, &raw mut urate, &raw mut dir)
        })?;
        // SAFETY: as above.
        check("snd_pcm_hw_params_set_format", unsafe {
            snd_pcm_hw_params_set_format(pcm, p, SND_PCM_FORMAT_FLOAT_LE)
        })?;
        // SAFETY: as above.
        check("snd_pcm_hw_params_set_access", unsafe {
            snd_pcm_hw_params_set_access(pcm, p, SND_PCM_ACCESS_RW_INTERLEAVED)
        })?;
        dir = 0;
        // SAFETY: as above.
        check("snd_pcm_hw_params_set_buffer_time_near", unsafe {
            snd_pcm_hw_params_set_buffer_time_near(pcm, p, &raw mut buffer, &raw mut dir)
        })?;
        dir = 0;
        // SAFETY: as above.
        check("snd_pcm_hw_params_set_period_time_near", unsafe {
            snd_pcm_hw_params_set_period_time_near(pcm, p, &raw mut period, &raw mut dir)
        })?;
        // SAFETY: as above.
        check("snd_pcm_hw_params", unsafe { snd_pcm_hw_params(pcm, p) })?;
        let (mut buffer_frames, mut period_frames) = (0, 0);
        // SAFETY: as above.
        check("snd_pcm_hw_params_get_buffer_size", unsafe {
            snd_pcm_hw_params_get_buffer_size(p, &raw mut buffer_frames)
        })?;
        // SAFETY: as above.
        check("snd_pcm_hw_params_get_period_size", unsafe {
            snd_pcm_hw_params_get_period_size(p, &raw mut period_frames, &raw mut dir)
        })?;
        Ok((buffer_frames, period_frames))
    }

    /// Software parameters: wake-up, start, and stop thresholds in frames.
    pub fn configure_sw(
        &mut self,
        avail_min: snd_pcm_uframes_t,
        start: snd_pcm_uframes_t,
        stop: snd_pcm_uframes_t,
    ) -> Result<(), StepError> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: `raw` is a valid out-pointer.
        check("snd_pcm_sw_params_malloc", unsafe { snd_pcm_sw_params_malloc(&raw mut raw) })?;
        let sw = SwParams(raw);
        let (pcm, p) = (self.0, sw.0);
        // SAFETY: `pcm` is an open PCM and `p` a live sw_params, here and
        // in the calls below.
        check("snd_pcm_sw_params_current", unsafe { snd_pcm_sw_params_current(pcm, p) })?;
        // SAFETY: as above.
        check("snd_pcm_sw_params_set_avail_min", unsafe {
            snd_pcm_sw_params_set_avail_min(pcm, p, avail_min)
        })?;
        // SAFETY: as above.
        check("snd_pcm_sw_params_set_start_threshold", unsafe {
            snd_pcm_sw_params_set_start_threshold(pcm, p, start)
        })?;
        // SAFETY: as above.
        check("snd_pcm_sw_params_set_stop_threshold", unsafe {
            snd_pcm_sw_params_set_stop_threshold(pcm, p, stop)
        })?;
        // SAFETY: as above.
        check("snd_pcm_sw_params", unsafe { snd_pcm_sw_params(pcm, p) })
    }

    pub fn avail_update(&mut self) -> snd_pcm_sframes_t {
        // SAFETY: the PCM is open.
        unsafe { snd_pcm_avail_update(self.0) }
    }

    /// Writes whole frames of `channels` interleaved samples from `samples`.
    pub fn writei(&mut self, samples: &[f32], channels: usize) -> snd_pcm_sframes_t {
        if channels == 0 {
            return 0;
        }
        let frames = (samples.len() / channels) as snd_pcm_uframes_t;
        // SAFETY: `samples` holds at least frames * channels floats, which
        // is what the PCM's configured format reads.
        unsafe { snd_pcm_writei(self.0, samples.as_ptr().cast(), frames) }
    }

    pub fn wait(&mut self, timeout_ms: c_int) -> c_int {
        // SAFETY: the PCM is open.
        unsafe { snd_pcm_wait(self.0, timeout_ms) }
    }

    pub fn recover(&mut self, err: c_int) -> c_int {
        // SAFETY: the PCM is open.
        unsafe { snd_pcm_recover(self.0, err, 1) }
    }
}

impl Drop for Pcm {
    fn drop(&mut self) {
        // SAFETY: the PCM is open and closed exactly once here.
        unsafe {
            snd_pcm_drop(self.0);
        }
        // SAFETY: as above.
        unsafe {
            snd_pcm_close(self.0);
        }
    }
}
