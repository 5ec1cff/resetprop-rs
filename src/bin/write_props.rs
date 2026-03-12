//! A small CLI for writing key-value pairs into a raw `prop_area` file.
//!
//! Usage:
//!   write_props <file> <key=value> [<key=value> ...]

use std::fs::OpenOptions;
use std::process::exit;

use resetprop_rs::PropArea;

fn parse_assignment(input: &str) -> Result<(&str, &str), &'static str> {
    let Some((key, value)) = input.split_once('=') else {
        return Err("assignment must be in key=value form");
    };

    if key.is_empty() {
        return Err("key must not be empty");
    }

    Ok((key, value))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <prop_area_file> <key=value> [<key=value> ...]", args[0]);
        exit(1);
    }

    let path = &args[1];
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|err| {
            eprintln!("Cannot open '{}' for read/write: {}", path, err);
            exit(1);
        });

    let mut area = PropArea::new(file).unwrap_or_else(|err| {
        eprintln!("Failed to parse prop area '{}': {}", path, err);
        exit(1);
    });

    for assignment in &args[2..] {
        let (key, value) = parse_assignment(assignment).unwrap_or_else(|err| {
            eprintln!("Invalid assignment '{}': {}", assignment, err);
            exit(1);
        });

        area.set_property(key, value).unwrap_or_else(|err| {
            eprintln!("Failed to set '{}': {}", key, err);
            exit(1);
        });

        let info = area
            .get_property_info(key)
            .unwrap_or_else(|err| {
                eprintln!("Failed to read back '{}': {}", key, err);
                exit(1);
            })
            .unwrap_or_else(|| {
                eprintln!("Property '{}' disappeared right after write", key);
                exit(1);
            });

        println!(
            "[{}] {}={}",
            if info.is_long { "long" } else { "inline" },
            info.name,
            info.value
        );
    }

    let file = area.into_inner();
    file.sync_all().unwrap_or_else(|err| {
        eprintln!("Failed to sync '{}': {}", path, err);
        exit(1);
    });
}
