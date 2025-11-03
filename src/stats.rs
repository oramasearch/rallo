use std::{borrow::Cow, collections::VecDeque, ffi::c_void, fmt::Debug, io::BufRead, path::Path};

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
    pub allocation_size: usize,
    /// Deallocation size
    pub deallocation_size: usize,
    /// address of the allocation
    pub address: usize,
    /// Stack trace
    pub stack: VecDeque<FrameInfo>,
}

#[derive(Debug)]
pub struct Stats {
    /// Allocations
    pub allocations: VecDeque<Allocation>,
    /// Deallocations
    pub deallocations: VecDeque<Allocation>,
}

impl Stats {
    /// Transform the raw stats into a tree structure
    pub fn into_tree(self) -> Result<Tree<Key>, Cow<'static, str>> {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("failed to get current directory: {e:?}"))?;
        let cwd = cwd.to_str().ok_or("current directory is not valid UTF-8")?;

        let mut root = Tree {
            category: Category::Unknown,
            key: Key {
                filename: "<root>".to_string(),
                colno: 0,
                lineno: 0,
                fn_address: std::ptr::null_mut(),
                fn_name: "<root>".to_string(),
                file_content: None,
            },
            allocation: 0,
            allocation_count: 0,
            deallocation: 0,
            deallocation_count: 0,
            children: Vec::new(),
        };

        for allocation in self.allocations {
            let mut pointer = &mut root;

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
                        allocation: 0,
                        allocation_count: 0,
                        deallocation: 0,
                        deallocation_count: 0,
                        children: Vec::new(),
                    };
                    pointer.children.push(c);
                    pointer.children.last_mut().unwrap()
                };

                // Put the effort only on the last frame
                if is_last {
                    pointer.allocation += allocation.allocation_size;
                    pointer.deallocation += allocation.deallocation_size;
                    pointer.allocation_count += 1;
                }
            }
        }

        for deallocation in self.deallocations {
            let mut pointer = &mut root;

            let stack_len = deallocation.stack.len();
            for (index, info) in deallocation.stack.into_iter().enumerate() {
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
                        allocation: 0,
                        allocation_count: 0,
                        deallocation: 0,
                        deallocation_count: 0,
                        children: Vec::new(),
                    };
                    pointer.children.push(c);
                    pointer.children.last_mut().unwrap()
                };

                // Put the effort only on the last frame
                if is_last {
                    pointer.allocation += deallocation.allocation_size;
                    pointer.deallocation += deallocation.deallocation_size;
                    pointer.deallocation_count += 1;
                }
            }
        }

        root.update_value();

        Ok(root)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Clone)]
pub struct FileContent {
    pub before: Vec<String>,
    pub highlighted: String,
    pub after: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Clone)]
pub struct Key {
    pub filename: String,
    pub colno: u32,
    pub lineno: u32,
    #[serde(skip_serializing)]
    pub fn_address: *mut c_void,
    pub fn_name: String,
    pub file_content: Option<FileContent>,
}

impl TryFrom<FrameInfo> for Key {
    type Error = &'static str;

    fn try_from(value: FrameInfo) -> Result<Self, Self::Error> {
        let filename = value
            .filename
            .and_then(|filename| filename.to_str().map(|s| s.to_string()));

        if let Some(filename) = filename {
            let colno = value.colno.ok_or("colno is None")?;
            let lineno = value.lineno.ok_or("lineno is None")?;
            let fn_address = value.fn_address.ok_or("fn_address is None")?;
            let fn_name = value.fn_name.ok_or("fn_name is None")?;

            let fn_name = rustc_demangle::demangle(&fn_name).to_string();

            let delta = 5;
            let range_min = if lineno > (delta + 1) {
                lineno - delta - 1
            } else {
                0
            };
            let file_content = std::fs::File::open(&filename).ok().and_then(|file| {
                let lines = std::io::BufReader::new(file).lines();
                let mut lines: Vec<_> = lines
                    .enumerate()
                    .filter_map(|(i, line)| Some((i, line.ok()?)))
                    .skip_while(|(index, _)| *index < range_min as usize)
                    .take_while(|(index, _)| *index < lineno as usize + delta as usize)
                    .collect();

                let highlighted_index = match lines.iter().position(|(i, _)| *i as u32 == lineno) {
                    Some(i) => i,
                    None => {
                        println!("not found");
                        return None;
                    }
                };

                let mut after = lines.split_off(highlighted_index - 1);
                let highlighted = after.remove(0);
                let before = lines;

                Some(FileContent {
                    before: before.into_iter().map(|(_, line)| line).collect(),
                    highlighted: highlighted.1,
                    after: after.into_iter().map(|(_, line)| line).collect(),
                })
            });

            Ok(Key {
                filename,
                colno,
                lineno,
                fn_address,
                fn_name,
                file_content,
            })
        } else {
            Ok(Key {
                filename: "<unknown>".to_string(),
                colno: 0,
                lineno: 0,
                fn_address: std::ptr::null_mut(),
                fn_name: "<unknown>".to_string(),
                file_content: None,
            })
        }
    }
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
/// Tree structure for the flamegraph
pub struct Tree<K: Debug + Serialize> {
    pub key: K,
    pub allocation: usize,
    pub allocation_count: usize,
    pub deallocation: usize,
    pub deallocation_count: usize,
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
        let html = html.replace("{ undefined }", &d);
        std::fs::write(path, html).unwrap();
    }

    fn update_value(&mut self) {
        let mut allocation = 0;
        let mut allocation_count = 0;
        let mut deallocation = 0;
        let mut deallocation_count = 0;
        for child in &mut self.children {
            child.update_value();
            allocation += child.allocation;
            if child.allocation > 0 {
                allocation_count += child.allocation_count;
            }
            deallocation += child.deallocation;
            if child.deallocation > 0 {
                deallocation_count += child.deallocation_count;
            }
        }
        self.allocation += allocation;
        self.allocation_count += allocation_count;
        self.deallocation += deallocation;
        self.deallocation_count += deallocation_count;
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
            deallocations: VecDeque::new(),
            allocations: VecDeque::from([Allocation {
                allocation_size: 1024,
                deallocation_size: 0,
                address: 0,
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
            }]),
        };
        let tree = stats.into_tree().unwrap();

        assert_eq!(
            tree,
            Tree {
                key: Key {
                    filename: "<root>".to_string(),
                    colno: 0,
                    lineno: 0,
                    fn_address: std::ptr::null_mut(),
                    fn_name: "<root>".to_string(),
                    file_content: None,
                },
                allocation: 1024,
                allocation_count: 1,
                deallocation: 0,
                deallocation_count: 0,
                category: Category::Unknown,
                children: vec![Tree {
                    key: Key {
                        filename: "foo.rs".to_string(),
                        colno: 1,
                        lineno: 1,
                        fn_address: std::ptr::null_mut(),
                        fn_name: "foo".to_string(),
                        file_content: None,
                    },
                    allocation: 1024,
                    allocation_count: 1,
                    deallocation: 0,
                    deallocation_count: 0,
                    category: Category::Unknown,
                    children: vec![Tree {
                        key: Key {
                            filename: "foo2.rs".to_string(),
                            colno: 1,
                            lineno: 1,
                            fn_address: std::ptr::null_mut(),
                            fn_name: "foo2".to_string(),
                            file_content: None,
                        },
                        allocation: 1024,
                        allocation_count: 1,
                        deallocation: 0,
                        deallocation_count: 0,
                        category: Category::Unknown,
                        children: vec![Tree {
                            key: Key {
                                filename: "foo3.rs".to_string(),
                                colno: 1,
                                lineno: 1,
                                fn_address: std::ptr::null_mut(),
                                fn_name: "foo3".to_string(),
                                file_content: None,
                            },
                            allocation: 1024,
                            allocation_count: 1,
                            deallocation: 0,
                            deallocation_count: 0,
                            category: Category::Unknown,
                            children: vec![],
                        }],
                    }],
                }]
            }
        );
    }

    #[test]
    fn test_tree_value_2() {
        let stats = Stats {
            deallocations: VecDeque::new(),
            allocations: VecDeque::from([
                Allocation {
                    allocation_size: 1024,
                    deallocation_size: 0,
                    address: 0,
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
                    allocation_size: 1024,
                    deallocation_size: 0,
                    address: 0,
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
            ]),
        };
        let tree = stats.into_tree().unwrap();

        assert_eq!(
            tree,
            Tree {
                key: Key {
                    filename: "<root>".to_string(),
                    colno: 0,
                    lineno: 0,
                    fn_address: std::ptr::null_mut(),
                    fn_name: "<root>".to_string(),
                    file_content: None,
                },
                allocation: 1024 * 2,
                allocation_count: 2,
                deallocation: 0,
                deallocation_count: 0,
                category: Category::Unknown,
                children: vec![Tree {
                    key: Key {
                        filename: "foo.rs".to_string(),
                        colno: 1,
                        lineno: 1,
                        fn_address: std::ptr::null_mut(),
                        fn_name: "foo".to_string(),
                        file_content: None,
                    },
                    allocation: 1024 * 2,
                    allocation_count: 2,
                    deallocation: 0,
                    deallocation_count: 0,
                    category: Category::Unknown,
                    children: vec![Tree {
                        key: Key {
                            filename: "foo2.rs".to_string(),
                            colno: 1,
                            lineno: 1,
                            fn_address: std::ptr::null_mut(),
                            fn_name: "foo2".to_string(),
                            file_content: None,
                        },
                        allocation: 1024 * 2,
                        allocation_count: 2,
                        deallocation: 0,
                        deallocation_count: 0,
                        category: Category::Unknown,
                        children: vec![Tree {
                            key: Key {
                                filename: "foo3.rs".to_string(),
                                colno: 1,
                                lineno: 1,
                                fn_address: std::ptr::null_mut(),
                                fn_name: "foo3".to_string(),
                                file_content: None,
                            },
                            allocation: 1024 * 2,
                            allocation_count: 2,
                            deallocation: 0,
                            deallocation_count: 0,
                            category: Category::Unknown,
                            children: vec![],
                        }],
                    }],
                }]
            }
        );
    }

    #[test]
    fn test_tree_value_3() {
        let stats = Stats {
            deallocations: VecDeque::new(),
            allocations: VecDeque::from([
                Allocation {
                    allocation_size: 1024,
                    deallocation_size: 0,
                    address: 0,
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
                    allocation_size: 1024,
                    deallocation_size: 0,
                    address: 0,
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
            ]),
        };
        let tree = stats.into_tree().unwrap();

        assert_eq!(
            tree,
            Tree {
                key: Key {
                    filename: "<root>".to_string(),
                    colno: 0,
                    lineno: 0,
                    fn_address: std::ptr::null_mut(),
                    fn_name: "<root>".to_string(),
                    file_content: None,
                },
                allocation: 1024 * 2,
                allocation_count: 2,
                deallocation: 0,
                deallocation_count: 0,
                category: Category::Unknown,
                children: vec![Tree {
                    key: Key {
                        filename: "foo.rs".to_string(),
                        colno: 1,
                        lineno: 1,
                        fn_address: std::ptr::null_mut(),
                        fn_name: "foo".to_string(),
                        file_content: None,
                    },
                    allocation: 1024 * 2,
                    allocation_count: 2,
                    deallocation: 0,
                    deallocation_count: 0,
                    category: Category::Unknown,
                    children: vec![Tree {
                        key: Key {
                            filename: "foo2.rs".to_string(),
                            colno: 1,
                            lineno: 1,
                            fn_address: std::ptr::null_mut(),
                            fn_name: "foo2".to_string(),
                            file_content: None,
                        },
                        allocation: 1024 * 2,
                        allocation_count: 2,
                        deallocation: 0,
                        deallocation_count: 0,
                        category: Category::Unknown,
                        children: vec![Tree {
                            key: Key {
                                filename: "foo3.rs".to_string(),
                                colno: 1,
                                lineno: 1,
                                fn_address: std::ptr::null_mut(),
                                fn_name: "foo3".to_string(),
                                file_content: None,
                            },
                            allocation: 1024 * 2,
                            allocation_count: 2,
                            deallocation: 0,
                            deallocation_count: 0,
                            category: Category::Unknown,
                            children: vec![Tree {
                                key: Key {
                                    filename: "foo4.rs".to_string(),
                                    colno: 1,
                                    lineno: 1,
                                    fn_address: std::ptr::null_mut(),
                                    fn_name: "foo4".to_string(),
                                    file_content: None,
                                },
                                allocation: 1024,
                                allocation_count: 1,
                                deallocation: 0,
                                deallocation_count: 0,
                                category: Category::Unknown,
                                children: vec![],
                            }],
                        }],
                    }],
                }]
            }
        );
    }

    #[test]
    fn test_tree_value_4() {
        let stats = Stats {
            deallocations: VecDeque::new(),
            allocations: VecDeque::from([
                Allocation {
                    allocation_size: 1024,
                    deallocation_size: 0,
                    address: 0,
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
                    allocation_size: 1024,
                    deallocation_size: 0,
                    address: 0,
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
            ]),
        };
        let tree = stats.into_tree().unwrap();

        assert_eq!(
            tree,
            Tree {
                key: Key {
                    filename: "<root>".to_string(),
                    colno: 0,
                    lineno: 0,
                    fn_address: std::ptr::null_mut(),
                    fn_name: "<root>".to_string(),
                    file_content: None,
                },
                allocation: 1024 * 2,
                allocation_count: 2,
                deallocation: 0,
                deallocation_count: 0,
                category: Category::Unknown,
                children: vec![Tree {
                    key: Key {
                        filename: "foo.rs".to_string(),
                        colno: 1,
                        lineno: 1,
                        fn_address: std::ptr::null_mut(),
                        fn_name: "foo".to_string(),
                        file_content: None,
                    },
                    allocation: 1024 * 2,
                    allocation_count: 2,
                    deallocation: 0,
                    deallocation_count: 0,
                    category: Category::Unknown,
                    children: vec![Tree {
                        key: Key {
                            filename: "foo2.rs".to_string(),
                            colno: 1,
                            lineno: 1,
                            fn_address: std::ptr::null_mut(),
                            fn_name: "foo2".to_string(),
                            file_content: None,
                        },
                        allocation: 1024 * 2,
                        allocation_count: 2,
                        deallocation: 0,
                        deallocation_count: 0,
                        category: Category::Unknown,
                        children: vec![
                            Tree {
                                key: Key {
                                    filename: "foo3.rs".to_string(),
                                    colno: 1,
                                    lineno: 1,
                                    fn_address: std::ptr::null_mut(),
                                    fn_name: "foo3".to_string(),
                                    file_content: None,
                                },
                                allocation: 1024,
                                allocation_count: 1,
                                deallocation: 0,
                                deallocation_count: 0,
                                category: Category::Unknown,
                                children: vec![],
                            },
                            Tree {
                                key: Key {
                                    filename: "foo4.rs".to_string(),
                                    colno: 1,
                                    lineno: 1,
                                    fn_address: std::ptr::null_mut(),
                                    fn_name: "foo4".to_string(),
                                    file_content: None,
                                },
                                allocation: 1024,
                                allocation_count: 1,
                                deallocation: 0,
                                deallocation_count: 0,
                                category: Category::Unknown,
                                children: vec![],
                            },
                        ],
                    }],
                }]
            }
        );
    }
}
