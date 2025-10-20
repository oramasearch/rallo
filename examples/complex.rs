use rallo::RalloAllocator;

// This is the maximum length of a frame
const MAX_FRAME_LENGTH: usize = 128;
// Maximum number of allocations to keep
const MAX_LOG_COUNT: usize = 1_024 * 10;
#[global_allocator]
static ALLOCATOR: RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT> = RalloAllocator::new();

fn foo() -> u32 {
    let vec: Vec<u32> = (0..100).collect();

    let mut sum = 0;
    for num in vec {
        sum += num;
    }

    let vec2: Vec<u32> = (0..400).collect();
    for num in vec2 {
        sum += num;
    }

    let v: Box<Vec<u8>> = Box::new(Vec::with_capacity(10));
    Box::leak(v); // Leak the box to prevent deallocation

    sum
}

fn main() {
    unsafe { ALLOCATOR.start_track() };
    foo();
    ALLOCATOR.stop_track();

    // Safety: it is called after `stop_track`
    let stats = unsafe { ALLOCATOR.calculate_stats() };
    let tree = stats.into_tree().unwrap();

    let file_name = "complex-memory-flamegraph.html";
    let path = std::env::current_dir().unwrap().join(file_name);
    tree.print_flamegraph(&path);

    println!("Flamegraph saved to {}", path.display());
}
