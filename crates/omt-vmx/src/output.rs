//! The decoder's one use of shared mutable memory: worker threads writing
//! disjoint rows of the caller's output buffer.
//!
//! A decode borrows the output mutably for its whole duration and does not
//! return until every worker has reported the frame finished, so the buffer
//! outlives every pointer made here. What the borrow checker cannot see is
//! that the threads never write the same bytes: each slice index is claimed
//! by exactly one thread per frame, and every row a slice writes is derived
//! from its index -- source rows `16*i..16*i+16`, or for a placed decode the
//! destination rows assigned to slice `i` alone. The callers state that per
//! write; this module only checks bounds.

use std::marker::PhantomData;

/// A raw view of an output buffer, shared by the pool for one frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Out {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: an Out is only dereferenced through `row`, whose callers guarantee
// that no two threads touch the same bytes and that the buffer outlives the
// frame (see the module documentation).
unsafe impl Send for Out {}
// SAFETY: as above.
unsafe impl Sync for Out {}

impl Out {
    pub(crate) const EMPTY: Self = Self { ptr: std::ptr::null_mut(), len: 0 };

    pub(crate) fn new(buffer: &mut [u8]) -> Self {
        Self { ptr: buffer.as_mut_ptr(), len: buffer.len() }
    }

    /// `len` bytes at `offset`, or `None` when they are not all inside the
    /// buffer.
    ///
    /// # Safety
    ///
    /// The buffer must still be borrowed by the decode that made this view,
    /// and no other thread may access these bytes until that decode returns.
    pub(crate) unsafe fn row<'a>(&self, offset: usize, len: usize) -> Option<&'a mut [u8]> {
        if self.ptr.is_null() || offset > self.len || self.len - offset < len {
            return None;
        }
        let start = self.ptr.wrapping_add(offset);
        // SAFETY: the range is inside the buffer (checked above); exclusive
        // access for the frame is the caller's guarantee.
        Some(unsafe { std::slice::from_raw_parts_mut(start, len) })
    }
}

/// A placement's tables, shared read-only by the pool for one frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlacementRef {
    pub(crate) x: usize,
    pub(crate) y: usize,
    columns: (*const u32, usize),
    rows: (*const u32, usize),
    _borrow: PhantomData<&'static [u32]>,
}

// SAFETY: the tables are only read, and only while the decode that borrowed
// them is running.
unsafe impl Send for PlacementRef {}
// SAFETY: as above.
unsafe impl Sync for PlacementRef {}

impl PlacementRef {
    pub(crate) fn new(p: &crate::Placement<'_>) -> Self {
        Self {
            x: p.x,
            y: p.y,
            columns: (p.columns.as_ptr(), p.columns.len()),
            rows: (p.rows.as_ptr(), p.rows.len()),
            _borrow: PhantomData,
        }
    }

    /// The column and row tables.
    ///
    /// # Safety
    ///
    /// The decode that made this view must still be running, so the
    /// placement it borrowed is alive.
    pub(crate) unsafe fn tables<'a>(&self) -> (&'a [u32], &'a [u32]) {
        // SAFETY: the pointer came from a live slice of this length, which
        // the caller guarantees is still borrowed.
        let columns = unsafe { std::slice::from_raw_parts(self.columns.0, self.columns.1) };
        // SAFETY: as above.
        let rows = unsafe { std::slice::from_raw_parts(self.rows.0, self.rows.1) };
        (columns, rows)
    }
}
