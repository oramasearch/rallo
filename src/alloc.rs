use std::{
    alloc::{GlobalAlloc, Layout},
    collections::VecDeque,
    ffi::c_void,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use crate::{
    stats::{Allocation, FrameInfo, Stats},
    unsafe_cell::RalloUnsafeCell,
};

#[derive(Default, Clone, Copy)]
pub struct FrameWrapper {
    pub ip: Option<usize>,
}
impl FrameWrapper {
    pub const fn new() -> Self {
        FrameWrapper { ip: None }
    }
}

type LogType<const MAX_FRAME_LENGTH: usize> = (usize, usize, [FrameWrapper; MAX_FRAME_LENGTH]);
type LogsType<const MAX_FRAME_LENGTH: usize, const MAX_LOG_COUNT: usize> =
    [RalloUnsafeCell<LogType<MAX_FRAME_LENGTH>>; MAX_LOG_COUNT];

/// A custom allocator that tracks memory allocations and deallocations.
/// ```rust
/// use rallo::RalloAllocator;
///
/// const MAX_FRAME_LENGTH: usize = 128;
/// const MAX_LOG_COUNT: usize = 1_024 * 10;
/// #[global_allocator]
/// static ALLOCATOR: RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT> = RalloAllocator::new();
///
/// fn foo() {
///     let _ = String::with_capacity(1024);
/// }
///
/// ALLOCATOR.start_track();
/// foo();
/// ALLOCATOR.stop_track();
///
/// // Safety: it is called after `stop_track`
/// let stats = unsafe { ALLOCATOR.calculate_stats() };
/// let tree = stats.into_tree().unwrap();
///
/// tree.print_flamegraph("flamegraph-like-page.html");
///
/// ```
pub struct RalloAllocator<const MAX_FRAME_LENGTH: usize, const MAX_LOG_COUNT: usize> {
    is_tracking: AtomicBool,
    alloc: std::alloc::System,
    logs: LogsType<MAX_FRAME_LENGTH, MAX_LOG_COUNT>,
    logs_pointer: AtomicUsize,
}
impl<const MAX_FRAME_LENGTH: usize, const MAX_LOG_COUNT: usize> Default
    for RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const MAX_FRAME_LENGTH: usize, const MAX_LOG_COUNT: usize>
    RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT>
{
    pub const fn new() -> Self {
        RalloAllocator {
            is_tracking: AtomicBool::new(false),
            alloc: std::alloc::System,
            logs: [const { RalloUnsafeCell::new((0, 0, [FrameWrapper::new(); MAX_FRAME_LENGTH])) };
                MAX_LOG_COUNT],
            logs_pointer: AtomicUsize::new(0),
        }
    }

    /// Start recording allocations.
    pub fn start_track(&self) {
        self.is_tracking.store(true, Ordering::SeqCst);
    }

    /// Stop recording allocations.
    pub fn stop_track(&self) {
        self.is_tracking.store(false, Ordering::SeqCst);
    }

    #[allow(clippy::mut_from_ref)]
    unsafe fn get_mut(&self, index: usize) -> &mut LogType<MAX_FRAME_LENGTH> {
        let element = &self.logs[index];
        unsafe { (&mut *element.get() as &mut LogType<MAX_FRAME_LENGTH>) as _ }
    }

    unsafe fn get(&self, index: usize) -> &LogType<MAX_FRAME_LENGTH> {
        let element = &self.logs[index];
        unsafe { (&*element.get() as &LogType<MAX_FRAME_LENGTH>) as _ }
    }

    unsafe fn log(&self, layout: &Layout) {
        let index = self.logs_pointer.fetch_add(1, Ordering::SeqCst);
        if index >= MAX_LOG_COUNT {
            panic!(
                "Log buffer overflow. Maximum log count ({}) exceeded.",
                MAX_LOG_COUNT
            );
        }

        let log = unsafe { self.get_mut(index) };
        log.0 = layout.size();

        let mut i: usize = 0;
        backtrace::trace(|frame| {
            let ip: *mut c_void = frame.ip();
            log.2[i].ip = Some(ip as usize);
            i += 1;
            true
        });
        log.1 = i;
    }

    /// Calculate the statistics of the allocations.
    ///
    /// # Safety
    ///
    /// It is the caller's responsibility to ensure that the allocator is not tracking
    /// allocations when this function is called. Undefined behavior may occur if the allocator
    /// is still tracking allocations.
    ///
    pub unsafe fn calculate_stats(&self) -> Stats {
        let mut stats = Stats {
            allocations: Vec::new(),
        };

        let index = self.logs_pointer.load(Ordering::SeqCst);
        for i in 0..index {
            let log = unsafe { self.get(i) };

            let mut allocation = Allocation {
                size: log.0,
                stack: VecDeque::new(),
            };

            let stack_size = log.1;
            for j in 0..stack_size {
                let frame = &log.2[j];
                let ip = frame.ip.unwrap() as *mut c_void;

                let mut filename: Option<std::path::PathBuf> = None;
                let mut colno: Option<u32> = None;
                let mut lineno: Option<u32> = None;
                let mut fn_address: Option<*mut c_void> = None;
                let mut fn_name: Option<String> = None;
                backtrace::resolve(ip, |s| {
                    filename = s.filename().map(|f| f.to_owned());
                    colno = s.colno();
                    lineno = s.lineno();
                    fn_address = s.addr();
                    fn_name = s.name().and_then(|s| s.as_str()).map(|s| s.to_string());
                });
                allocation.stack.push_front(FrameInfo {
                    filename,
                    colno,
                    lineno,
                    fn_address,
                    fn_name,
                });
            }

            stats.allocations.push(allocation);
        }

        stats
    }
}

unsafe impl<const MAX_FRAME_LENGTH: usize, const MAX_LOG_COUNT: usize> GlobalAlloc
    for RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT>
{
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Don't track allocations if not enabled
        if self.is_tracking.load(Ordering::SeqCst) {
            unsafe { self.log(&layout) };
        }

        unsafe { self.alloc.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.alloc.dealloc(ptr, layout) }
    }
}
