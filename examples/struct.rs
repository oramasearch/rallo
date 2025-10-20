use rallo::RalloAllocator;

// This is the maximum length of a frame
const MAX_FRAME_LENGTH: usize = 128;
// Maximum number of allocations to keep
const MAX_LOG_COUNT: usize = 1_024 * 10;
#[global_allocator]
static ALLOCATOR: RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT> = RalloAllocator::new();

struct Foo {
    a: Vec<u32>,
}

impl Foo {
    fn new() -> Self {
        Foo { a: Vec::new() }
    }

    fn add(&mut self, value: u32) {
        self.a.push(value);
    }
}

fn main() {
    let mut f = Foo::new();
    // Safety: the program is single-threaded
    unsafe { ALLOCATOR.start_track() };
    for i in 0..100 {
        f.add(i);
    }
    f.a.shrink_to_fit();
    ALLOCATOR.stop_track();

    // Safety: it is called after `stop_track`
    let stats = unsafe { ALLOCATOR.calculate_stats() };
    let tree = stats.into_tree().unwrap();

    let file_name = "struct-memory-flamegraph.html";
    let path = std::env::current_dir().unwrap().join(file_name);
    tree.print_flamegraph(&path);

    println!("Flamegraph saved to {}", path.display());
}
