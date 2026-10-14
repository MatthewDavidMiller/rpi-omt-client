//! Opt-in per-frame latency trace. When `OMT_LATENCY_TRACE` names a file,
//! every presented video frame appends one CSV row of `CLOCK_MONOTONIC`
//! instants: the fixed header read, the last payload byte read, decode start
//! and end, the flip queued, and the vblank that put it on screen. The
//! sender's own timestamp leads the row so arrival can be held against the
//! sender's cadence. Audio frames go to the same path with `.audio` added:
//! the sender timestamp, the last byte read, the write into the ring, the
//! frames then queued ahead of the speaker, the sample rate, and the
//! underruns so far. Unset, nothing here runs.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};

use crate::channel::Frame;
use crate::video::Stamps;

pub const TRACE_ENV: &str = "OMT_LATENCY_TRACE";
/// Rows buffered between writes, so the trace adds no per-frame syscall.
const FLUSH_ROWS: u32 = 64;

/// Opens the trace file at the configured path plus `suffix`.
fn open(suffix: &str) -> Option<BufWriter<File>> {
    let mut path = std::env::var_os(TRACE_ENV).filter(|p| !p.is_empty())?;
    path.push(suffix);
    let file = OpenOptions::new().create(true).append(true).open(path).ok()?;
    Some(BufWriter::new(file))
}

#[derive(Debug)]
pub struct AudioTrace {
    out: BufWriter<File>,
    rows: u32,
}

impl AudioTrace {
    pub fn from_env() -> Option<Self> {
        Some(Self { out: open(".audio")?, rows: 0 })
    }

    pub fn written(&mut self, frame: &Frame, queued: u64, rate: i32, underruns: u64) {
        let _ = writeln!(
            self.out,
            "{},{},{},{},{},{}",
            frame.header.map_or(0, |h| h.timestamp),
            frame.complete_ns,
            omt_sys::posix::monotonic_ns(),
            queued,
            rate,
            underruns
        );
        self.rows += 1;
        if self.rows.is_multiple_of(FLUSH_ROWS) {
            let _ = self.out.flush();
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Row {
    timestamp: i64,
    header: u64,
    complete: u64,
    decode_start: u64,
    decoded: u64,
    queued: u64,
    dropped: u64,
}

#[derive(Debug)]
pub struct LatencyTrace {
    out: BufWriter<File>,
    pending: Option<Row>,
    rows: u32,
}

impl LatencyTrace {
    pub fn from_env() -> Option<Self> {
        Some(Self { out: open("")?, pending: None, rows: 0 })
    }

    /// Records the frame just presented. Its flip is still in flight, so the
    /// row is completed with the vblank the next presentation retires.
    pub fn presented(&mut self, frame: &Frame, stamps: &Stamps, dropped: u64) {
        self.complete(stamps);
        self.pending = Some(Row {
            timestamp: frame.header.map_or(0, |h| h.timestamp),
            header: frame.header_ns,
            complete: frame.complete_ns,
            decode_start: stamps.decode_start,
            decoded: stamps.decoded,
            queued: stamps.queued,
            dropped,
        });
    }

    fn complete(&mut self, stamps: &Stamps) {
        let Some(row) = self.pending.take() else { return };
        // A vblank from before this row's flip was queued is not its own.
        let vblank = if stamps.vblank >= row.queued { stamps.vblank } else { 0 };
        let _ = writeln!(
            self.out,
            "{},{},{},{},{},{},{},{}",
            row.timestamp,
            row.header,
            row.complete,
            row.decode_start,
            row.decoded,
            row.queued,
            vblank,
            row.dropped
        );
        self.rows += 1;
        if self.rows.is_multiple_of(FLUSH_ROWS) {
            let _ = self.out.flush();
        }
    }
}
