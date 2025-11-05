use std::{
    borrow::Cow,
    collections::HashMap,
    convert::TryFrom,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use fxprof_processed_profile::debugid::DebugId;
use rustc_demangle::try_demangle;

use crate::stats::{Allocation, FrameInfo as StatsFrameInfo, Stats};
use fxprof_processed_profile::ReferenceTimestamp;
use fxprof_processed_profile::{
    CategoryColor, CategoryHandle, CategoryPairHandle, Frame as FxFrame,
    FrameFlags as FxFrameFlags, FrameInfo as FxFrameInfo, LibraryHandle, LibraryInfo,
    ProcessHandle, Profile, SamplingInterval, StackHandle, Symbol, SymbolTable, ThreadHandle,
    Timestamp,
};
use serde_json::Error as SerdeError;

/// Wrapper around `fxprof_processed_profile::Profile` produced from allocation stats.
#[derive(Debug)]
pub struct FirefoxProfile {
    inner: Profile,
}

impl FirefoxProfile {
    /// Build a Firefox profile from the recorded allocation metadata.
    pub fn from_stats(stats: Stats) -> Result<Self, Cow<'static, str>> {
        let mut builder = FirefoxProfileBuilder::new()?;
        builder.ingest(stats);
        let profile = builder.finish();
        Ok(Self { inner: profile })
    }

    /// Serialize the Firefox profile to the given JSON file path.
    pub fn write_json<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let writer = std::io::BufWriter::new(std::fs::File::create(path)?);
        serde_json::to_writer(writer, &self.inner).map_err(std::io::Error::other)
    }

    /// Serialize the profile into a JSON string.
    pub fn to_json_string(&self) -> Result<String, SerdeError> {
        serde_json::to_string(&self.inner)
    }

    /// Access the underlying `fxprof_processed_profile::Profile`.
    pub fn as_profile(&self) -> &Profile {
        &self.inner
    }

    /// Consume the wrapper and return the `Profile`.
    pub fn into_profile(self) -> Profile {
        self.inner
    }
}

struct FirefoxProfileBuilder {
    profile: Profile,
    process: ProcessHandle,
    thread: ThreadHandle,
    categories: CategoryHandles,
    cwd: PathBuf,
    symbol_registry: SymbolRegistry,
    current_time_ns: u64,
    last_timestamp: Timestamp,
}

impl FirefoxProfileBuilder {
    fn new() -> Result<Self, Cow<'static, str>> {
        let mut profile = Profile::new(
            "rallo memory profile",
            ReferenceTimestamp::from(SystemTime::now()),
            SamplingInterval::from_millis(1),
        );
        profile.set_symbolicated(true);

        let process = profile.add_process("rallo", 1, Timestamp::from_millis_since_reference(0.0));
        let thread = profile.add_thread(
            process,
            1,
            Timestamp::from_millis_since_reference(0.0),
            true,
        );
        profile.set_thread_name(thread, "Allocations");
        profile.add_initial_visible_thread(thread);
        profile.add_initial_selected_thread(thread);

        let cwd = std::env::current_dir()
            .map_err(|e| Cow::Owned(format!("failed to get current directory: {e:?}")))?;

        let categories = CategoryHandles::new(&mut profile);

        Ok(Self {
            profile,
            process,
            thread,
            categories,
            cwd,
            symbol_registry: SymbolRegistry::new(),
            current_time_ns: 0,
            last_timestamp: Timestamp::from_nanos_since_reference(0),
        })
    }

    fn ingest(&mut self, stats: Stats) {
        self.process_allocation_iter(
            stats.allocations.into_iter().rev(),
            AllocationKind::Allocation,
        );
        self.process_allocation_iter(
            stats.deallocations.into_iter().rev(),
            AllocationKind::Deallocation,
        );
    }

    fn finish(mut self) -> Profile {
        self.profile
            .set_process_end_time(self.process, self.last_timestamp);
        self.profile
            .set_thread_end_time(self.thread, self.last_timestamp);
        self.symbol_registry.finalize(&mut self.profile);
        self.profile
    }

    fn process_allocation_iter<I>(&mut self, allocations: I, kind: AllocationKind)
    where
        I: Iterator<Item = Allocation>,
    {
        for allocation in allocations {
            let size = match kind {
                AllocationKind::Allocation => allocation.allocation_size,
                AllocationKind::Deallocation => allocation.deallocation_size,
            };
            if size == 0 {
                continue;
            }

            self.current_time_ns = self.current_time_ns.saturating_add(1_000_000);
            self.last_timestamp = Timestamp::from_nanos_since_reference(self.current_time_ns);

            let stack = self.build_stack(&allocation.stack);
            let address = allocation.address as u64;
            let size = match kind {
                AllocationKind::Allocation => usize_to_i64(size),
                AllocationKind::Deallocation => -usize_to_i64(size),
            };

            self.profile.add_allocation_sample(
                self.thread,
                self.last_timestamp,
                stack,
                address,
                size,
            );
        }
    }

    fn build_stack(
        &mut self,
        stack: &std::collections::VecDeque<StatsFrameInfo>,
    ) -> Option<StackHandle> {
        let frames: Vec<FxFrameInfo> = stack
            .iter()
            .filter_map(|frame| self.convert_frame(frame))
            .collect();

        if frames.is_empty() {
            None
        } else {
            self.profile
                .intern_stack_frames(self.thread, frames.into_iter())
        }
    }

    fn convert_frame(&mut self, frame: &StatsFrameInfo) -> Option<FxFrameInfo> {
        let category = determine_category(&self.cwd, frame.filename.as_deref());
        let name = demangle_name(frame.fn_name.as_deref());
        if name.is_empty() {
            return None;
        }

        let fx_frame = self.symbol_registry.resolve_frame(
            &mut self.profile,
            frame.filename.as_deref(),
            &name,
            frame.lineno,
            frame.colno,
        );

        Some(FxFrameInfo {
            frame: fx_frame,
            category_pair: self.categories.get(category),
            flags: FxFrameFlags::empty(),
        })
    }
}

#[derive(Clone, Copy)]
enum AllocationKind {
    Allocation,
    Deallocation,
}

struct SymbolRegistry {
    libraries: HashMap<PathBuf, LibraryEntry>,
}

impl SymbolRegistry {
    fn new() -> Self {
        Self {
            libraries: HashMap::new(),
        }
    }

    fn resolve_frame(
        &mut self,
        profile: &mut Profile,
        filename: Option<&Path>,
        function: &str,
        lineno: Option<u32>,
        colno: Option<u32>,
    ) -> FxFrame {
        if let Some(path) = filename
            && let Some(frame) = self.resolve_with_library(profile, path, function, lineno, colno)
        {
            return frame;
        }

        let interned = profile.intern_string(function);
        FxFrame::Label(interned)
    }

    fn resolve_with_library(
        &mut self,
        profile: &mut Profile,
        path: &Path,
        function: &str,
        lineno: Option<u32>,
        colno: Option<u32>,
    ) -> Option<FxFrame> {
        let entry = self.ensure_library(profile, path);
        let key = SymbolKey::new(function, lineno, colno);

        let address = if let Some(address) = entry.symbol_map.get(&key) {
            *address
        } else {
            let address = next_symbol_address(&mut entry.next_address, lineno, colno);
            entry.symbol_map.insert(key, address);
            entry.symbols.push(Symbol {
                address,
                size: None,
                name: function.to_string(),
            });
            address
        };

        SymbolRegistry::refresh_symbol_table(profile, entry);

        Some(FxFrame::RelativeAddressFromInstructionPointer(
            entry.handle,
            address,
        ))
    }

    fn ensure_library(&mut self, profile: &mut Profile, path: &Path) -> &mut LibraryEntry {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let display_path = canonical.to_string_lossy().to_string();
        self.libraries.entry(canonical.clone()).or_insert_with(|| {
            let info = LibraryInfo {
                name: display_path.clone(),
                debug_name: display_path.clone(),
                path: display_path.clone(),
                debug_path: display_path,
                debug_id: DebugId::nil(),
                code_id: None,
                arch: None,
                symbol_table: None,
            };
            let handle = profile.add_lib(info);
            LibraryEntry {
                handle,
                symbols: Vec::new(),
                symbol_map: HashMap::new(),
                next_address: 1,
            }
        })
    }

    fn finalize(&mut self, profile: &mut Profile) {
        for entry in self.libraries.values_mut() {
            SymbolRegistry::refresh_symbol_table(profile, entry);
        }
    }

    fn refresh_symbol_table(profile: &mut Profile, entry: &mut LibraryEntry) {
        if entry.symbols.is_empty() {
            return;
        }
        let table = Arc::new(SymbolTable::new(entry.symbols.clone()));
        profile.set_lib_symbol_table(entry.handle, table);
    }
}

struct LibraryEntry {
    handle: LibraryHandle,
    symbols: Vec<Symbol>,
    symbol_map: HashMap<SymbolKey, u32>,
    next_address: u32,
}

#[derive(Hash, Eq, PartialEq)]
struct SymbolKey {
    name: String,
    lineno: Option<u32>,
    colno: Option<u32>,
}

impl SymbolKey {
    fn new(name: &str, lineno: Option<u32>, colno: Option<u32>) -> Self {
        Self {
            name: name.to_string(),
            lineno,
            colno,
        }
    }
}

fn next_symbol_address(next: &mut u32, lineno: Option<u32>, colno: Option<u32>) -> u32 {
    const COLUMN_STRIDE: u32 = 1_000;

    if let Some(line) = lineno {
        let column_component = colno.unwrap_or(0).min(COLUMN_STRIDE - 1);
        match line.checked_mul(COLUMN_STRIDE) {
            Some(base) => base.saturating_add(column_component),
            None => {
                let address = *next;
                *next = (*next).saturating_add(1);
                address
            }
        }
    } else {
        let address = *next;
        *next = (*next).saturating_add(1);
        address
    }
}

fn demangle_name(name: Option<&str>) -> String {
    let Some(name) = name else {
        return String::new();
    };
    let demangled = try_demangle(name)
        .map(|demangled| demangled.to_string())
        .unwrap_or_else(|_| name.to_string());

    strip_rust_hash_suffix(&demangled).to_string()
}

fn strip_rust_hash_suffix(name: &str) -> &str {
    const HASH_PREFIX: &str = "::h";

    match name.rfind(HASH_PREFIX) {
        Some(index) => {
            let hash = &name[index + HASH_PREFIX.len()..];
            let is_hex = !hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit());
            if is_hex { &name[..index] } else { name }
        }
        None => name,
    }
}

fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value as u64).unwrap_or(i64::MAX)
}

#[derive(Debug, Clone, Copy)]
enum CategoryKind {
    Application,
    RustStdLib,
    RustC,
    Dependencies,
    Unknown,
}

struct CategoryHandles {
    application: CategoryPairHandle,
    rust_std_lib: CategoryPairHandle,
    rustc: CategoryPairHandle,
    dependencies: CategoryPairHandle,
    unknown: CategoryPairHandle,
}

impl CategoryHandles {
    fn new(profile: &mut Profile) -> Self {
        let application = profile
            .add_category("Application", CategoryColor::Green)
            .into();
        let rust_std_lib = profile
            .add_category("Rust stdlib", CategoryColor::Blue)
            .into();
        let rustc = profile
            .add_category("Rust compiler", CategoryColor::Orange)
            .into();
        let dependencies = profile
            .add_category("Dependencies", CategoryColor::Purple)
            .into();
        let unknown = CategoryHandle::OTHER.into();

        Self {
            application,
            rust_std_lib,
            rustc,
            dependencies,
            unknown,
        }
    }

    fn get(&self, kind: CategoryKind) -> CategoryPairHandle {
        match kind {
            CategoryKind::Application => self.application,
            CategoryKind::RustStdLib => self.rust_std_lib,
            CategoryKind::RustC => self.rustc,
            CategoryKind::Dependencies => self.dependencies,
            CategoryKind::Unknown => self.unknown,
        }
    }
}

fn determine_category(cwd: &Path, filename: Option<&Path>) -> CategoryKind {
    let Some(path) = filename else {
        return CategoryKind::Unknown;
    };

    let normalized_str = path.to_string_lossy().replace('\\', "/");

    if normalized_str.contains("/rustc/") {
        CategoryKind::RustC
    } else if normalized_str.contains("/rustlib/") {
        CategoryKind::RustStdLib
    } else if normalized_str.contains("cargo/registry/src") {
        CategoryKind::Dependencies
    } else if path.starts_with(cwd) {
        CategoryKind::Application
    } else {
        CategoryKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::stats::{Allocation, FrameInfo};

    #[test]
    fn firefox_profile_contains_frames() {
        let stats = Stats {
            allocations: VecDeque::from([Allocation {
                allocation_size: 128,
                deallocation_size: 0,
                address: 0xdead_beef,
                stack: VecDeque::from([FrameInfo {
                    filename: Some("src/lib.rs".into()),
                    colno: Some(1),
                    lineno: Some(10),
                    fn_address: Some(std::ptr::null_mut()),
                    fn_name: Some("my_function".into()),
                }]),
            }]),
            deallocations: VecDeque::from([Allocation {
                allocation_size: 0,
                deallocation_size: 128,
                address: 0xdead_beef,
                stack: VecDeque::from([FrameInfo {
                    filename: Some("src/lib.rs".into()),
                    colno: Some(1),
                    lineno: Some(20),
                    fn_address: Some(std::ptr::null_mut()),
                    fn_name: Some("drop_my_function".into()),
                }]),
            }]),
        };

        let profile = FirefoxProfile::from_stats(stats).expect("profile generation");
        let json = profile.to_json_string().expect("serialize");
        assert!(json.contains("my_function"));
        assert!(json.contains("drop_my_function"));
    }
}
