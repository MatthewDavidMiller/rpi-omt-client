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

enum Message {
    Work(Box<Job>),
    Shutdown,
}

/// Bounded pool of decode workers with explicit stacks.
pub struct WorkerPool {
    workers: Vec<WorkerHandle>,
}

struct WorkerHandle {
    jobs: SyncSender<Message>,
    done: Receiver<bool>,
    thread: JoinHandle<()>,
}

impl WorkerPool {
    /// Spawns `count` parked workers. `count` must be at least one.
    pub fn new(count: usize) -> Result<Self, DecodeError> {
        if count == 0 {
            return Err(DecodeError::InvalidDimensions);
        }
        let mut workers = Vec::new();
        workers
            .try_reserve_exact(count)
            .map_err(|_| DecodeError::WorkerFailure)?;
        for index in 0..count {
            let (job_tx, job_rx) = mpsc::sync_channel::<Message>(1);
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

        let mut active = 0_usize;
        for index in 0..worker_count {
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
            let Some(worker) = self.workers.get(index) else {
                return Err(DecodeError::WorkerFailure);
            };
            // SAFETY: slice/output ranges for distinct `index` values are
            // disjoint partitions of the caller-provided buffers. The main
            // thread does not touch them again until every `done` arrives.
            let job = Job {
                slices: slice_ptr,
                slice_offset,
                slice_count,
                output: unsafe { output_ptr.add(out_offset) },
                output_len: out_count,
                geometry,
                matrix: *matrix,
                coefficients,
            };
            worker
                .jobs
                .send(Message::Work(Box::new(job)))
                .map_err(|_| DecodeError::WorkerFailure)?;
            active += 1;
        }

        let mut ok = true;
        for worker in self.workers.iter().take(active) {
            match worker.done.recv() {
                Ok(true) => {}
                Ok(false) => ok = false,
                Err(_) => return Err(DecodeError::WorkerFailure),
            }
        }
        Ok(ok)
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        for worker in &self.workers {
            let _ = worker.jobs.send(Message::Shutdown);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.thread.join();
        }
    }
}

fn worker_loop(jobs: Receiver<Message>, done: SyncSender<bool>) {
    while let Ok(message) = jobs.recv() {
        match message {
            Message::Shutdown => break,
            Message::Work(job) => {
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
