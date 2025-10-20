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
    unsafe { ALLOCATOR.start_track() };
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
            alloc.push((
                node.key.clone(),
                node.allocation,
                node.category,
                node.allocation,
                node.deallocation,
            ));
        }
        for child in node.children {
            stack.push(child);
        }
    }

    println!("Allocations in foo function: {:#?}", alloc);

    // 2 different allocations in the foo function
    // 1 deallocation
    assert_eq!(alloc.len(), 3);

    let sum = alloc.iter().map(|(_, size, _, _, _)| *size).sum::<usize>();
    // The exact number depends on the system and the allocator
    // but it should be greater than 2 * 1024
    // because we have 3 allocations of 1024 bytes
    // and the allocator may add some overhead
    // for the allocation
    assert!(sum >= 1024 * 2);

    let file_contents: Vec<_> = alloc
        .iter()
        .filter_map(|(key, _, _, _, _)| key.file_content.clone())
        .collect();

    let highlighteds: Vec<_> = file_contents
        .iter()
        .map(|c| c.highlighted.trim().to_string())
        .collect();
    assert_eq!(
        highlighteds,
        vec!["}", "my_clone(&s);", "let mut s = Box::new(s);",]
    );
    let befores: Vec<_> = file_contents.iter().map(|c| c.before.len()).collect();
    assert_eq!(befores, vec![5, 5, 5]);
    let afters: Vec<_> = file_contents.iter().map(|c| c.after.len()).collect();
    assert_eq!(afters, vec![5, 5, 5]);
}
