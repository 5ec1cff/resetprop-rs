//! Integration test against a real prop_area file.
//!
//! Set the environment variable `PROP_AREA_FILE` to the path of the file to test.
//! If the variable is absent the test is skipped automatically.
//!
//! Run with:
//!   $env:PROP_AREA_FILE = "C:\path\to\properties_serial"; cargo test --test file_read -- --nocapture
//!
//! The expected key-value pairs below are placeholders; replace them with the
//! real keys/values once you provide the file.

use std::fs::File;

use resetprop_rs::PropArea;

// ── Expected values ────────────────────────────────────────────────────────────
// Replace these with real key-value pairs from your prop area file.
const EXPECTED: &[(&str, &str)] = &[
    // ("ro.build.version.sdk",      "34"),
    // ("ro.product.model",          "Pixel 6"),
    // ("persist.sys.locale",        "en-US"),
];

// ── Helpers ────────────────────────────────────────────────────────────────────

fn open_area() -> Option<PropArea<File>> {
    let path = std::env::var("PROP_AREA_FILE").ok()?;
    let file = File::open(&path)
        .unwrap_or_else(|err| panic!("Cannot open '{}': {}", path, err));
    Some(
        PropArea::new(file)
            .unwrap_or_else(|err| panic!("Failed to parse prop area '{}': {}", path, err)),
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[test]
fn dump_all_properties() {
    let Some(mut area) = open_area() else {
        eprintln!("PROP_AREA_FILE not set – skipping file_read tests");
        return;
    };

    let mut props: Vec<(String, String)> = Vec::new();
    area.for_each_property(|info| props.push((info.name, info.value)))
        .expect("for_each_property failed");

    props.sort_by(|a, b| a.0.cmp(&b.0));
    println!("=== {} properties ===", props.len());
    for (k, v) in &props {
        println!("{}={}", k, v);
    }

    assert!(!props.is_empty(), "prop area appears to be empty");
}

#[test]
fn lookup_expected_keys() {
    let Some(mut area) = open_area() else {
        eprintln!("PROP_AREA_FILE not set – skipping file_read tests");
        return;
    };

    for (key, expected_value) in EXPECTED {
        let info = area
            .get_property_info(key)
            .unwrap_or_else(|err| panic!("error looking up '{}': {}", key, err));

        let info = info.unwrap_or_else(|| panic!("property '{}' not found", key));
        assert_eq!(
            &info.value, expected_value,
            "key '{}': expected {:?}, got {:?}",
            key, expected_value, info.value
        );
        println!(
            "ok  {} = {} [{}]",
            key,
            info.value,
            if info.is_long { "long" } else { "inline" }
        );
    }
}
