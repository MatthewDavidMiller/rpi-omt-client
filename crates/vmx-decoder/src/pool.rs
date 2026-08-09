// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Persistent decode workers. Spawning scoped threads per frame costs roughly
// three creations per frame (~180/sec at 60 fps). These threads live for the
// decoder's lifetime and take one job per frame through bounded channels.
//
// Unsafe is confined to constructing `Send` pointers for one frame's disjoint
// slice and output regions. The main thread does not mutate those regions
// until every worker has reported completion, and `Drop` joins the workers
// before the decoder frees its slices.
#![allow(unsafe_code)]

use crate::tables::SLICE_HEIGHT;
use crate::{DecodeError, DecodeGeometry, Slice, decode_group};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

/// One worker's share of a frame: disjoint slices and their output region.
struct Job {
    slices: *mut Slice,
    slice_offset: usize,
    slice_count: usize,
    output: *mut u8,
    output_len: usize,
    geometry: DecodeGeometry,
    matrix: [u16; 64],
    coefficients: &'static [i16; 5],
}

// SAFETY: a Job is only sent while the main thread uniquely borrows the
// decoder for this frame and waits for every reply before those pointers can
// be reused or dropped. Each worker receives a disjoint slice/output range.
unsafe impl Send for Job {}

/// Bounded pool of decode workers with explicit stacks.
pub struct WorkerPool {
    workers: Vec<WorkerHandle>,
}

struct WorkerHandle {
    // `Some` carries one inline job; `None` shuts the worker down. Keeping the
    // job in the bounded channel avoids one heap allocation per worker per
    // decoded frame.
    jobs: SyncSender<Option<Job>>,
    done: Receiver<bool>,
    thread: JoinHandle<()>,
}

impl WorkerPool {
    /// Spawns `count` parked workers. `count` must be at least one.
    pub fn new(count: usize) -> Result<Self, DecodeError> {
        if count == 0 || count > crate::MAX_WORKERS {
            return Err(DecodeError::InvalidDimensions);
        }
        let mut workers = Vec::new();
        workers
            .try_reserve_exact(count)
            .map_err(|_| DecodeError::WorkerFailure)?;
        for index in 0..count {
            let (job_tx, job_rx) = mpsc::sync_channel::<Option<Job>>(1);
            let (done_tx, done_rx) = mpsc::sync_channel::<bool>(1);
            let thread = thread::Builder::new()
                .name(format!("vmx-decode-{index}"))
                .stack_size(crate::WORKER_STACK_SIZE)
                .spawn(move || worker_loop(job_rx, done_tx))
                .map_err(|_| DecodeError::WorkerFailure)?;
            workers.push(WorkerHandle {
                jobs: job_tx,
                done: done_rx,
                thread,
            });
        }
        Ok(Self { workers })
    }

    /// Decodes `slices` into `output` using the pool, returning whether every
    /// worker reported success.
    pub fn decode(
        &self,
        slices: &mut [Slice],
        output: &mut [u8],
        geometry: DecodeGeometry,
        matrix: &[u16; 64],
        coefficients: &'static [i16; 5],
    ) -> Result<bool, DecodeError> {
        let worker_count = self.workers.len().min(slices.len().max(1));
        if worker_count == 0 {
            return Ok(true);
        }
        let group = slices.len().div_ceil(worker_count);
        let rows_per_group = group * SLICE_HEIGHT;
        let stride = geometry.stride;
        let slice_len = slices.len();
        let output_len = output.len();
        let slice_ptr = slices.as_mut_ptr();
        let output_ptr = output.as_mut_ptr();

        // Validate and construct the whole disjoint partition before any raw
        // pointer crosses a thread boundary. After dispatch begins, the only
        // possible error is a closed worker channel, whose cleanup below can
        // drain the exact number of jobs already sent.
        let mut jobs: [Option<Job>; crate::MAX_WORKERS] = std::array::from_fn(|_| None);
        let mut job_count = 0_usize;
        for (index, job_slot) in jobs.iter_mut().enumerate().take(worker_count) {
            let slice_offset = index.checked_mul(group).ok_or(DecodeError::WorkerFailure)?;
            if slice_offset >= slice_len {
                break;
            }
            let slice_count = (slice_len - slice_offset).min(group);
            let out_offset = index
                .checked_mul(rows_per_group)
                .and_then(|rows| rows.checked_mul(stride))
                .ok_or(DecodeError::WorkerFailure)?;
            if out_offset >= output_len {
                return Err(DecodeError::OutputSize);
            }
            let out_count = (output_len - out_offset).min(rows_per_group * stride);
            // SAFETY: slice/output ranges for distinct `index` values are
            // disjoint partitions of the caller-provided buffers. The main
            // thread does not touch them again until every `done` arrives.
            *job_slot = Some(Job {
                slices: slice_ptr,
                slice_offset,
                slice_count,
                output: unsafe { output_ptr.add(out_offset) },
                output_len: out_count,
                geometry,
                matrix: *matrix,
                coefficients,
            });
            job_count += 1;
        }

        let mut active = 0_usize;
        for (index, job) in jobs.into_iter().take(job_count).enumerate() {
            let Some(worker) = self.workers.get(index) else {
                let _ = self.finish(active);
                return Err(DecodeError::WorkerFailure);
            };
            let Some(job) = job else {
                let _ = self.finish(active);
                return Err(DecodeError::WorkerFailure);
            };
            if worker.jobs.send(Some(job)).is_err() {
                // Jobs sent earlier in this dispatch still borrow the
                // caller's buffers through raw pointers. Wait for every one
                // before returning the channel failure, or the caller could
                // reuse or drop those buffers while a worker is writing them.
                let _ = self.finish(active);
                return Err(DecodeError::WorkerFailure);
            }
            active += 1;
        }

        self.finish(active)
    }

    /// Receives every active worker's completion even after one fails. A
    /// short-circuit here would leave later workers holding the frame's raw
    /// pointers after `decode` returned.
    fn finish(&self, active: usize) -> Result<bool, DecodeError> {
        let mut ok = true;
        let mut worker_failed = false;
        for worker in self.workers.iter().take(active) {
            match worker.done.recv() {
                Ok(true) => {}
                Ok(false) => ok = false,
                Err(_) => worker_failed = true,
            }
        }
        if worker_failed {
            Err(DecodeError::WorkerFailure)
        } else {
            Ok(ok)
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        for worker in &self.workers {
            let _ = worker.jobs.send(None);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.thread.join();
        }
    }
}

fn worker_loop(jobs: Receiver<Option<Job>>, done: SyncSender<bool>) {
    while let Ok(message) = jobs.recv() {
        match message {
            None => break,
            Some(job) => {
                // SAFETY: the main thread constructed disjoint slice/output
                // ranges for this job and waits for `done` before touching
                // them again or dropping the decoder.
                let ok = unsafe {
                    let slices = std::slice::from_raw_parts_mut(
                        job.slices.add(job.slice_offset),
                        job.slice_count,
                    );
                    let output = std::slice::from_raw_parts_mut(job.output, job.output_len);
                    decode_group(slices, output, job.geometry, &job.matrix, job.coefficients)
                };
                if done.send(ok).is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorSpace, Decoder, Dimensions};
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    #[test]
    fn a_mid_dispatch_failure_drains_started_workers() {
        let mut decoder = Decoder::new(
            Dimensions {
                width: 320,
                height: 176,
            },
            ColorSpace::Bt709,
            2,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        decoder
            .load(include_bytes!(
                "../../../tests/vectors/vmx/edges-320x176-709.vmx"
            ))
            .unwrap_or_else(|error| panic!("{error}"));

        // Stop the second worker so dispatch succeeds for worker zero and
        // fails for worker one. The first completion must be consumed before
        // the error is returned to the decoder.
        decoder.pool.workers[1]
            .jobs
            .send(None)
            .unwrap_or_else(|_| panic!("worker shutdown channel closed"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while !decoder.pool.workers[1].thread.is_finished() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            decoder.pool.workers[1].thread.is_finished(),
            "worker did not stop"
        );

        let mut output = vec![0_u8; 320 * 176 * 4];
        assert_eq!(
            decoder.decode_bgrx(&mut output, 320 * 4),
            Err(DecodeError::WorkerFailure)
        );
        assert_eq!(
            decoder.pool.workers[0]
                .done
                .recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Timeout),
            "the started worker's completion was left queued"
        );
    }
}
