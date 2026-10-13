//! The reference codec's Exp-Golomb bit reader. The C original reads through
//! a fixed 0xFF-filled slice buffer, lets its bit counter go negative on
//! damaged input, and relies on undefined shift behaviour to recover. This
//! port keeps the reference behaviour for every well-formed stream and turns
//! each of those undefined cases into an explicit `corrupt` flag instead.
//!
//! Everything here is inlined: it is the innermost loop of the entropy
//! decode.

/// Bytes of 0xFF padding kept past the loaded payload so the reader's
/// eight-byte lookahead always lands inside the buffer.
pub(crate) const PADDING: usize = 64;

/// A slice's stored stream: the payload, its padding, and the largest
/// payload it accepts.
#[derive(Debug)]
pub(crate) struct Stream {
    buffer: Vec<u8>,
    /// Exclusive end of the loaded payload plus its padding. Bytes left from
    /// a larger earlier load are not visible to a reader.
    length: usize,
    capacity: usize,
}

impl Stream {
    /// An empty stream whose payload may grow up to `capacity` bytes. The
    /// padded store grows only on load, so a 1080p decoder does not touch
    /// megabytes per slice before the first frame arrives.
    pub(crate) fn new(capacity: usize) -> Self {
        Self { buffer: vec![0xFF; PADDING], length: PADDING, capacity }
    }

    /// Replaces the contents, restoring the 0xFF padding the reference
    /// codec depends on to terminate an over-read.
    pub(crate) fn load(&mut self, data: &[u8]) -> bool {
        if data.len() > self.capacity {
            return false;
        }
        let needed = data.len() + PADDING;
        if needed > self.buffer.len() {
            if self.buffer.try_reserve_exact(needed - self.buffer.len()).is_err() {
                return false;
            }
            self.buffer.resize(needed, 0);
        }
        self.buffer[..data.len()].copy_from_slice(data);
        self.buffer[data.len()..needed].fill(0xFF);
        self.length = needed;
        true
    }

    /// A reader positioned at the start (VMX_ResetData).
    pub(crate) fn reader(&self) -> Reader<'_> {
        let mut r = Reader {
            buf: &self.buffer[..self.length],
            position: 0,
            bits_left: 64,
            window: 0,
            corrupt: false,
        };
        r.window = r.window_at(0);
        r
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.length
    }

    #[cfg(test)]
    pub(crate) fn allocated(&self) -> usize {
        self.buffer.len()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pub(crate) position: usize,
    pub(crate) bits_left: i32,
    pub(crate) window: u64,
    pub(crate) corrupt: bool,
}

#[inline(always)]
fn shl64(v: u64, n: u32) -> u64 {
    if n >= 64 { 0 } else { v << n }
}

#[inline(always)]
fn clz64(v: u64) -> i32 {
    v.leading_zeros() as i32
}

impl Reader<'_> {
    #[inline(always)]
    fn window_at(&mut self, position: usize) -> u64 {
        if let Some(bytes) = self.buf.get(position..).and_then(|rest| rest.first_chunk::<8>()) {
            u64::from_be_bytes(*bytes)
        } else {
            self.corrupt = true;
            u64::MAX
        }
    }

    /// FLUSHREADBITS.
    #[inline(always)]
    pub(crate) fn flush(&mut self) {
        if self.bits_left == 0 {
            self.bits_left = 64;
            self.position += 8;
            self.window = self.window_at(self.position);
        }
    }

    /// RELOADBITS.
    #[inline(always)]
    pub(crate) fn reload(&mut self) {
        if self.bits_left < 32 {
            if self.bits_left < 0 {
                self.corrupt = true;
                return;
            }
            let advance = ((64 - self.bits_left) >> 3) as usize;
            self.position += advance;
            self.window = self.window_at(self.position);
            self.bits_left += (advance << 3) as i32;
        }
    }

    /// Reads the `count` bits immediately above the new `bits_left`.
    #[inline(always)]
    pub(crate) fn take(&mut self, count: i32) -> u64 {
        self.bits_left = self.bits_left.wrapping_sub(count);
        if self.bits_left < 0 || !(0..=64).contains(&count) {
            self.corrupt = true;
            if self.bits_left < 0 {
                self.bits_left = 0;
            }
            return 0;
        }
        let mask = if count >= 64 { u64::MAX } else { (1_u64 << count) - 1 };
        let shifted = if self.bits_left >= 64 { 0 } else { self.window >> self.bits_left };
        shifted & mask
    }

    /// GETBIT: one bit, flushing an exhausted window.
    #[inline(always)]
    pub(crate) fn bit(&mut self) -> u64 {
        let v = self.take(1);
        self.flush();
        v
    }

    #[inline(always)]
    fn leading_zeros(&mut self) -> i32 {
        if self.bits_left <= 0 || self.bits_left > 64 {
            self.corrupt = true;
            return 0;
        }
        clz64(shl64(self.window, (64 - self.bits_left) as u32))
    }

    /// GETZEROSB: leading zeros inside the current window.
    #[inline(always)]
    pub(crate) fn zeros_bare(&mut self) -> i32 {
        let count = self.leading_zeros();
        if count > self.bits_left {
            // The reference lets the counter go negative and recovers through
            // an undefined shift. Refuse the stream instead.
            self.corrupt = true;
            return 0;
        }
        self.bits_left -= count;
        count
    }

    /// GETZEROS: leading zeros, continuing into the next window if needed.
    #[inline(always)]
    pub(crate) fn zeros(&mut self) -> i32 {
        let mut count = self.leading_zeros();
        if count >= self.bits_left {
            count = self.bits_left;
            self.bits_left = 0;
            self.flush();
            let extra = clz64(self.window);
            if extra > self.bits_left {
                self.corrupt = true;
                return 0;
            }
            self.bits_left -= extra;
            count += extra;
        } else {
            self.bits_left -= count;
        }
        count
    }

    /// GETBITS: `count` bits, flushing across window boundaries.
    #[inline(always)]
    pub(crate) fn bits(&mut self, count: i32) -> u64 {
        if count < 0 {
            self.corrupt = true;
            return 0;
        }
        let mut remaining = count;
        let mut value = 0_u64;
        while remaining > 0 {
            let take = remaining.min(self.bits_left);
            if take <= 0 {
                self.corrupt = true;
                return value;
            }
            if value != 0 {
                value = shl64(value, take as u32);
            }
            value |= self.take(take);
            remaining -= take;
            self.flush();
        }
        value
    }

    /// FLUSHREMAININGREADBITS: realign to the next byte boundary.
    #[inline(always)]
    pub(crate) fn align(&mut self) {
        if self.bits_left < 64 {
            let remainder = self.bits_left & 7;
            let _ = self.bits(remainder);
        }
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(bytes: &[u8], capacity: usize) -> Stream {
        let mut s = Stream::new(capacity);
        assert!(s.load(bytes));
        s
    }

    #[test]
    fn bit_reader_matches_the_reference() {
        let s = stream(&[0xB2, 0, 0, 0], 64);
        let mut r = s.reader();
        assert_eq!([r.bit(), r.bit(), r.bit(), r.bit()], [1, 0, 1, 1]);
        assert_eq!(r.bits(4), 2);
        assert!(!r.corrupt);

        let s = stream(&[0x04], 64);
        let mut r = s.reader();
        assert_eq!(r.zeros(), 5);
        assert_eq!(r.bit(), 1);
        assert!(!r.corrupt);

        let s = stream(&[0], 64);
        let mut r = s.reader();
        for _ in 0..8 {
            let _ = r.bits(8);
            r.reload();
        }
        assert!(!r.corrupt);

        let mut r = s.reader();
        for _ in 0..64 {
            let _ = r.bits(64);
            r.reload();
        }
        assert!(r.corrupt);

        let mut big = Stream::new(1_000_000);
        assert!(big.allocated() <= PADDING);
        assert!(big.load(&[0x80, 0, 0, 0]));
        assert_eq!(big.len(), 4 + PADDING);

        let mut small = Stream::new(1024);
        let mut long = [0xFF_u8; 200];
        long[0] = 0x80;
        assert!(small.load(&long));
        assert!(small.load(&[0x80]));
        assert_eq!(small.len(), 1 + PADDING);
        let mut r = small.reader();
        for _ in 0..64 {
            let _ = r.bits(64);
            r.reload();
        }
        assert!(r.corrupt);
        assert!(!small.load(&[0; 1025]));
    }
}
