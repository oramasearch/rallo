#![allow(clippy::test_attr_in_doctest)]
#![doc = include_str!("../README.md")]

mod alloc;
mod stats;
mod unsafe_cell;

pub use alloc::*;
pub use stats::*;
