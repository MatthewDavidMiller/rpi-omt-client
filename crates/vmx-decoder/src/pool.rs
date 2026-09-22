// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Persistent decode workers. Spawning scoped threads per frame costs roughly
// three creations per frame (~180/sec at 60 fps). These threads live for the
// decoder's lifetime and take one job per frame through bounded channels.
//
// Slices are claimed one at a time from a shared counter rather than handed
// out as fixed contiguous bands. Slice cost follows picture detail, so with
// fixed bands every frame waited for whichever worker drew the busiest part
// of the picture while the others sat idle; claiming lets a worker that
// finishes early take the next slice instead.
//
// Unsafe is confined to constructing `Send` pointers for one frame's slices
// and output. Each slice index is claimed exactly once, so every worker holds
// disjoint slice and output ranges. The main thread does not mutate those
// regions until every worker has reported completion, and `Drop` joins the
// workers before the decoder frees its slices.
#![allow(unsafe_code)]

use crate::tables::SLICE_HEIGHT;
use crate::{DecodeError, DecodeGeometry, PlaneScratch, Slice, decode_group};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

/// One frame, shared by every worker: all of its slices and the whole output.
/// Which slices a worker decodes is decided by the claim counter.
struct Job {
    slices: *mut Slice,
    slice_count: usize,
    output: *mut u8,
    output_len: usize,
    geometry: DecodeGeometry,
    matrix: [u16; 64],
    coefficients: &'static [i16; 5],
}

// SAFETY: a Job is only sent while the main thread uniquely borrows the
// decoder for this frame and waits for every reply before those pointers can
// be reused or dropped. Workers touch only the slice indices they claim from
// the shared counter, and each index is claimed once.
unsafe impl Send for Job {}

/// Bounded pool of decode workers with explicit stacks.
pub struct WorkerPool {
    workers: Vec<WorkerHandle>,
    /// The next unclaimed slice of the frame being decoded.
    next: Arc<AtomicUsize>,
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
    /// Each worker owns one slice of YUV scratch, reused across the slices in
    /// its partition, so the pool's plane memory is `count` slices rather than
    /// one per slice of the frame.
    pub fn new(
        count: usize,
        luma_stride: usize,
        chroma_stride: usize,
    ) -> Result<Self, DecodeError> {
        if count == 0 || count > crate::MAX_WORKERS {
            return Err(DecodeError::InvalidDimensions);
        }
        let mut workers = Vec::new();
        workers
            .try_reserve_exact(count)
            .map_err(|_| DecodeError::WorkerFailure)?;
        let next = Arc::new(AtomicUsize::new(0));
        for index in 0..count {
            let claims = Arc::clone(&next);
            let (job_tx, job_rx) = mpsc::sync_channel::<Option<Job>>(1);
            let (done_tx, done_rx) = mpsc::sync_channel::<bool>(1);
            let scratch = PlaneScratch::new(luma_stride, chroma_stride)?;
            let thread = thread::Builder::new()
                .name(format!("vmx-decode-{index}"))
                .stack_size(crate::WORKER_STACK_SIZE)
                .spawn(move || worker_loop(&job_rx, &done_tx, &claims, scratch))
                .map_err(|_| DecodeError::WorkerFailure)?;
            workers.push(WorkerHandle {
                jobs: job_tx,
                done: done_rx,
                thread,
            });
        }
        Ok(Self { workers, next })
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
        let worker_count = self.workers.len().min(slices.len());
        if worker_count == 0 {
            return Ok(true);
        }
        let slice_len = slices.len();
        let output_len = output.len();
        // Every slice has to start inside the output before any pointer
        // crosses a thread boundary; a worker then clamps its own range.
        let last_offset = (slice_len - 1)
            .checked_mul(SLICE_HEIGHT)
            .and_then(|rows| rows.checked_mul(geometry.stride))
            .ok_or(DecodeError::WorkerFailure)?;
        if last_offset >= output_len {
            return Err(DecodeError::OutputSize);
        }
        let slice_ptr = slices.as_mut_ptr();
        let output_ptr = output.as_mut_ptr();
        // Workers are parked on their channels, so nothing reads the counter
        // until the sends below, which order this store before their claims.
        self.next.store(0, Ordering::Relaxed);

        let mut jobs: [Option<Job>; crate::MAX_WORKERS] = std::array::from_fn(|_| None);
        for job_slot in jobs.iter_mut().take(worker_count) {
            *job_slot = Some(Job {
                slices: slice_ptr,
                slice_count: slice_len,
                output: output_ptr,
                output_len,
                geometry,
                matrix: *matrix,
                coefficients,
            });
        }
        let job_count = worker_count;

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

fn worker_loop(
    jobs: &Receiver<Option<Job>>,
    done: &SyncSender<bool>,
    claims: &AtomicUsize,
    mut scratch: PlaneScratch,
) {
    while let Ok(message) = jobs.recv() {
        match message {
            None => break,
            Some(job) => {
                let ok = decode_claimed(&job, claims, &mut scratch);
                if done.send(ok).is_err() {
                    break;
                }
            }
        }
    }
}

/// Decodes slices claimed from `claims` until none are left.
///
/// A failed slice fails the frame, so it also ends the claiming for every
/// worker: there is no point decoding the rest of a frame that will be
/// skipped.
fn decode_claimed(job: &Job, claims: &AtomicUsize, scratch: &mut PlaneScratch) -> bool {
    let rows = SLICE_HEIGHT.saturating_mul(job.geometry.stride);
    loop {
        let index = claims.fetch_add(1, Ordering::Relaxed);
        if index >= job.slice_count {
            return true;
        }
        let Some(offset) = index.checked_mul(rows).filter(|&at| at < job.output_len) else {
            claims.store(job.slice_count, Ordering::Relaxed);
            return false;
        };
        let length = (job.output_len - offset).min(rows);
        // SAFETY: `index` was claimed by this worker alone and is below the
        // slice count, and `offset..offset + length` is inside the output and
        // belongs to that slice alone. The main thread waits for `done` before
        // touching either again or dropping the decoder.
        let ok = unsafe {
            let slice = std::slice::from_raw_parts_mut(job.slices.add(index), 1);
            let output = std::slice::from_raw_parts_mut(job.output.add(offset), length);
            decode_group(
                slice,
                output,
                job.geometry,
                &job.matrix,
                job.coefficients,
                scratch,
            )
        };
        if !ok {
            claims.store(job.slice_count, Ordering::Relaxed);
            return false;
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
