// Appended ONLY to a disposable diagnostic binary by the manual workflow.
// Never linked into a release. Abort before a runaway allocation harms CI.
mod diagnostic_allocation_trace {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static CAPTURING: Cell<bool> = const { Cell::new(false) };
    }

    struct TracedSystem;

    fn observe(bytes: usize) {
        if bytes < 4 * 1024 * 1024 {
            return;
        }
        let entered = CAPTURING.try_with(|v| !v.replace(true)).unwrap_or(false);
        if !entered {
            return;
        }
        eprintln!(
            "ISOLATED_ALLOCATION bytes={bytes}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        if bytes >= 256 * 1024 * 1024 {
            std::process::exit(86);
        }
        let _ = CAPTURING.try_with(|v| v.set(false));
    }

    unsafe impl GlobalAlloc for TracedSystem {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            observe(layout.size());
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            observe(layout.size());
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, old: Layout, bytes: usize) -> *mut u8 {
            observe(bytes);
            unsafe { System.realloc(ptr, old, bytes) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: TracedSystem = TracedSystem;
}
