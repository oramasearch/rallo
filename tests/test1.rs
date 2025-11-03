#![allow(clippy::borrowed_box)]

use std::{collections::VecDeque, fmt::Debug, path::PathBuf};

use rallo::{FrameInfo, RalloAllocator, Tree};
use serde::Serialize;

const MAX_FRAME_LENGTH: usize = 128;
const MAX_LOG_COUNT: usize = 1_024 * 10;
#[global_allocator]
static ALLOCATOR: RalloAllocator<MAX_FRAME_LENGTH, MAX_LOG_COUNT> = RalloAllocator::new();

#[inline(never)]
fn run() {
    let _ = vec![0_u8; 1024];
}

#[test]
fn test1() {
    unsafe { ALLOCATOR.start_track() };
    run();
    ALLOCATOR.stop_track();
    let stats = unsafe { ALLOCATOR.calculate_stats() };

    let current_file: &PathBuf = &std::fs::canonicalize(file!()).unwrap();

    assert_eq!(stats.allocations.len(), 1);
    assert_eq!(stats.allocations[0].allocation_size, 1024);
    let allocation =
        extrapolate_frame(&stats.allocations[0].stack, "::run::", current_file).unwrap();
    assert_eq!(allocation.lineno, Some(15));

    assert_eq!(stats.deallocations.len(), 1);
    assert_eq!(stats.deallocations[0].deallocation_size, 1024);
    let deallocation =
        extrapolate_frame(&stats.deallocations[0].stack, "::run::", current_file).unwrap();
    assert_eq!(deallocation.lineno, Some(15));

    let tree = stats.into_tree().unwrap();

    let current_file = current_file.to_str().unwrap().to_string();
    let flatten = flat_tree(&tree);
    let nodes: Vec<_> = flatten
        .into_iter()
        .filter(|n| &n.key.filename == &current_file && n.key.fn_name.contains("::run::"))
        .collect();

    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].allocation, 1024);
    assert_eq!(nodes[0].allocation_count, 1);
    assert_eq!(nodes[0].deallocation, 0);
    assert_eq!(nodes[0].deallocation_count, 0);
    assert_eq!(nodes[1].allocation, 0);
    assert_eq!(nodes[1].allocation_count, 0);
    assert_eq!(nodes[1].deallocation, 1024);
    assert_eq!(nodes[1].deallocation_count, 1);
}

fn flat_tree<K: Debug + Serialize>(tree: &Tree<K>) -> Vec<&Tree<K>> {
    let mut result = vec![tree];
    for child in &tree.children {
        result.extend(flat_tree(child));
    }
    result
}

fn extrapolate_frame<'f>(
    frames: &'f VecDeque<FrameInfo>,
    wanted_fn_name: &str,
    filename: &PathBuf,
) -> Option<&'f FrameInfo> {
    frames.iter().find(|f| {
        if let Some(fn_name) = &f.fn_name {
            let fn_name = rustc_demangle::demangle(fn_name).to_string();
            f.filename.as_ref() == Some(filename) && fn_name.contains(&wanted_fn_name)
        } else {
            false
        }
    })
}
