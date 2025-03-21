use rallo::RalloAllocator;

const MAX_FRAME_LENGTH: usize = 128;
const MAX_LOG_COUNT: usize = 1_024 * 10;
#[global_allocator]
static ALLOCATOR: RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT> = RalloAllocator::new();

fn foo() {
    let _ = String::with_capacity(1024);
}

fn main() {
    ALLOCATOR.start_track();
    foo();
    ALLOCATOR.stop_track();

    // Safety: it is called after `stop_track`
    let stats = unsafe { ALLOCATOR.calculate_stats() };
    let tree = stats.into_tree();

    tree.print_flamegraph("simple-memory-flamegraph.html");
}
