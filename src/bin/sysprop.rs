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
use std::path::{Path, PathBuf};
use std::process;

use clap::{Args, Parser, Subcommand, ValueEnum};

use resetprop_rs::{PropArea, PropAreaError, PropertyContext};

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
    },

    /// List all properties in this area.
    List,
}

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

type AppError = Box<dyn std::error::Error>;
type AppResult<T> = Result<T, AppError>;

// Converting PropAreaError to Box<dyn Error> is automatic via From<E: Error>.
// We add a blanket helper for ergonomic use with ?

fn prop_area_err(e: PropAreaError) -> AppError {
    Box::new(e)
}

#[derive(Debug)]
enum OpenAreaRoDetailedError {
    Io(io::Error),
    Parse(PropAreaError),
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Open a prop area file read-only.
fn open_area_ro(path: &Path) -> AppResult<PropArea<File>> {
    let f = File::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    PropArea::new(f).map_err(prop_area_err)
}

/// Open a prop area file read-only while preserving whether the failure came
/// from the file open itself or from parsing the prop area contents.
fn open_area_ro_detailed(path: &Path) -> Result<PropArea<File>, OpenAreaRoDetailedError> {
    let f = File::open(path).map_err(OpenAreaRoDetailedError::Io)?;
    PropArea::new(f).map_err(OpenAreaRoDetailedError::Parse)
}

/// Open a prop area file read-write.
fn open_area_rw(path: &Path) -> AppResult<PropArea<File>> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    PropArea::new(f).map_err(prop_area_err)
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
    area.set_property(key, value).map_err(prop_area_err)
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
    let targets: Vec<(String, PathBuf)> = if let Some(ctx) = filter_context {
        vec![(ctx.to_string(), pc.context_file_path(ctx))]
    } else {
        pc.prop_area_files()?
    };

    let mut skipped_permission_denied = 0usize;
    let mut skipped_missing = 0usize;
    let mut other_errors = Vec::new();

    for (ctx_label, path) in &targets {
        let mut area = match open_area_ro_detailed(path) {
            Ok(a) => a,
            Err(e) => {
                if specific_context {
                    let message = match e {
                        OpenAreaRoDetailedError::Io(err) => {
                            format!("{}: {err}", path.display())
                        }
                        OpenAreaRoDetailedError::Parse(err) => {
                            format!("{}: {err}", path.display())
                        }
                    };
                    return Err(message.into());
                }

                match e {
                    OpenAreaRoDetailedError::Io(err) => match err.kind() {
                        io::ErrorKind::PermissionDenied => skipped_permission_denied += 1,
                        io::ErrorKind::NotFound => skipped_missing += 1,
                        _ => other_errors.push(format!("{}: {err}", path.display())),
                    },
                    OpenAreaRoDetailedError::Parse(err) => {
                        other_errors.push(format!("{}: {err}", path.display()));
                    }
                }
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

    if !specific_context && emit_error_output {
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
        }

        AreaCommand::Del { key } => {
            let mut area = open_area_rw(&area_path)?;
            let deleted = area.delete_property(key).map_err(prop_area_err)?;
            if !deleted {
                eprintln!("{key}: property not found");
                process::exit(1);
            }
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
