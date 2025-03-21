use std::{ffi::c_void, fmt::Debug, path::Path};

use serde::Serialize;

#[derive(Debug, Clone)]
pub struct FrameInfo {
    /// Filename where the function call was made
    pub filename: Option<std::path::PathBuf>,
    /// Column number where the function call was made
    pub colno: Option<u32>,
    /// Line number where the function call was made
    pub lineno: Option<u32>,
    /// Address of the function
    pub fn_address: Option<*mut c_void>,
    /// Name of the function
    pub fn_name: Option<String>,
}

#[derive(Debug)]
pub struct Allocation {
    /// Allocation size
    pub size: usize,
    /// Stack trace
    pub stack: Vec<FrameInfo>,
}

#[derive(Debug)]
pub struct Stats {
    /// Allocations
    pub allocations: Vec<Allocation>,
}

impl Stats {
    /// Transform the raw stats into a tree structure
    pub fn into_tree(mut self) -> Tree<Key, usize> {
        let cwd = std::env::current_dir().unwrap();
        let cwd = cwd.to_str().unwrap();

        let allocation = self.allocations.pop().unwrap();

        let mut stack = allocation.stack;
        stack.reverse();

        let key = loop {
            let root = stack.pop();
            let root = match root {
                None => panic!("stack is empty"),
                Some(root) => root,
            };
            let key = TryInto::<Key>::try_into(root);
            if let Ok(key) = key {
                break key;
            }
        };

        let mut tree = Tree {
            category: guess_category(cwd, key.filename.as_str()),
            key,
            value: 0,
            children: Vec::new(),
        };
        let mut pointer = &mut tree;
        for info in stack {
            let key: Key = match info.try_into() {
                Ok(key) => key,
                Err(_) => continue,
            };

            let c = Tree {
                category: guess_category(cwd, key.filename.as_str()),
                key,
                value: 0,
                children: Vec::new(),
            };
            pointer.children.push(c);
            pointer = pointer.children.last_mut().unwrap();
        }

        for mut allocation in self.allocations {
            allocation.stack.reverse();
            let mut pointer = &mut tree;
            for info in allocation.stack {
                let key: Key = match info.try_into() {
                    Ok(key) => key,
                    Err(_) => continue,
                };

                let found = pointer.children.iter().position(|c| c.key == key);
                pointer = if let Some(found) = found {
                    pointer.children.get_mut(found).unwrap()
                } else {
                    let c = Tree {
                        category: guess_category(cwd, key.filename.as_str()),
                        key,
                        value: allocation.size,
                        children: Vec::new(),
                    };
                    pointer.children.push(c);
                    pointer.children.last_mut().unwrap()
                };
            }
        }

        tree
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Key {
    pub filename: String,
    pub colno: u32,
    pub lineno: u32,
    #[serde(skip_serializing)]
    pub fn_address: *mut c_void,
    pub fn_name: String,
}

impl TryFrom<FrameInfo> for Key {
    type Error = &'static str;

    fn try_from(value: FrameInfo) -> Result<Self, Self::Error> {
        let filename = value.filename.ok_or("filename is None")?;
        let filename = filename.to_str().ok_or("filename is not valid UTF-8")?;
        let filename = filename.to_string();

        let colno = value.colno.ok_or("colno is None")?;
        let lineno = value.lineno.ok_or("lineno is None")?;
        let fn_address = value.fn_address.ok_or("fn_address is None")?;
        let fn_name = value.fn_name.ok_or("fn_name is None")?;

        Ok(Key {
            filename,
            colno,
            lineno,
            fn_address,
            fn_name,
        })
    }
}

#[derive(Debug, Serialize)]
/// Tree structure for the flamegraph
pub struct Tree<K: Debug + Serialize, V: Debug + Serialize> {
    pub key: K,
    pub value: V,
    pub category: Category,
    pub children: Vec<Tree<K, V>>,
}

impl<K: Debug + Serialize, V: Debug + Serialize> Tree<K, V> {
    /// Write an HTML file with the flamegraph at the given path
    pub fn print_flamegraph<P>(&self, path: P)
    where
        P: AsRef<Path>,
    {
        let d = serde_json::to_string(&self).unwrap();
        let html = include_str!("../template.html");
        let html = html.replace("{undefined}", &d);
        std::fs::write(path, html).unwrap();
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
/// Category of the allocation
/// - `rustc`: Rust compiler
/// - `ruststdlib`: Rust standard library
/// - `deps`: Dependencies
/// - `application`: Application code
/// - `unknown`: Unknown code
///
/// The category is determined by the path of the file.
pub enum Category {
    RustStdLib,
    RustC,
    Deps,
    Application,
    Unknown,
}

fn guess_category(cwd: &str, filename: &str) -> Category {
    if filename.contains("/rustc/") {
        Category::RustC
    } else if filename.contains("/rustlib/") {
        Category::RustStdLib
    } else if filename.contains("cargo/registry/src") {
        Category::Deps
    } else if filename.starts_with(cwd) {
        Category::Application
    } else {
        Category::Unknown
    }
}
