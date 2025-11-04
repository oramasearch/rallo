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
    // Safety: the program is single-threaded
    unsafe { ALLOCATOR.start_track() };
    foo();
    ALLOCATOR.stop_track();

    // Safety: it is called after `stop_track`
    let stats = unsafe { ALLOCATOR.calculate_stats() };
    let profile = rallo::FirefoxProfile::from_stats(stats).unwrap();

    let file_name = "simple-memory-profile.json";
    let path = std::env::current_dir().unwrap().join(file_name);
    profile.write_json(&path).unwrap();

    println!("Firefox profile saved to {}", path.display());
}
