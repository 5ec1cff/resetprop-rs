//! `sysprop` — Android system property CLI
//!
//! # Features
//!
//! **Context-routed operations** (automatically routes to the correct context's
//! prop area):
//!
//! ```text
//! sysprop get <KEY>
//! sysprop set <KEY> <VALUE>
//! sysprop del <KEY>
//! sysprop list [--context <CTX>] [--show-context] [--error-output <auto|on|off>]
//! sysprop scan [--context <CTX>] [--objects] [--error-output <auto|on|off>]
//! sysprop compact [--context <CTX>] [--error-output <auto|on|off>]
//! sysprop getcontext <KEY>
//! sysprop dump-context <CONTEXT>
//! sysprop list-contexts [--existing-only]
//! ```
//!
//! **Single prop-area operations** (target one area by context name or path):
//!
//! ```text
//! sysprop area { --context <CTX> | --path <FILE> } get <KEY>
//! sysprop area { --context <CTX> | --path <FILE> } set <KEY> <VALUE>
//! sysprop area { --context <CTX> | --path <FILE> } del <KEY>
//! sysprop area { --context <CTX> | --path <FILE> } list
//! sysprop area { --context <CTX> | --path <FILE> } scan [--objects]
//! ```
//!
//! # Global options
//!
//! | Option | Non-Android | Android |
//! |---|---|---|
//! | `--props-dir` | optional（上下文相关命令需要） | optional (default `/dev/__properties__`) |
//! | `--system-root` | optional | optional (default `/`) |

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process;

use clap::{Args, Parser, Subcommand, ValueEnum};
use memmap2::{Mmap, MmapMut, MmapOptions};

use resetprop_rs::{
    CompactResult, PropArea, PropAreaAllocationScan, PropAreaError, PropAreaObjectKind,
    PropertyContext,
};

// ─────────────────────────────────────────────────────────────────────────────
// CLI definition
// ─────────────────────────────────────────────────────────────────────────────

/// Android system property tool — read, write and inspect prop areas.
#[derive(Parser)]
#[command(name = "sysprop", version, author)]
struct Cli {
    /// Path to /dev/__properties__ or an offline copy of that directory.
    #[cfg(target_os = "android")]
    #[arg(long, default_value = "/dev/__properties__")]
    props_dir: PathBuf,

    /// Path to /dev/__properties__ or an offline copy of that directory.
    #[cfg(not(target_os = "android"))]
    #[arg(long)]
    props_dir: Option<PathBuf>,

    /// System-root path used to locate SELinux property_contexts files when
    /// the storage format is Split (older Android without property_info).
    /// Defaults to `/` on Android; usually needs to be set on non-Android hosts.
    #[arg(long, global = true)]
    system_root: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Get the value of a property (routes through the context system).
    Get {
        /// Property name, e.g. `ro.build.fingerprint`.
        key: String,
    },

    /// Set a property value (routes through the context system).
    ///
    /// The prop area file for the property's SELinux context must already exist.
    Set {
        /// Property name.
        key: String,
        /// New value.
        value: String,
    },

    /// Delete a property (routes through the context system).
    Del {
        /// Property name.
        key: String,
    },

    /// List all properties across every context.
    List {
        /// Only list properties in this SELinux context.
        #[arg(long)]
        context: Option<String>,

        /// Print the SELinux context label next to each property.
        #[arg(long)]
        show_context: bool,

        /// Controls aggregated error output while reading multiple prop areas.
        /// `auto` = disabled on Android targets, enabled elsewhere.
        #[arg(long, value_enum, default_value_t = ErrorOutputMode::Auto)]
        error_output: ErrorOutputMode,
    },

    /// Scan allocation objects/holes across every context.
    Scan {
        /// Only scan this SELinux context.
        #[arg(long)]
        context: Option<String>,

        /// Print detailed object list in addition to holes.
        #[arg(long)]
        objects: bool,

        /// Controls aggregated error output while compacting multiple prop areas.
        /// `auto` = disabled on Android targets, enabled elsewhere.
        #[arg(long, value_enum, default_value_t = ErrorOutputMode::Auto)]
        error_output: ErrorOutputMode,
    },

    /// Compact prop areas across every context.
    Compact {
        /// Only compact this SELinux context.
        #[arg(long)]
        context: Option<String>,

        /// Controls aggregated error output while scanning multiple prop areas.
        /// `auto` = disabled on Android targets, enabled elsewhere.
        #[arg(long, value_enum, default_value_t = ErrorOutputMode::Auto)]
        error_output: ErrorOutputMode,
    },

    /// Print the SELinux context string that owns a property name.
    Getcontext {
        /// Property name.
        key: String,
    },

    /// Dump detailed info about one context's prop area.
    #[command(name = "dump-context")]
    DumpContext {
        /// SELinux context name, e.g. `u:object_r:build_prop:s0`.
        context: String,
    },

    /// List all known SELinux context names.
    #[command(name = "list-contexts")]
    ListContexts {
        /// Only show contexts whose prop area file actually exists on disk.
        #[arg(long)]
        existing_only: bool,
    },

    /// Operate on a single prop area file.
    Area(AreaArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ErrorOutputMode {
    Auto,
    On,
    Off,
}

impl ErrorOutputMode {
    fn enabled(self) -> bool {
        match self {
            Self::Auto => Self::auto_enabled(),
            Self::On => true,
            Self::Off => false,
        }
    }

    #[cfg(target_os = "android")]
    const fn auto_enabled() -> bool {
        false
    }

    #[cfg(not(target_os = "android"))]
    const fn auto_enabled() -> bool {
        true
    }
}

/// Arguments for the `area` subcommand.
#[derive(Args)]
struct AreaArgs {
    /// Select the prop area by SELinux context name.
    /// Exactly one of `--context` or `--path` must be provided.
    #[arg(long)]
    context: Option<String>,

    /// Select the prop area by direct file path.
    /// Exactly one of `--context` or `--path` must be provided.
    #[arg(long)]
    path: Option<PathBuf>,

    #[command(subcommand)]
    command: AreaCommand,
}

#[derive(Subcommand)]
enum AreaCommand {
    /// Get the value of a property from this area.
    Get {
        /// Property name.
        key: String,
    },

    /// Set a property value in this area.
    Set {
        /// Property name.
        key: String,
        /// New value.
        value: String,
    },

    /// Delete a property from this area.
    Del {
        /// Property name.
        key: String,
        /// After deleting, compact the allocation space to reclaim holes.
        #[arg(long)]
        compact: bool,
    },

    /// List all properties in this area.
    List,

    /// Scan allocation objects/holes in this prop area.
    Scan {
        /// Print detailed object list in addition to holes.
        #[arg(long)]
        objects: bool,
    },

    /// Compact the prop area, eliminating holes left by deleted properties.
    Compact,
}

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

type AppError = Box<dyn std::error::Error>;
type AppResult<T> = Result<T, AppError>;

type MmapRoArea = PropArea<MmapCursor<Mmap>>;
type MmapRwArea = PropArea<MmapCursor<MmapMut>>;

// Converting PropAreaError to Box<dyn Error> is automatic via From<E: Error>.
// We add a blanket helper for ergonomic use with ?

fn prop_area_err(e: PropAreaError) -> AppError {
    Box::new(e)
}

fn path_io_err(path: &Path, err: io::Error) -> AppError {
    AppError::from(format!("{}: {err}", path.display()))
}

#[derive(Debug)]
enum OpenAreaDetailedError {
    Io(io::Error),
    Parse(PropAreaError),
}

#[derive(Debug)]
struct MmapCursor<M> {
    map: M,
    pos: usize,
}

impl<M> MmapCursor<M> {
    fn new(map: M) -> Self {
        Self { map, pos: 0 }
    }
}

impl MmapCursor<MmapMut> {
    fn flush(&self) -> io::Result<()> {
        self.map.flush()
    }
}

impl<M: AsRef<[u8]>> Read for MmapCursor<M> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let data = self.map.as_ref();
        if self.pos >= data.len() {
            return Ok(0);
        }

        let count = (data.len() - self.pos).min(buf.len());
        buf[..count].copy_from_slice(&data[self.pos..self.pos + count]);
        self.pos += count;
        Ok(count)
    }
}

impl<M: AsRef<[u8]>> Seek for MmapCursor<M> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let len = self.map.as_ref().len() as i64;
        let current = self.pos as i64;
        let next = match pos {
            SeekFrom::Start(offset) => i64::try_from(offset)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?,
            SeekFrom::End(offset) => len
                .checked_add(offset)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?,
            SeekFrom::Current(offset) => current
                .checked_add(offset)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?,
        };

        if next < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot seek before start of mmap",
            ));
        }

        self.pos = usize::try_from(next)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?;
        Ok(self.pos as u64)
    }
}

impl Write for MmapCursor<MmapMut> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let data = &mut self.map[..];
        if self.pos > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "write past end of mmap",
            ));
        }

        let remaining = data.len() - self.pos;
        if buf.len() > remaining {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "mmap write exceeds mapped region",
            ));
        }

        data[self.pos..self.pos + buf.len()].copy_from_slice(buf);
        self.pos += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.map.flush()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Open a prop area file read-only using a read-only memory map.
fn open_area_ro(path: &Path) -> AppResult<MmapRoArea> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let map = unsafe { MmapOptions::new().map(&f) }
        .map_err(|e| format!("{}: {e}", path.display()))?;
    PropArea::new(MmapCursor::new(map)).map_err(prop_area_err)
}

/// Open a prop area file read-only while preserving whether the failure came
/// from the file open itself or from parsing the prop area contents.
fn open_area_ro_detailed(path: &Path) -> Result<MmapRoArea, OpenAreaDetailedError> {
    let f = File::open(path).map_err(OpenAreaDetailedError::Io)?;
    let map = unsafe { MmapOptions::new().map(&f) }.map_err(OpenAreaDetailedError::Io)?;
    PropArea::new(MmapCursor::new(map)).map_err(OpenAreaDetailedError::Parse)
}

/// Open a prop area file read-write using a shared read-write memory map.
fn open_area_rw(path: &Path) -> AppResult<MmapRwArea> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let map = unsafe { MmapOptions::new().map_mut(&f) }
        .map_err(|e| format!("{}: {e}", path.display()))?;
    PropArea::new(MmapCursor::new(map)).map_err(prop_area_err)
}

/// Open a prop area file read-write while preserving whether the failure came
/// from the file open itself or from parsing the prop area contents.
fn open_area_rw_detailed(path: &Path) -> Result<MmapRwArea, OpenAreaDetailedError> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(OpenAreaDetailedError::Io)?;
    let map = unsafe { MmapOptions::new().map_mut(&f) }.map_err(OpenAreaDetailedError::Io)?;
    PropArea::new(MmapCursor::new(map)).map_err(OpenAreaDetailedError::Parse)
}

fn require_props_dir<'a>(props_dir: Option<&'a Path>) -> AppResult<&'a Path> {
    props_dir.ok_or_else(|| {
        "--props-dir is required for this command (or build for Android to use default /dev/__properties__)"
            .into()
    })
}

/// Load `PropertyContext` from global CLI options.
fn load_context(props_dir: Option<&Path>, system_root: Option<&Path>) -> AppResult<PropertyContext> {
    let props_dir = require_props_dir(props_dir)?;
    PropertyContext::new(props_dir, system_root).map_err(|e| {
        format!(
            "failed to load property context from '{}': {e}",
            props_dir.display()
        )
        .into()
    })
}

/// Validate `AreaArgs` and resolve `(context_label, area_path)`.
///
/// For `--path`, no `PropertyContext` is needed.
/// For `--context`, a property context must be loadable.
fn resolve_area_path(
    args: &AreaArgs,
    props_dir: Option<&Path>,
    system_root: Option<&Path>,
) -> AppResult<(String, PathBuf)> {
    match (&args.context, &args.path) {
        (Some(ctx), None) => {
            let pc = load_context(props_dir, system_root)?;
            let path = pc.context_file_path(ctx);
            Ok((ctx.clone(), path))
        }
        (None, Some(path)) => {
            let label = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Ok((label, path.clone()))
        }
        (None, None) => Err("area: exactly one of --context or --path is required".into()),
        (Some(_), Some(_)) => Err("area: --context and --path are mutually exclusive".into()),
    }
}

fn resolve_context_targets(
    pc: &PropertyContext,
    filter_context: Option<&str>,
) -> io::Result<Vec<(String, PathBuf)>> {
    if let Some(ctx) = filter_context {
        Ok(vec![(ctx.to_string(), pc.context_file_path(ctx))])
    } else {
        pc.prop_area_files()
    }
}

fn format_open_area_error(path: &Path, error: OpenAreaDetailedError) -> String {
    match error {
        OpenAreaDetailedError::Io(err) => format!("{}: {err}", path.display()),
        OpenAreaDetailedError::Parse(err) => format!("{}: {err}", path.display()),
    }
}

fn record_area_open_error(
    path: &Path,
    error: OpenAreaDetailedError,
    specific_context: bool,
    skipped_permission_denied: &mut usize,
    skipped_missing: &mut usize,
    other_errors: &mut Vec<String>,
) -> AppResult<()> {
    if specific_context {
        return Err(format_open_area_error(path, error).into());
    }

    match error {
        OpenAreaDetailedError::Io(err) => match err.kind() {
            io::ErrorKind::PermissionDenied => *skipped_permission_denied += 1,
            io::ErrorKind::NotFound => *skipped_missing += 1,
            _ => other_errors.push(format!("{}: {err}", path.display())),
        },
        OpenAreaDetailedError::Parse(err) => {
            other_errors.push(format!("{}: {err}", path.display()));
        }
    }

    Ok(())
}

fn print_multi_area_error_summary(
    emit_error_output: bool,
    skipped_permission_denied: usize,
    skipped_missing: usize,
    other_errors: &[String],
) {
    if !emit_error_output {
        return;
    }

    if skipped_permission_denied > 0 {
        eprintln!(
            "note: skipped {skipped_permission_denied} prop area(s) due to permission denied"
        );
    }
    if skipped_missing > 0 {
        eprintln!("note: skipped {skipped_missing} prop area(s) that do not exist");
    }
    for message in other_errors.iter().take(3) {
        eprintln!("warning: {message}");
    }
    if other_errors.len() > 3 {
        eprintln!(
            "warning: suppressed {} additional prop area error(s)",
            other_errors.len() - 3
        );
    }
}

fn object_kind_name(kind: PropAreaObjectKind) -> &'static str {
    match kind {
        PropAreaObjectKind::TrieNode => "trie-node",
        PropAreaObjectKind::DirtyBackup => "dirty-backup",
        PropAreaObjectKind::PropInfo => "prop-info",
        PropAreaObjectKind::LongValue => "long-value",
    }
}

fn print_allocation_scan(report: &PropAreaAllocationScan, show_objects: bool) {
    println!("bytes_used={}", report.bytes_used);
    println!("has_dirty_backup={}", report.has_dirty_backup);

    if show_objects {
        println!("objects({}):", report.objects.len());
        for (index, object) in report.objects.iter().enumerate() {
            println!(
                "  [{index:03}] {:<10} off={} size={} aligned={} end={} aligned_end={} detail={}",
                object_kind_name(object.kind),
                object.offset,
                object.size,
                object.aligned_size,
                object.end_offset,
                object.aligned_end_offset,
                object.detail
            );
        }
    }

    println!("holes({}):", report.holes.len());
    if report.holes.is_empty() {
        println!("  (none)");
        return;
    }

    for (index, hole) in report.holes.iter().enumerate() {
        println!(
            "  [{index:03}] start={} end={} size={} aligned={}",
            hole.start_offset,
            hole.end_offset,
            hole.size,
            hole.aligned_size
        );
    }
}

fn cmd_area_scan(area_path: &Path, show_objects: bool) -> AppResult<()> {
    let mut area = open_area_ro(area_path)?;
    let report = area.scan_allocations().map_err(prop_area_err)?;
    print_allocation_scan(&report, show_objects);
    Ok(())
}

fn compact_result_summary(result: &CompactResult) -> String {
    match result {
        CompactResult::NoHoles => "no holes found, area is already fully packed".to_string(),
        CompactResult::AdjustedBytesUsed { old, new } => format!(
            "reclaimed trailing hole — bytes_used {} → {} (freed {})",
            old,
            new,
            old - new,
        ),
        CompactResult::MovedObjects {
            old,
            new,
            objects_moved,
        } => format!(
            "moved {} object(s) — bytes_used {} → {} (freed {})",
            objects_moved,
            old,
            new,
            old - new,
        ),
    }
}

fn cmd_area_compact(area_path: &Path) -> AppResult<()> {
    let mut area = open_area_rw(area_path)?;
    let result = area.compact_allocations().map_err(prop_area_err)?;
    area.into_inner().flush().map_err(|e| path_io_err(area_path, e))?;
    eprintln!("compact: {}", compact_result_summary(&result));
    Ok(())
}

fn cmd_scan(
    props_dir: Option<&Path>,
    system_root: Option<&Path>,
    filter_context: Option<&str>,
    show_objects: bool,
    error_output: ErrorOutputMode,
) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;
    let specific_context = filter_context.is_some();
    let emit_error_output = error_output.enabled();
    let targets = resolve_context_targets(&pc, filter_context)?;

    let mut skipped_permission_denied = 0usize;
    let mut skipped_missing = 0usize;
    let mut other_errors = Vec::new();
    let mut printed_any = false;

    for (ctx_label, path) in &targets {
        let mut area = match open_area_ro_detailed(path) {
            Ok(area) => area,
            Err(error) => {
                record_area_open_error(
                    path,
                    error,
                    specific_context,
                    &mut skipped_permission_denied,
                    &mut skipped_missing,
                    &mut other_errors,
                )?;
                continue;
            }
        };

        let report = match area.scan_allocations() {
            Ok(report) => report,
            Err(err) => {
                let message = format!("{}: {err}", path.display());
                if specific_context {
                    return Err(message.into());
                }
                other_errors.push(message);
                continue;
            }
        };

        if printed_any {
            println!();
        }
        printed_any = true;

        println!("# context: {ctx_label}  |  file: {}", path.display());
        print_allocation_scan(&report, show_objects);
    }

    if !specific_context {
        print_multi_area_error_summary(
            emit_error_output,
            skipped_permission_denied,
            skipped_missing,
            &other_errors,
        );
    }

    Ok(())
}

fn cmd_compact(
    props_dir: Option<&Path>,
    system_root: Option<&Path>,
    filter_context: Option<&str>,
    error_output: ErrorOutputMode,
) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;
    let specific_context = filter_context.is_some();
    let emit_error_output = error_output.enabled();
    let targets = resolve_context_targets(&pc, filter_context)?;

    let mut skipped_permission_denied = 0usize;
    let mut skipped_missing = 0usize;
    let mut other_errors = Vec::new();

    for (ctx_label, path) in &targets {
        let mut area = match open_area_rw_detailed(path) {
            Ok(area) => area,
            Err(error) => {
                record_area_open_error(
                    path,
                    error,
                    specific_context,
                    &mut skipped_permission_denied,
                    &mut skipped_missing,
                    &mut other_errors,
                )?;
                continue;
            }
        };

        let result = match area.compact_allocations() {
            Ok(result) => result,
            Err(err) => {
                let message = format!("{}: {err}", path.display());
                if specific_context {
                    return Err(message.into());
                }
                other_errors.push(message);
                continue;
            }
        };

        if let Err(err) = area.into_inner().flush() {
            if specific_context {
                return Err(path_io_err(path, err));
            }
            other_errors.push(format!("{}: {err}", path.display()));
            continue;
        }

        eprintln!("[{ctx_label}] {}", compact_result_summary(&result));
    }

    if !specific_context {
        print_multi_area_error_summary(
            emit_error_output,
            skipped_permission_denied,
            skipped_missing,
            &other_errors,
        );
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Command implementations
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_get(props_dir: Option<&Path>, system_root: Option<&Path>, key: &str) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;
    let ctx_name = pc.get_context_for_name(key);
    let area_path = pc.context_file_path(ctx_name);

    let mut area = open_area_ro(&area_path)?;
    match area.get_property(key).map_err(prop_area_err)? {
        Some(value) => {
            println!("{value}");
            Ok(())
        }
        None => {
            eprintln!("{key}: property not found");
            process::exit(1)
        }
    }
}

fn cmd_set(
    props_dir: Option<&Path>,
    system_root: Option<&Path>,
    key: &str,
    value: &str,
) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;
    let ctx_name = pc.get_context_for_name(key);
    let area_path = pc.context_file_path(ctx_name);

    let mut area = open_area_rw(&area_path)?;
    area.set_property(key, value).map_err(prop_area_err)?;
    area.into_inner().flush().map_err(|e| path_io_err(&area_path, e))
}

fn cmd_del(props_dir: Option<&Path>, system_root: Option<&Path>, key: &str) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;
    let ctx_name = pc.get_context_for_name(key);
    let area_path = pc.context_file_path(ctx_name);

    let mut area = open_area_rw(&area_path)?;
    let deleted = area.delete_property(key).map_err(prop_area_err)?;
    if !deleted {
        eprintln!("{key}: property not found");
        process::exit(1);
    }
    area.into_inner().flush().map_err(|e| path_io_err(&area_path, e))?;
    Ok(())
}

fn cmd_list(
    props_dir: Option<&Path>,
    system_root: Option<&Path>,
    filter_context: Option<&str>,
    show_context: bool,
    error_output: ErrorOutputMode,
) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;
    let specific_context = filter_context.is_some();
    let emit_error_output = error_output.enabled();

    // Determine which (context_label, file_path) pairs to iterate.
    let targets = resolve_context_targets(&pc, filter_context)?;

    let mut skipped_permission_denied = 0usize;
    let mut skipped_missing = 0usize;
    let mut other_errors = Vec::new();

    for (ctx_label, path) in &targets {
        let mut area = match open_area_ro_detailed(path) {
            Ok(a) => a,
            Err(error) => {
                record_area_open_error(
                    path,
                    error,
                    specific_context,
                    &mut skipped_permission_denied,
                    &mut skipped_missing,
                    &mut other_errors,
                )?;
                continue;
            }
        };
        area.for_each_property(|info| {
            if show_context {
                println!("[{ctx_label}] {}={}", info.name, info.value);
            } else {
                println!("{}={}", info.name, info.value);
            }
        })
        .map_err(prop_area_err)?;
    }

    if !specific_context {
        print_multi_area_error_summary(
            emit_error_output,
            skipped_permission_denied,
            skipped_missing,
            &other_errors,
        );
    }

    Ok(())
}

fn cmd_getcontext(props_dir: Option<&Path>, system_root: Option<&Path>, key: &str) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;
    println!("{}", pc.get_context_for_name(key));
    Ok(())
}

fn cmd_dump_context(
    props_dir: Option<&Path>,
    system_root: Option<&Path>,
    context: &str,
) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;
    let area_path = pc.context_file_path(context);

    println!("context  : {context}");
    println!("type     : {}", pc.context_type());
    println!("file     : {}", area_path.display());

    if !area_path.exists() {
        println!("status   : (file does not exist)");
        return Ok(());
    }

    let mut area = open_area_ro(&area_path)?;
    let area_size = area.area_size();
    let data_size = area.data_size();

    // Collect all properties for count + display.
    let mut props = Vec::new();
    area.for_each_property(|info| props.push(info))
        .map_err(prop_area_err)?;

    println!(
        "area     : {} bytes total, {} bytes used ({:.1}%)",
        area_size,
        data_size,
        data_size as f64 / area_size as f64 * 100.0,
    );
    println!("props    : {}", props.len());
    println!();

    let max_name_len = props.iter().map(|p| p.name.len()).max().unwrap_or(0);
    for p in &props {
        let tag = if p.is_long { " [long]" } else { "" };
        println!(
            "  {:<width$}  = {}{}",
            p.name,
            p.value,
            tag,
            width = max_name_len,
        );
    }
    Ok(())
}

fn cmd_list_contexts(
    props_dir: Option<&Path>,
    system_root: Option<&Path>,
    existing_only: bool,
) -> AppResult<()> {
    let pc = load_context(props_dir, system_root)?;

    // Only use contexts known to the parser; do not enumerate props dir,
    // because low-privilege Android users may not be allowed to read it.
    let all: BTreeSet<String> = pc
        .list_all_contexts()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let width = all.iter().map(|s| s.len()).max().unwrap_or(0);
    for name in &all {
        let exists = pc.context_file_path(name).exists();
        if existing_only && !exists {
            continue;
        }
        let marker = if exists { "[+]" } else { "[ ]" };
        println!("{marker}  {:<width$}", name, width = width);
    }
    Ok(())
}

fn cmd_area(props_dir: Option<&Path>, system_root: Option<&Path>, args: &AreaArgs) -> AppResult<()> {
    let (ctx_label, area_path) = resolve_area_path(args, props_dir, system_root)?;

    match &args.command {
        AreaCommand::Get { key } => {
            let mut area = open_area_ro(&area_path)?;
            match area.get_property(key).map_err(prop_area_err)? {
                Some(value) => println!("{value}"),
                None => {
                    eprintln!("{key}: property not found");
                    process::exit(1);
                }
            }
        }

        AreaCommand::Set { key, value } => {
            let mut area = open_area_rw(&area_path)?;
            area.set_property(key, value).map_err(prop_area_err)?;
            area.into_inner().flush().map_err(|e| path_io_err(&area_path, e))?;
        }

        AreaCommand::Del { key, compact } => {
            let mut area = open_area_rw(&area_path)?;
            let deleted = area.delete_property(key).map_err(prop_area_err)?;
            if !deleted {
                eprintln!("{key}: property not found");
                process::exit(1);
            }
            if *compact {
                area.compact_allocations().map_err(prop_area_err)?;
            }
            area.into_inner().flush().map_err(|e| path_io_err(&area_path, e))?;
        }

        AreaCommand::List => {
            let area_size;
            let data_size;
            let mut props = Vec::new();
            {
                let mut area = open_area_ro(&area_path)?;
                area_size = area.area_size();
                data_size = area.data_size();
                area.for_each_property(|info| props.push(info))
                    .map_err(prop_area_err)?;
            }
            // Header
            eprintln!(
                "# context: {ctx_label}  |  file: {}  |  {} props  |  {}/{} bytes used",
                area_path.display(),
                props.len(),
                data_size,
                area_size,
            );
            for p in &props {
                let tag = if p.is_long { " [long]" } else { "" };
                println!("{}={}{}", p.name, p.value, tag);
            }
        }

        AreaCommand::Scan { objects } => {
            cmd_area_scan(&area_path, *objects)?;
        }

        AreaCommand::Compact => {
            cmd_area_compact(&area_path)?;
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

fn run() -> AppResult<()> {
    let cli = Cli::parse();

    #[cfg(target_os = "android")]
    let props_dir: Option<&Path> = Some(cli.props_dir.as_path());

    #[cfg(not(target_os = "android"))]
    let props_dir: Option<&Path> = cli.props_dir.as_deref();

    let system_root: Option<&Path> = cli.system_root.as_deref();

    match &cli.command {
        Commands::Get { key } => cmd_get(props_dir, system_root, key),
        Commands::Set { key, value } => cmd_set(props_dir, system_root, key, value),
        Commands::Del { key } => cmd_del(props_dir, system_root, key),
        Commands::List {
            context,
            show_context,
            error_output,
        } => cmd_list(
            props_dir,
            system_root,
            context.as_deref(),
            *show_context,
            *error_output,
        ),
        Commands::Scan {
            context,
            objects,
            error_output,
        } => cmd_scan(
            props_dir,
            system_root,
            context.as_deref(),
            *objects,
            *error_output,
        ),
        Commands::Compact {
            context,
            error_output,
        } => cmd_compact(props_dir, system_root, context.as_deref(), *error_output),
        Commands::Getcontext { key } => cmd_getcontext(props_dir, system_root, key),
        Commands::DumpContext { context } => cmd_dump_context(props_dir, system_root, context),
        Commands::ListContexts { existing_only } => {
            cmd_list_contexts(props_dir, system_root, *existing_only)
        }
        Commands::Area(area_args) => cmd_area(props_dir, system_root, area_args),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
