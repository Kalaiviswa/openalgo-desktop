//! The test binary's allocator: the system allocator, plus a cap a test can
//! put on the largest single allocation a future makes while it is polled
//! (PROD-TR-03: a split of 2147483647 in chunks of 1 asked for 16 GiB
//! before its 100-order limit was checked).
//!
//! Above the cap the allocation fails, so the process aborts with "memory
//! allocation of N bytes failed" before the memory is touched: a regression
//! fails the test instead of swapping the machine. Outside [`capped`] every
//! allocation goes straight to the system allocator, so the rest of the
//! binary is unaffected.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;

struct Capped;

thread_local! {
    /// The cap on this thread while a capped future is polled.
    static CAP: Cell<Option<usize>> = const { Cell::new(None) };
    /// The largest single request made under the cap.
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

/// Whether a request of `size` bytes may go ahead. Const-initialised
/// thread locals without a destructor never allocate, so this is safe to
/// call from inside the allocator.
fn admit(size: usize) -> bool {
    let Ok(Some(cap)) = CAP.try_with(Cell::get) else {
        return true;
    };
    let _ = LARGEST.try_with(|l| l.set(l.get().max(size)));
    size <= cap
}

// SAFETY: every call is forwarded unchanged to `System`, or refused with a
// null pointer, which `GlobalAlloc` allows.
unsafe impl GlobalAlloc for Capped {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if admit(layout.size()) {
            unsafe { System.alloc(layout) }
        } else {
            std::ptr::null_mut()
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if admit(layout.size()) {
            unsafe { System.alloc_zeroed(layout) }
        } else {
            std::ptr::null_mut()
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if admit(new_size) {
            unsafe { System.realloc(ptr, layout, new_size) }
        } else {
            std::ptr::null_mut()
        }
    }
}

#[global_allocator]
static ALLOCATOR: Capped = Capped;

/// Run `fut` with every single allocation it makes while it is polled held
/// to `cap` bytes. Returns its output and the largest single request it
/// made. Work it hands to another thread (the blocking pool) is not capped.
pub async fn capped<F: Future>(cap: usize, fut: F) -> (F::Output, usize) {
    let mut fut = std::pin::pin!(fut);
    let mut largest = 0;
    let out = std::future::poll_fn(|cx| {
        let before = CAP.with(|c| c.replace(Some(cap)));
        LARGEST.with(|l| l.set(0));
        let polled = fut.as_mut().poll(cx);
        largest = largest.max(LARGEST.with(Cell::get));
        CAP.with(|c| c.set(before));
        polled
    })
    .await;
    (out, largest)
}

/// [`capped`] for plain code.
pub fn capped_sync<T>(cap: usize, f: impl FnOnce() -> T) -> (T, usize) {
    let before = CAP.with(|c| c.replace(Some(cap)));
    LARGEST.with(|l| l.set(0));
    let out = f();
    let largest = LARGEST.with(Cell::get);
    CAP.with(|c| c.set(before));
    (out, largest)
}
