// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// The reference codec's Exp-Golomb bit reader. The C original reads through a
// fixed 0xFF-filled slice buffer, lets its bit counter go negative on damaged
// input, and relies on undefined shift behaviour to recover. This port keeps
// the reference behaviour for every well-formed stream and turns each of those
// undefined cases into an explicit `corrupt` flag instead.

/// Bytes of 0xFF padding kept past the largest accepted slice so the reader's
/// eight-byte lookahead always lands inside the allocation.
pub const PADDING: usize = 64;

pub struct BitReader {
    buffer: Vec<u8>,
    /// Exclusive end of the loaded payload plus its 0xFF padding. Reads past
    /// this are corrupt; bytes left over from a larger previous load are not
    /// visible to the window.
    length: usize,
    /// Maximum payload-plus-padding size this reader will grow to.
    max_total: usize,
    position: usize,
    bits_left: i32,
    window: u64,
    corrupt: bool,
}

impl BitReader {
    /// Creates a reader whose payload may grow up to `capacity` bytes.
    ///
    /// The padded backing store is reserved only when a frame is loaded, so a
    /// 1080p decoder does not touch megabytes of 0xFF per slice before the
    /// first packet arrives. Returns `None` if the maximum cannot be
    /// represented.
    pub fn with_capacity(capacity: usize) -> Option<Self> {
        let max_total = capacity.checked_add(PADDING)?;
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(PADDING).ok()?;
        buffer.resize(PADDING, 0xFF);
        Some(Self {
            buffer,
            length: PADDING,
            max_total,
            position: 0,
            bits_left: 64,
            window: 0,
            corrupt: false,
        })
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.max_total.saturating_sub(PADDING)
    }

    /// True once the stream drove the reader outside the behaviour the
    /// reference codec defines for well-formed input.
    #[must_use]
    pub fn corrupt(&self) -> bool {
        self.corrupt
    }

    /// Replaces the slice contents, restoring the 0xFF padding the reference
    /// codec depends on to terminate an over-read.
    pub fn load(&mut self, data: &[u8]) -> bool {
        if data.len() > self.capacity() {
            return false;
        }
        let needed = data.len() + PADDING;
        if needed > self.buffer.len() {
            let additional = needed - self.buffer.len();
            if self.buffer.try_reserve_exact(additional).is_err() {
                return false;
            }
            self.buffer.resize(needed, 0xFF);
        }
        self.buffer[..data.len()].copy_from_slice(data);
        self.buffer[data.len()..needed].fill(0xFF);
        self.length = needed;
        self.reset();
        true
    }

    /// `VMX_ResetData`.
    pub fn reset(&mut self) {
        self.position = 0;
        self.bits_left = 64;
        self.corrupt = false;
        self.window = self.window_at(0);
    }

    fn window_at(&mut self, position: usize) -> u64 {
        let end = position.saturating_add(8);
        if end > self.length {
            self.corrupt = true;
            return u64::MAX;
        }
        let Some(bytes) = self.buffer.get(position..end) else {
            self.corrupt = true;
            return u64::MAX;
        };
        let mut octets = [0_u8; 8];
        octets.copy_from_slice(bytes);
        u64::from_be_bytes(octets)
    }

    /// `FLUSHREADBITS`.
    fn flush(&mut self) {
        if self.bits_left == 0 {
            self.bits_left = 64;
            self.position = self.position.saturating_add(8);
            self.window = self.window_at(self.position);
        }
    }

    /// `RELOADBITS`.
    pub fn reload(&mut self) {
        if self.bits_left < 32 {
            if self.bits_left < 0 {
                self.corrupt = true;
                return;
            }
            let advance = usize::try_from((64 - self.bits_left) >> 3).unwrap_or(0);
            self.position = self.position.saturating_add(advance);
            self.window = self.window_at(self.position);
            self.bits_left += i32::try_from(advance << 3).unwrap_or(0);
        }
    }

    /// Reads the `count` bits that sit immediately above the new `bits_left`.
    fn take(&mut self, count: i32) -> u64 {
        self.bits_left -= count;
        if self.bits_left < 0 || !(0..=64).contains(&count) {
            self.corrupt = true;
            self.bits_left = self.bits_left.max(0);
            return 0;
        }
        let mask = if count >= 64 {
            u64::MAX
        } else {
            (1_u64 << count) - 1
        };
        (self.window >> self.bits_left) & mask
    }

    /// `GETBITB`: one bit without a reload.
    pub fn bit_bare(&mut self) -> u64 {
        self.take(1)
    }

    /// `GETBIT`: one bit, flushing an exhausted window.
    pub fn bit(&mut self) -> u64 {
        let value = self.take(1);
        self.flush();
        value
    }

    /// `GETZEROSB`: leading zeros inside the current window.
    pub fn zeros_bare(&mut self) -> i32 {
        let count = self.leading_zeros();
        if count > self.bits_left {
            // The reference codec lets the counter go negative here and
            // recovers through an undefined shift. Refuse the stream instead.
            self.corrupt = true;
            return 0;
        }
        self.bits_left -= count;
        count
    }

    /// `GETZEROS`: leading zeros, continuing into the next window if needed.
    pub fn zeros(&mut self) -> i32 {
        let mut count = self.leading_zeros();
        if count >= self.bits_left {
            count = self.bits_left;
            self.bits_left = 0;
            self.flush();
            let extra = i32::try_from(self.window.leading_zeros()).unwrap_or(64);
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

    fn leading_zeros(&mut self) -> i32 {
        if self.bits_left <= 0 || self.bits_left > 64 {
            self.corrupt = true;
            return 0;
        }
        let consumed = u32::try_from(64 - self.bits_left).unwrap_or(64);
        let shifted = self.window.checked_shl(consumed).unwrap_or(0);
        i32::try_from(shifted.leading_zeros()).unwrap_or(64)
    }

    /// `GETBITSB`: `count` bits without a reload.
    pub fn bits_bare(&mut self, count: i32) -> u64 {
        self.take(count)
    }

    /// `GETBITS`: `count` bits, flushing across window boundaries.
    pub fn bits(&mut self, count: i32) -> u64 {
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
                value = value
                    .checked_shl(u32::try_from(take).unwrap_or(0))
                    .unwrap_or(0);
            }
            value |= self.take(take);
            remaining -= take;
            self.flush();
        }
        value
    }

    /// `FLUSHREMAININGREADBITS`: realign to the next byte boundary.
    pub fn align(&mut self) {
        if self.bits_left < 64 {
            let remainder = self.bits_left & 7;
            let _discarded = self.bits(remainder);
        }
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(bytes: &[u8]) -> BitReader {
        let mut reader = BitReader::with_capacity(64).unwrap_or_else(|| panic!("allocation"));
        assert!(reader.load(bytes));
        reader
    }

    #[test]
    fn reads_bits_in_reference_order() {
        let mut r = reader(&[0b1011_0010, 0x00, 0x00, 0x00]);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bit(), 0);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bits(4), 0b0010);
        assert!(!r.corrupt());
    }

    #[test]
    fn counts_leading_zeros_like_the_reference() {
        let mut r = reader(&[0b0000_0100]);
        assert_eq!(r.zeros(), 5);
        assert_eq!(r.bit(), 1);
        assert!(!r.corrupt());
    }

    #[test]
    fn padding_terminates_an_exhausted_stream() {
        let mut r = reader(&[0x00]);
        for _ in 0..8 {
            let _consumed = r.bits(8);
            r.reload();
        }
        assert!(!r.corrupt());
    }

    #[test]
    fn refuses_to_run_past_the_allocation() {
        let mut r = reader(&[0x00]);
        for _ in 0..64 {
            let _consumed = r.bits(64);
            r.reload();
        }
        assert!(r.corrupt());
    }

    #[test]
    fn defers_the_maximum_allocation_until_load() {
        let reader = BitReader::with_capacity(1_000_000).unwrap_or_else(|| panic!("allocation"));
        assert!(reader.buffer.len() <= PADDING);
        assert_eq!(reader.capacity(), 1_000_000);
    }

    #[test]
    fn load_grows_only_to_the_payload_plus_padding() {
        let mut reader =
            BitReader::with_capacity(1_000_000).unwrap_or_else(|| panic!("allocation"));
        assert!(reader.load(&[0x80, 0x00, 0x00, 0x00]));
        assert_eq!(reader.buffer.len(), 4 + PADDING);
        assert_eq!(reader.length, 4 + PADDING);
    }

    #[test]
    fn a_shorter_load_cannot_read_leftover_bytes() {
        let mut reader = BitReader::with_capacity(1024).unwrap_or_else(|| panic!("allocation"));
        let mut long = vec![0xFF; 200];
        long[0] = 0x80;
        assert!(reader.load(&long));
        assert!(reader.load(&[0x80]));
        assert_eq!(reader.length, 1 + PADDING);
        for _ in 0..64 {
            let _consumed = reader.bits(64);
            reader.reload();
        }
        assert!(reader.corrupt());
    }
}
