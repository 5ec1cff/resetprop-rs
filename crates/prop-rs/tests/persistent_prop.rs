use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use prop_rs::PersistentPropertyFile;

fn unique_temp_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "resetprop-rs-{name}-{}-{nonce}.pb",
        std::process::id()
    ))
}

#[test]
fn persistent_props_set_get_delete_and_sorted_iteration() {
    let mut props = PersistentPropertyFile::default();
    props.set("persist.z", "3");
    props.set("persist.a", "1");
    props.set("persist.m", "2");

    assert_eq!(props.get("persist.a"), Some("1"));
    assert_eq!(props.get("persist.m"), Some("2"));
    assert_eq!(props.get("persist.z"), Some("3"));

    // Update existing key.
    props.set("persist.m", "updated");
    assert_eq!(props.get("persist.m"), Some("updated"));

    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["persist.a", "persist.m", "persist.z"]);

    assert!(props.delete("persist.a"));
    assert!(!props.delete("persist.unknown"));
    assert_eq!(props.get("persist.a"), None);
}

#[test]
fn persistent_props_binary_round_trip() {
    let mut props = PersistentPropertyFile::default();
    props.set("persist.locale", "en-US");
    props.set("persist.demo", "1");

    let bytes = props.to_bytes().expect("encode failed");
    let decoded = PersistentPropertyFile::from_bytes(&bytes).expect("decode failed");

    assert_eq!(decoded.get("persist.locale"), Some("en-US"));
    assert_eq!(decoded.get("persist.demo"), Some("1"));
}

#[test]
fn persistent_props_write_and_load_from_file() {
    let path = unique_temp_path("persistent-props");

    let mut props = PersistentPropertyFile::default();
    props.set("persist.one", "v1");
    props.set("persist.two", "v2");
    props.write_to_path(&path).expect("write failed");

    let loaded = PersistentPropertyFile::load(&path).expect("load failed");
    assert_eq!(loaded.get("persist.one"), Some("v1"));
    assert_eq!(loaded.get("persist.two"), Some("v2"));

    std::fs::remove_file(&path).expect("cleanup failed");
}

#[cfg(unix)]
#[test]
fn persistent_props_write_replaces_file_with_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let path = unique_temp_path("file-mode");
    std::fs::write(&path, b"").expect("create failed");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod failed");

    let mut props = PersistentPropertyFile::default();
    props.set("persist.one", "v1");
    props.write_to_path(&path).expect("write failed");

    let mode = std::fs::metadata(&path)
        .expect("stat failed")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    std::fs::remove_file(&path).expect("cleanup failed");
}

#[test]
fn persistent_props_load_or_default_on_missing_file() {
    let path = unique_temp_path("missing");
    let loaded = PersistentPropertyFile::load_or_default(&path).expect("load_or_default failed");
    assert!(loaded.is_empty());
}
