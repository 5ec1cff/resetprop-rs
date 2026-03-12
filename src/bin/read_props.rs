//! A small CLI for reading a raw `prop_area` file.
//!
//! Usage:
//!   read_props <file>           – dump all properties
//!   read_props <file> <key>     – look up a single property

use std::fs::File;
use std::process::exit;

use resetprop_rs::{PropArea, PropAreaError};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <prop_area_file> [key]", args[0]);
        exit(1);
    }

    let path = &args[1];
    let file = File::open(path).unwrap_or_else(|err| {
        eprintln!("Cannot open '{}': {}", path, err);
        exit(1);
    });

    let mut area = PropArea::new(file).unwrap_or_else(|err| {
        eprintln!("Failed to parse prop area '{}': {}", path, err);
        exit(1);
    });

    if args.len() >= 3 {
        // Single-key lookup
        let key = &args[2];
        match area.get_property_info(key) {
            Ok(Some(info)) => {
                println!("[{}]", if info.is_long { "long" } else { "inline" });
                println!("  name  = {}", info.name);
                println!("  value = {}", info.value);
            }
            Ok(None) => {
                eprintln!("Property '{}' not found.", key);
                exit(2);
            }
            Err(PropAreaError::InvalidKey(k)) => {
                eprintln!("Invalid key: {}", k);
                exit(1);
            }
            Err(err) => {
                eprintln!("Error: {}", err);
                exit(1);
            }
        }
    } else {
        // Dump all
        let mut props: Vec<(String, String)> = Vec::new();
        area.for_each_property(|info| {
            props.push((info.name, info.value));
        })
        .unwrap_or_else(|err| {
            eprintln!("Error while iterating: {}", err);
            exit(1);
        });

        props.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in &props {
            println!("{}={}", k, v);
        }
        eprintln!("({} properties)", props.len());
    }
}
