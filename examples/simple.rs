use rallo::RalloAllocator;

// This is the maximum length of a frame
const MAX_FRAME_LENGTH: usize = 128;
// Maximum number of allocations to keep
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
    let tree = stats.into_tree().unwrap();

    let file_name = "simple-memory-flamegraph.html";
    let path = std::fs::canonicalize(file_name).unwrap();
    tree.print_flamegraph(&path);

    println!("Flamegraph saved to {}", path.display());
}
