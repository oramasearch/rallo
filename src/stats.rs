use std::{borrow::Cow, collections::VecDeque, ffi::c_void, fmt::Debug, path::Path};

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
    pub stack: VecDeque<FrameInfo>,
}

#[derive(Debug)]
pub struct Stats {
    /// Allocations
    pub allocations: Vec<Allocation>,
}

impl Stats {
    /// Transform the raw stats into a tree structure
    pub fn into_tree(mut self) -> Result<Tree<Key>, Cow<'static, str>> {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("failed to get current directory: {:?}", e))?;
        let cwd = cwd.to_str().ok_or("current directory is not valid UTF-8")?;

        let allocation = match self.allocations.pop() {
            None => {
                return Err("no allocations".into());
            }
            Some(allocation) => allocation,
        };

        let mut stack = allocation.stack;

        let initial_key = loop {
            let root = stack.pop_front();
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
            category: guess_category(cwd, initial_key.filename.as_str()),
            key: initial_key.clone(),
            value: 0,
            children: Vec::new(),
        };
        let mut pointer = &mut tree;
        let stack_len = stack.len();
        for (index, info) in stack.into_iter().enumerate() {
            let is_last = stack_len == index + 1;
            let key: Key = match info.try_into() {
                Ok(key) => key,
                Err(_) => continue,
            };

            let mut c = Tree {
                category: guess_category(cwd, key.filename.as_str()),
                key,
                value: 0,
                children: Vec::new(),
            };
            if is_last {
                c.value += allocation.size;
            }
            pointer.children.push(c);
            pointer = pointer.children.last_mut().unwrap();
        }

        for mut allocation in self.allocations {
            let mut pointer = &mut tree;

            // Ensure the first frame of the allocation is the same as the first frame of the tree
            // Otherwise we have 2 roots.
            // Technically we should merge the "root" tree with the current one.
            // But, it is effortly to do so.
            // So we just panic.
            let first_key: Key = loop {
                let s = match allocation.stack.pop_front() {
                    None => panic!("stack is empty"),
                    Some(s) => s,
                };
                match s.try_into() {
                    Ok(k) => break k,
                    Err(_) => continue,
                };
            };
            if first_key != initial_key {
                panic!("first frame of allocation is not the same as the first frame of the tree");
            }

            let stack_len = allocation.stack.len();
            for (index, info) in allocation.stack.into_iter().enumerate() {
                let is_last = stack_len == index + 1;
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
                        value: 0,
                        children: Vec::new(),
                    };
                    pointer.children.push(c);
                    pointer.children.last_mut().unwrap()
                };

                // Put the effort only on the last frame
                if is_last {
                    pointer.value += allocation.size;
                }
            }
        }

        tree.update_value();

        Ok(tree)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Clone)]
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

        let fn_name = rustc_demangle::demangle(&fn_name).to_string();

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
#[cfg_attr(test, derive(PartialEq, Eq))]
/// Tree structure for the flamegraph
pub struct Tree<K: Debug + Serialize> {
    pub key: K,
    pub value: usize,
    pub category: Category,
    pub children: Vec<Tree<K>>,
}

impl<K: Debug + Serialize> Tree<K> {
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

    fn update_value(&mut self) {
        let mut value = 0;
        for child in &mut self.children {
            child.update_value();
            value += child.value;
        }
        self.value += value;
    }
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
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

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_tree_value_1() {
        let stats = Stats {
            allocations: vec![Allocation {
                size: 1024,
                stack: VecDeque::from([
                    FrameInfo {
                        filename: Some("foo.rs".into()),
                        colno: Some(1),
                        lineno: Some(1),
                        fn_address: Some(std::ptr::null_mut()),
                        fn_name: Some("foo".into()),
                    },
                    FrameInfo {
                        filename: Some("foo2.rs".into()),
                        colno: Some(1),
                        lineno: Some(1),
                        fn_address: Some(std::ptr::null_mut()),
                        fn_name: Some("foo2".into()),
                    },
                    FrameInfo {
                        filename: Some("foo3.rs".into()),
                        colno: Some(1),
                        lineno: Some(1),
                        fn_address: Some(std::ptr::null_mut()),
                        fn_name: Some("foo3".into()),
                    },
                ]),
            }],
        };
        let tree = stats.into_tree().unwrap();

        assert_eq!(
            tree,
            Tree {
                key: Key {
                    filename: "foo.rs".to_string(),
                    colno: 1,
                    lineno: 1,
                    fn_address: std::ptr::null_mut(),
                    fn_name: "foo".to_string(),
                },
                value: 1024,
                category: Category::Unknown,
                children: vec![Tree {
                    key: Key {
                        filename: "foo2.rs".to_string(),
                        colno: 1,
                        lineno: 1,
                        fn_address: std::ptr::null_mut(),
                        fn_name: "foo2".to_string(),
                    },
                    value: 1024,
                    category: Category::Unknown,
                    children: vec![Tree {
                        key: Key {
                            filename: "foo3.rs".to_string(),
                            colno: 1,
                            lineno: 1,
                            fn_address: std::ptr::null_mut(),
                            fn_name: "foo3".to_string(),
                        },
                        value: 1024,
                        category: Category::Unknown,
                        children: vec![],
                    }],
                }],
            }
        );
    }

    #[test]
    fn test_tree_value_2() {
        let stats = Stats {
            allocations: vec![
                Allocation {
                    size: 1024,
                    stack: VecDeque::from([
                        FrameInfo {
                            filename: Some("foo.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo".into()),
                        },
                        FrameInfo {
                            filename: Some("foo2.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo2".into()),
                        },
                        FrameInfo {
                            filename: Some("foo3.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo3".into()),
                        },
                    ]),
                },
                Allocation {
                    size: 1024,
                    stack: VecDeque::from([
                        FrameInfo {
                            filename: Some("foo.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo".into()),
                        },
                        FrameInfo {
                            filename: Some("foo2.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo2".into()),
                        },
                        FrameInfo {
                            filename: Some("foo3.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo3".into()),
                        },
                    ]),
                },
            ],
        };
        let tree = stats.into_tree().unwrap();

        assert_eq!(
            tree,
            Tree {
                key: Key {
                    filename: "foo.rs".to_string(),
                    colno: 1,
                    lineno: 1,
                    fn_address: std::ptr::null_mut(),
                    fn_name: "foo".to_string(),
                },
                value: 1024 * 2,
                category: Category::Unknown,
                children: vec![Tree {
                    key: Key {
                        filename: "foo2.rs".to_string(),
                        colno: 1,
                        lineno: 1,
                        fn_address: std::ptr::null_mut(),
                        fn_name: "foo2".to_string(),
                    },
                    value: 1024 * 2,
                    category: Category::Unknown,
                    children: vec![Tree {
                        key: Key {
                            filename: "foo3.rs".to_string(),
                            colno: 1,
                            lineno: 1,
                            fn_address: std::ptr::null_mut(),
                            fn_name: "foo3".to_string(),
                        },
                        value: 1024 * 2,
                        category: Category::Unknown,
                        children: vec![],
                    }],
                }],
            }
        );
    }

    #[test]
    fn test_tree_value_3() {
        let stats = Stats {
            allocations: vec![
                Allocation {
                    size: 1024,
                    stack: VecDeque::from([
                        FrameInfo {
                            filename: Some("foo.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo".into()),
                        },
                        FrameInfo {
                            filename: Some("foo2.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo2".into()),
                        },
                        FrameInfo {
                            filename: Some("foo3.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo3".into()),
                        },
                    ]),
                },
                Allocation {
                    size: 1024,
                    stack: VecDeque::from([
                        FrameInfo {
                            filename: Some("foo.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo".into()),
                        },
                        FrameInfo {
                            filename: Some("foo2.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo2".into()),
                        },
                        FrameInfo {
                            filename: Some("foo3.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo3".into()),
                        },
                        FrameInfo {
                            filename: Some("foo4.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo4".into()),
                        },
                    ]),
                },
            ],
        };
        let tree = stats.into_tree().unwrap();

        assert_eq!(
            tree,
            Tree {
                key: Key {
                    filename: "foo.rs".to_string(),
                    colno: 1,
                    lineno: 1,
                    fn_address: std::ptr::null_mut(),
                    fn_name: "foo".to_string(),
                },
                value: 1024 * 2,
                category: Category::Unknown,
                children: vec![Tree {
                    key: Key {
                        filename: "foo2.rs".to_string(),
                        colno: 1,
                        lineno: 1,
                        fn_address: std::ptr::null_mut(),
                        fn_name: "foo2".to_string(),
                    },
                    value: 1024 * 2,
                    category: Category::Unknown,
                    children: vec![Tree {
                        key: Key {
                            filename: "foo3.rs".to_string(),
                            colno: 1,
                            lineno: 1,
                            fn_address: std::ptr::null_mut(),
                            fn_name: "foo3".to_string(),
                        },
                        value: 1024 * 2,
                        category: Category::Unknown,
                        children: vec![Tree {
                            key: Key {
                                filename: "foo4.rs".to_string(),
                                colno: 1,
                                lineno: 1,
                                fn_address: std::ptr::null_mut(),
                                fn_name: "foo4".to_string(),
                            },
                            value: 1024,
                            category: Category::Unknown,
                            children: vec![],
                        }],
                    }],
                }],
            }
        );
    }

    #[test]
    fn test_tree_value_4() {
        let stats = Stats {
            allocations: vec![
                Allocation {
                    size: 1024,
                    stack: VecDeque::from([
                        FrameInfo {
                            filename: Some("foo.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo".into()),
                        },
                        FrameInfo {
                            filename: Some("foo2.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo2".into()),
                        },
                        FrameInfo {
                            filename: Some("foo3.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo3".into()),
                        },
                    ]),
                },
                Allocation {
                    size: 1024,
                    stack: VecDeque::from([
                        FrameInfo {
                            filename: Some("foo.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo".into()),
                        },
                        FrameInfo {
                            filename: Some("foo2.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo2".into()),
                        },
                        FrameInfo {
                            filename: Some("foo4.rs".into()),
                            colno: Some(1),
                            lineno: Some(1),
                            fn_address: Some(std::ptr::null_mut()),
                            fn_name: Some("foo4".into()),
                        },
                    ]),
                },
            ],
        };
        let tree = stats.into_tree().unwrap();

        assert_eq!(
            tree,
            Tree {
                key: Key {
                    filename: "foo.rs".to_string(),
                    colno: 1,
                    lineno: 1,
                    fn_address: std::ptr::null_mut(),
                    fn_name: "foo".to_string(),
                },
                value: 1024 * 2,
                category: Category::Unknown,
                children: vec![Tree {
                    key: Key {
                        filename: "foo2.rs".to_string(),
                        colno: 1,
                        lineno: 1,
                        fn_address: std::ptr::null_mut(),
                        fn_name: "foo2".to_string(),
                    },
                    value: 1024 * 2,
                    category: Category::Unknown,
                    children: vec![
                        Tree {
                            key: Key {
                                filename: "foo4.rs".to_string(),
                                colno: 1,
                                lineno: 1,
                                fn_address: std::ptr::null_mut(),
                                fn_name: "foo4".to_string(),
                            },
                            value: 1024,
                            category: Category::Unknown,
                            children: vec![],
                        },
                        Tree {
                            key: Key {
                                filename: "foo3.rs".to_string(),
                                colno: 1,
                                lineno: 1,
                                fn_address: std::ptr::null_mut(),
                                fn_name: "foo3".to_string(),
                            },
                            value: 1024,
                            category: Category::Unknown,
                            children: vec![],
                        }
                    ],
                }],
            }
        );
    }
}
