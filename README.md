# resetprop-rs

[简体中文](README.zh-CN.md)

`resetprop-rs` is a Rust toolkit for Android system property storage.

It can:

- parse raw Android `prop_area` files
- read, write, update, and delete properties
- resolve property names to SELinux contexts through Android `property_contexts`
- inspect allocation layout, holes, and dirty-backup regions
- compact holes left behind by deletions
- read and write Android persistent property files (`persistent_properties` protobuf format)
- run as both a reusable library and a practical CLI

Despite the name, this is **not** a wrapper around Magisk's `resetprop`. It directly understands Android's property-area data structures and context metadata.

## Why this project exists

Android's native property implementation is powerful, but it is not very convenient when the goal is:

- offline inspection of copied `/dev/__properties__` data
- host-side debugging on Windows/Linux/macOS
- writing tests against real or synthetic property areas
- building small custom tools without pulling in Android-specific runtime dependencies

`resetprop-rs` keeps compatibility with the underlying layout while exposing the behavior as a focused Rust crate and CLI.

## Advantages

### 1. Safer implementation language

The project is written in Rust rather than C/C++. That does not magically remove every bug, but it does reduce entire classes of memory-safety problems that are common in binary parsers and in-place editors.

For a tool that reads and mutates low-level binary structures, that is a practical advantage.

### 2. Works off-device

The property-context parser uses plain file I/O and can operate on copied Android filesystem data.

That means the project is useful even when the device is not rooted, not connected, or not available at all. It fits well into reverse-engineering workflows, ROM bring-up, CI, regression testing, and forensic/offline analysis.

### 3. Better observability than typical property tools

Most property tools focus on `get` and `set`.

This project also exposes the internal allocation state:

- `scan` shows live objects, holes, and dirty-backup presence
- allocation objects are typed (`trie-node`, `prop-info`, `long-value`, `dirty-backup`)
- `compact` can reclaim space after deletions

That makes it easier to debug fragmentation, validate assumptions about the on-disk layout, and inspect how updates actually affect the property area.

### 4. Practical mutation support

The crate does not stop at parsing:

- update inline values in place
- update long values in place when possible
- delete properties
- compact freed holes after deletion

This is useful for fixture generation, controlled experiments, and offline modification of copied property-area files.

### 5. Context-aware routing

The main `sysprop` CLI can route a property name to the correct prop-area file by reading Android property-context data.

Supported context storage modes:

- **Serialized**
- **Split**
- **PreSplit**

This mirrors the major Android property-context layouts and avoids hard-coding a single storage model.

### 6. Designed for low-privilege and host-side inspection

When enumerating prop-area files, the project prefers known context metadata over directory enumeration where possible. That is helpful on Android systems where low-privilege users may not be allowed to list `/dev/__properties__` directly.

### 7. Useful as both a library and a tool

The repository includes:

- a reusable Rust library
- `sysprop`: the main context-aware CLI
- `read_props`: a minimal raw prop-area reader
- `write_props`: a minimal raw prop-area writer
- `cargo_android_sysprop`: helper for building and pushing `sysprop` to Android via `cargo ndk` + `adb`

### 8. Tested against synthetic and fixture-based cases

The test suite covers:

- short and long properties
- read/write/delete behavior
- in-place update rules
- allocation scanning
- dirty-backup detection
- compaction after deletion
- fixture-based reads/writes against a sample prop-area file

## Project layout

- `src/lib.rs` — public library exports
- `src/prop_area.rs` — low-level prop-area parsing, editing, scanning, compaction
- `src/property_context.rs` — Android property-context parsing and context resolution
- `src/persistent_prop.rs` — persistent property protobuf CRUD (no libc, pure Rust stdlib)
- `src/bin/sysprop.rs` — main CLI
- `src/bin/read_props.rs` — simple raw reader
- `src/bin/write_props.rs` — simple raw writer
- `src/bin/cargo_android_sysprop.rs` — Android build/deploy helper
- `tests/` — integration tests and fixtures

## Build

```bash
cargo build
cargo test
```

Build only the main CLI:

```bash
cargo build --bin sysprop
```

## Main CLI: `sysprop`

Show help:

```bash
cargo run --bin sysprop -- --help
```

### Context-routed operations

These commands resolve the correct prop-area file from Android property-context metadata:

```bash
cargo run --bin sysprop -- --props-dir <PROPS_DIR> get ro.build.fingerprint
cargo run --bin sysprop -- --props-dir <PROPS_DIR> set persist.sys.locale en-US
cargo run --bin sysprop -- --props-dir <PROPS_DIR> del persist.sys.locale
cargo run --bin sysprop -- --props-dir <PROPS_DIR> list --show-context
cargo run --bin sysprop -- --props-dir <PROPS_DIR> scan --objects
cargo run --bin sysprop -- --props-dir <PROPS_DIR> compact
```

#### `--persistent` flag (Android only)

`set` and `del` support a `--persistent` flag that writes to **both** the prop area and
`/data/property/persistent_properties`, so the change survives a reboot:

```bash
# Write to prop area AND persist across reboots
sysprop --props-dir <PROPS_DIR> set persist.sys.locale en-US --persistent
# Delete from prop area AND remove from persistent storage
sysprop --props-dir <PROPS_DIR> del persist.sys.locale --persistent
```

`get` and `list` also accept `--persistent`; in that mode they read directly from
`/data/property/persistent_properties` instead of the prop area.

If you only want one context:

```bash
cargo run --bin sysprop -- --props-dir <PROPS_DIR> scan --context u:object_r:build_prop:s0 --objects
cargo run --bin sysprop -- --props-dir <PROPS_DIR> compact --context u:object_r:build_prop:s0
```

On non-Android hosts, `--system-root <ANDROID_ROOT>` may also be needed when the context storage format is `Split`.

### Persistent property file operations

The `persistent-file` subcommand operates on an Android `persistent_properties` protobuf file
directly. It works on any platform; on non-Android hosts `--path` is required.

```bash
# List all persistent properties
cargo run --bin sysprop -- persistent-file --path /data/property/persistent_properties list

# Get a single property
cargo run --bin sysprop -- persistent-file --path /data/property/persistent_properties get persist.sys.locale

# Set a property (atomic write, survives reboot)
cargo run --bin sysprop -- persistent-file --path /data/property/persistent_properties set persist.sys.locale en-US

# Delete a property
cargo run --bin sysprop -- persistent-file --path /data/property/persistent_properties del persist.sys.locale
```

On Android the path defaults to `/data/property/persistent_properties` and may be omitted:

```bash
sysprop persistent-file list
sysprop persistent-file get persist.sys.locale
```

### Single-area operations

These commands target one specific prop-area file directly:

```bash
cargo run --bin sysprop -- area --path tests/fixtures/sample_props.prop list
cargo run --bin sysprop -- area --path tests/fixtures/sample_props.prop scan --objects
cargo run --bin sysprop -- area --path tests/fixtures/sample_props.prop compact
```

You can also select an area by context name:

```bash
cargo run --bin sysprop -- --props-dir <PROPS_DIR> area --context u:object_r:build_prop:s0 scan --objects
```

## Minimal tools

### Read a raw prop-area file

```bash
cargo run --bin read_props -- tests/fixtures/sample_props.prop
cargo run --bin read_props -- tests/fixtures/sample_props.prop ro.product.locale
```

### Write a raw prop-area file

```bash
cargo run --bin write_props -- tests/fixtures/sample_props.prop ro.product.locale=en-US
```

## Deploy `sysprop` to Android

If `cargo ndk` and `adb` are available:

```bash
cargo run --bin cargo_android_sysprop -- --target aarch64-linux-android --profile release
```

The helper builds `sysprop`, pushes it to the device, and marks it executable.

## Library usage

```rust
use std::fs::File;
use resetprop_rs::PropArea;

let file = File::open("tests/fixtures/sample_props.prop")?;
let mut area = PropArea::new(file)?;

if let Some(info) = area.get_property_info("ro.product.locale")? {
    println!("{} = {}", info.name, info.value);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Scope and non-goals

This project focuses on understanding and manipulating Android property-area data.

It is **not** a full replacement for Android's property service, and it does not try to emulate every runtime behavior of init, SELinux policy enforcement, or the complete Android property stack.

## Current status

The project is already useful for inspection, experimentation, and tooling, especially when the main goal is visibility and control over raw property-area data.

If the problem is "understand what is in this property area and change it safely enough for offline or controlled workflows", this repository is aimed directly at that use case.
