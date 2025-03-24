#![allow(clippy::borrowed_box)]

use rallo::RalloAllocator;

const MAX_FRAME_LENGTH: usize = 128;
const MAX_LOG_COUNT: usize = 1_024 * 10;
#[global_allocator]
static ALLOCATOR: RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT> = RalloAllocator::new();

#[inline(never)]
fn my_clone(s: &Box<[u8; 1024]>) {
    let mut a: Box<[u8; 1024]> = s.clone();
    a[0] = 2;
}

#[inline(never)]
fn foo() {
    let s = [0_u8; 1024];
    let mut s = Box::new(s);
    s[0] = 1;
    my_clone(&s);
}

#[test]
fn test_stack() {
    ALLOCATOR.start_track();
    foo();
    ALLOCATOR.stop_track();

    // Safety: it is called after `stop_track`
    let stats = unsafe { ALLOCATOR.calculate_stats() };

    let tree = stats.into_tree().unwrap();

    tree.print_flamegraph("foo.html");

    let current_file = std::fs::canonicalize(file!()).unwrap();
    let current_file = current_file.to_str().unwrap();

    let mut alloc = vec![];
    let mut stack = vec![tree];
    while let Some(node) = stack.pop() {
        if node.key.filename == current_file && node.key.fn_name.contains("foo") {
            alloc.push((node.key.clone(), node.value, node.category));
        }
        for child in node.children {
            stack.push(child);
        }
    }

    // 2 different allocations in the foo function
    assert_eq!(alloc.len(), 2);

    let sum = alloc.into_iter().map(|(_, size, _)| size).sum::<usize>();
    // The exact number depends on the system and the allocator
    // but it should be greater than 2 * 1024
    // because we have 2 allocations of 1024 bytes
    // and the allocator may add some overhead
    // for the allocation
    assert!(sum >= 1024 * 2);
}
