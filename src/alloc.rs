use std::{
    alloc::{GlobalAlloc, Layout},
    collections::{HashSet, VecDeque},
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

type LogType<const MAX_FRAME_LENGTH: usize> = (usize, usize, usize, [FrameWrapper; MAX_FRAME_LENGTH]);
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
    allocation_logs: LogsType<MAX_FRAME_LENGTH, MAX_LOG_COUNT>,
    allocation_logs_pointer: AtomicUsize,
    deallocation_logs: [RalloUnsafeCell<(usize, usize)>; MAX_LOG_COUNT],
    deallocation_logs_pointer: AtomicUsize,
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
            allocation_logs: [
                const {
                    RalloUnsafeCell::new(
                        (
                            0, // size
                            0, // `backtrace` len (stack depth)
                            0, // ptr address
                            [FrameWrapper::new(); MAX_FRAME_LENGTH]
                        )
                    )
                };
                MAX_LOG_COUNT],
            allocation_logs_pointer: AtomicUsize::new(0),
            deallocation_logs: [
                const {
                    RalloUnsafeCell::new((0, 0))
                }; MAX_LOG_COUNT],
            deallocation_logs_pointer: AtomicUsize::new(0),
        }
    }

    /// Start recording allocations.
    pub fn start_track(&self) {
        // Ask the backtrace to allow the backtrace system inizialization
        // without tracking it.
        backtrace::trace(|_| true);

        self.is_tracking.store(true, Ordering::SeqCst);
    }

    /// Stop recording allocations.
    pub fn stop_track(&self) {
        self.is_tracking.store(false, Ordering::SeqCst);
    }

    #[allow(clippy::mut_from_ref)]
    unsafe fn get_allocation_item_mut(&self, index: usize) -> &mut LogType<MAX_FRAME_LENGTH> {
        let element = &self.allocation_logs[index];
        unsafe { (&mut *element.get() as &mut LogType<MAX_FRAME_LENGTH>) as _ }
    }

    #[allow(clippy::mut_from_ref)]
    unsafe fn get_deallocation_item_mut(&self, index: usize) -> &mut (usize, usize) {
        let element = &self.deallocation_logs[index];
        unsafe { (&mut *element.get() as &mut (usize, usize)) as _ }
    }

    unsafe fn get_allocation_item(&self, index: usize) -> &LogType<MAX_FRAME_LENGTH> {
        let element = &self.allocation_logs[index];
        unsafe { (&*element.get() as &LogType<MAX_FRAME_LENGTH>) as _ }
    }

    unsafe fn log_alloc(&self, layout: &Layout, address: usize) {
        let index = self.allocation_logs_pointer.fetch_add(1, Ordering::SeqCst);
        if index >= MAX_LOG_COUNT {
            panic!(
                "Log buffer overflow. Maximum log count ({}) exceeded.",
                MAX_LOG_COUNT
            );
        }

        // Safety: index is incrementally increasing and within bounds
        // So, we can safely get a mutable reference to the log at this index.
        let log = unsafe { self.get_allocation_item_mut(index) };
        log.0 = layout.size();

        let mut i: usize = 0;
        backtrace::trace(|frame| {
            let ip: *mut c_void = frame.ip();
            log.3[i].ip = Some(ip as usize);
            i += 1;
            true
        });
        log.1 = i;
        log.2 = address;
    }

    unsafe fn log_dealloc(&self, layout: &Layout, address: usize) {
        let size = layout.size();
        let index = self.deallocation_logs_pointer.fetch_add(1, Ordering::SeqCst);
        if index >= MAX_LOG_COUNT {
            panic!(
                "Deallocation log buffer overflow. Maximum log count ({}) exceeded.",
                MAX_LOG_COUNT
            );
        }
        // Safety: index is incrementally increasing and within bounds
        let log = unsafe { self.get_deallocation_item_mut(index) };
        log.0 = size;
        log.1 = address;
    }

    /// Calculate the statistics of the allocations.
    ///
    /// # Safety
    ///
    /// It is the caller's responsibility to ensure that the allocator is not tracking
    /// allocations when this function is called. Undefined behavior may occur if the allocator
    /// is still tracking allocations.
    /// Don't call this function concurrently
    ///
    pub unsafe fn calculate_stats(&self) -> Stats {

        let old = self.deallocation_logs_pointer.load(Ordering::SeqCst);

        let mut stats = Stats {
            allocations: VecDeque::new(),
        };

        let mut deallocation_visited = HashSet::with_capacity(old);

        let index = self.allocation_logs_pointer.load(Ordering::SeqCst);
        for i in 0..index {
            let log = unsafe { self.get_allocation_item(i) };

            let address = log.2;

            println!(
                "Allocating {} bytes at address {:x}",
                log.0,
                address
            );

            let a = self.deallocation_logs.iter().find(|d| {
                let d = unsafe { *d.get() };
                d.1 == address
            })
                .map(|d| unsafe { *d.get() });
            let deallocation_size = match a {
                    // No deallocation log found for this address
                    None => {
                        0
                    },
                    Some((size, ptr)) => {
                        println!("Deallocating {} bytes at address {:x}", size, ptr);
                        // Mark this deallocation as visited
                        deallocation_visited.insert(ptr);
                        size
                    }
            };

            let mut allocation = Allocation {
                allocation_size: log.0,
                deallocation_size,
                address,
                stack: VecDeque::new(),
            };

            let stack_size = log.1;
            for j in 0..stack_size {
                let frame = &log.3[j];
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

            stats.allocations.push_front(allocation);
        }

        self.allocation_logs_pointer.store(0, Ordering::SeqCst);
        self.deallocation_logs_pointer.store(0, Ordering::SeqCst);

        assert_eq!(old, deallocation_visited.len());

        stats
    }
}

unsafe impl<const MAX_FRAME_LENGTH: usize, const MAX_LOG_COUNT: usize> GlobalAlloc
    for RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT>
{
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {

        let ptr = unsafe { self.alloc.alloc(layout) };

        // Don't track allocations if not enabled
        if self.is_tracking.load(Ordering::SeqCst) {
            let address = ptr as usize;
            unsafe { self.log_alloc(&layout, address) };
        }

        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Don't track allocations if not enabled
        if self.is_tracking.load(Ordering::SeqCst) {
            let address = ptr as usize;
            unsafe { self.log_dealloc(&layout, address) };
        }

        unsafe { self.alloc.dealloc(ptr, layout) }
    }
}
