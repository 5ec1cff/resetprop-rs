use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use resetprop_rs::{PropArea, PROP_VALUE_MAX};

struct TempFixture {
    path: PathBuf,
}

impl TempFixture {
    fn new() -> Self {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("sample_props.prop");
        assert!(
            fixture.exists(),
            "fixture file not found: {}",
            fixture.display()
        );

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "prop2-write-test-{}-{}.hex",
            std::process::id(),
            nonce
        ));

        fs::copy(&fixture, &path)
            .unwrap_or_else(|err| panic!("failed to copy fixture '{}': {}", fixture.display(), err));

        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn open_area_rw(path: &Path) -> PropArea<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|err| panic!("cannot open '{}' for read/write: {}", path.display(), err));
    PropArea::new(file)
        .unwrap_or_else(|err| panic!("cannot parse prop area '{}': {}", path.display(), err))
}

fn open_area_ro(path: &Path) -> PropArea<File> {
    let file = File::open(path)
        .unwrap_or_else(|err| panic!("cannot open '{}' for read: {}", path.display(), err));
    PropArea::new(file)
        .unwrap_or_else(|err| panic!("cannot parse prop area '{}': {}", path.display(), err))
}

#[test]
fn write_selected_short_properties_from_fixture() {
    let fixture = TempFixture::new();

    {
        let mut area = open_area_rw(fixture.path());
        area.set_property("ro.product.locale", "en-US").unwrap();
        area.set_property("ro.secureboot.lockstate", "locked").unwrap();
        area.set_property("persist.timed.enable", "false").unwrap();

        area.into_inner().sync_all().unwrap();
    }

    let mut area = open_area_ro(fixture.path());

    for (key, expected) in [
        ("ro.product.locale", "en-US"),
        ("ro.secureboot.lockstate", "locked"),
        ("persist.timed.enable", "false"),
    ] {
        let info = area
            .get_property_info(key)
            .unwrap_or_else(|err| panic!("lookup '{}' failed: {}", key, err))
            .unwrap_or_else(|| panic!("property '{}' not found", key));

        assert_eq!(info.value, expected, "unexpected value for '{}':", key);
        assert!(!info.is_long, "'{}' should stay inline", key);
    }
}

#[test]
fn write_existing_long_property_from_fixture() {
    let fixture = TempFixture::new();

    let long_value = "Alpha,Beta,Gamma,Delta,Epsilon,Zeta,Eta,Theta,Iota,Kappa,Lambda,Mu,Nu,Xi,Omicron,Pi,Rho,Sigma,Tau,Upsilon,Phi,Chi,Psi,Omega";
    assert!(long_value.len() >= PROP_VALUE_MAX);

    {
        let mut area = open_area_rw(fixture.path());
        area.set_property("ro.build.version.known_codenames", long_value)
            .unwrap();
        area.into_inner().sync_all().unwrap();
    }

    let mut area = open_area_ro(fixture.path());
    let info = area
        .get_property_info("ro.build.version.known_codenames")
        .unwrap()
        .unwrap();

    assert_eq!(info.value, long_value);
    assert!(info.is_long);
}
