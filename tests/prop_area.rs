use std::io::Cursor;

use resetprop_rs::{PropArea, PropAreaError, PROP_AREA_HEADER_SIZE, PROP_VALUE_MAX};

fn new_area(size: usize) -> PropArea<Cursor<Vec<u8>>> {
    PropArea::create(Cursor::new(vec![0; size]), size as u64).unwrap()
}

fn data_abs(offset: u32) -> usize {
    PROP_AREA_HEADER_SIZE as usize + offset as usize
}

fn read_serial(raw: &[u8], prop_offset: u32) -> u32 {
    let abs = data_abs(prop_offset);
    u32::from_le_bytes(raw[abs..abs + 4].try_into().unwrap())
}

#[test]
fn add_get_update_and_delete_short_property() {
    let mut area = new_area(4096);

    assert_eq!(area.get_property("ro.secure").unwrap(), None);

    area.set_property("ro.secure", "1").unwrap();
    assert_eq!(area.get_property("ro.secure").unwrap(), Some("1".to_owned()));

    area.set_property("ro.secure", "0").unwrap();
    let info = area.get_property_info("ro.secure").unwrap().unwrap();
    assert_eq!(info.name, "ro.secure");
    assert_eq!(info.value, "0");
    assert!(!info.is_long);

    assert!(area.delete_property("ro.secure").unwrap());
    assert_eq!(area.get_property("ro.secure").unwrap(), None);
}

#[test]
fn long_property_round_trip_and_update() {
    let mut area = new_area(8192);
    let long_value = "x".repeat(140);

    area.set_property("persist.sys.long", &long_value).unwrap();

    let info = area.get_property_info("persist.sys.long").unwrap().unwrap();
    assert_eq!(info.value, long_value);
    assert!(info.is_long);

    area.set_property("persist.sys.long", "short").unwrap();
    let updated = area.get_property_info("persist.sys.long").unwrap().unwrap();
    assert_eq!(updated.value, "short");
    assert!(updated.is_long);
}

#[test]
fn foreach_reports_all_properties() {
    let mut area = new_area(8192);
    area.set_property("ro.secure", "1").unwrap();
    area.set_property("persist.sys.locale", "en-US").unwrap();
    area.set_property("persist.sys.timezone", "UTC").unwrap();

    let mut props = Vec::new();
    area.for_each_property(|info| props.push((info.name, info.value)))
        .unwrap();

    props.sort();
    assert_eq!(
        props,
        vec![
            ("persist.sys.locale".to_owned(), "en-US".to_owned()),
            ("persist.sys.timezone".to_owned(), "UTC".to_owned()),
            ("ro.secure".to_owned(), "1".to_owned()),
        ]
    );
}

#[test]
fn delete_prunes_only_removed_branch() {
    let mut area = new_area(8192);
    area.set_property("persist.sys.locale", "en-US").unwrap();
    area.set_property("persist.sys.timezone", "UTC").unwrap();

    assert!(area.delete_property("persist.sys.locale").unwrap());
    assert_eq!(area.get_property("persist.sys.locale").unwrap(), None);
    assert_eq!(
        area.get_property("persist.sys.timezone").unwrap(),
        Some("UTC".to_owned())
    );
}

#[test]
fn update_inline_keeps_serial_and_clears_trailing_bytes() {
    let key = "ro.inline.serial";
    let old_value = "ABCDEFGHIJ";
    let new_value = "abc";

    let mut area = new_area(4096);
    area.set_property(key, old_value).unwrap();
    let before = area.get_property_info(key).unwrap().unwrap();
    assert!(!before.is_long);

    let raw_before = area.into_inner().into_inner();
    let serial_before = read_serial(&raw_before, before.prop_offset);

    let mut area = PropArea::new(Cursor::new(raw_before)).unwrap();
    area.set_property(key, new_value).unwrap();
    let after = area.get_property_info(key).unwrap().unwrap();
    assert_eq!(after.prop_offset, before.prop_offset);
    assert_eq!(after.value_offset, before.value_offset);

    let raw_after = area.into_inner().into_inner();
    let serial_after = read_serial(&raw_after, after.prop_offset);
    assert_eq!(serial_after, serial_before);

    let value_abs = data_abs(after.value_offset);
    assert_eq!(&raw_after[value_abs..value_abs + new_value.len()], new_value.as_bytes());
    assert_eq!(raw_after[value_abs + new_value.len()], 0);
    assert!(
        raw_after[value_abs + new_value.len() + 1..value_abs + PROP_VALUE_MAX]
            .iter()
            .all(|&b| b == 0)
    );
}

#[test]
fn update_long_keeps_serial_and_updates_in_place() {
    let key = "persist.sys.long.serial";
    let old_value = "x".repeat(140);
    let new_value = "y".repeat(40);

    let mut area = new_area(16384);
    area.set_property(key, &old_value).unwrap();
    let before = area.get_property_info(key).unwrap().unwrap();
    assert!(before.is_long);

    let raw_before = area.into_inner().into_inner();
    let serial_before = read_serial(&raw_before, before.prop_offset);

    let mut area = PropArea::new(Cursor::new(raw_before)).unwrap();
    area.set_property(key, &new_value).unwrap();
    let after = area.get_property_info(key).unwrap().unwrap();
    assert!(after.is_long);
    assert_eq!(after.value_offset, before.value_offset);

    let raw_after = area.into_inner().into_inner();
    let serial_after = read_serial(&raw_after, after.prop_offset);
    assert_eq!(serial_after, serial_before);

    let value_abs = data_abs(after.value_offset);
    assert_eq!(&raw_after[value_abs..value_abs + new_value.len()], new_value.as_bytes());
    assert_eq!(raw_after[value_abs + new_value.len()], 0);
    assert!(
        raw_after[value_abs + new_value.len() + 1..value_abs + old_value.len() + 1]
            .iter()
            .all(|&b| b == 0)
    );
}

#[test]
fn update_long_rejects_growth_without_reallocation() {
    let key = "persist.sys.long.noalloc";
    let old_value = "x".repeat(120);
    let new_value = "y".repeat(121);

    let mut area = new_area(16384);
    area.set_property(key, &old_value).unwrap();
    let before = area.get_property_info(key).unwrap().unwrap();

    let err = area.set_property(key, &new_value).unwrap_err();
    assert!(matches!(err, PropAreaError::InPlaceUpdateTooLong { .. }));

    let after = area.get_property_info(key).unwrap().unwrap();
    assert_eq!(after.value, old_value);
    assert_eq!(after.value_offset, before.value_offset);
}

#[test]
fn update_inline_rejects_conversion_to_long() {
    let key = "ro.inline.noalloc";
    let old_value = "inline";
    let new_value = "z".repeat(PROP_VALUE_MAX);

    let mut area = new_area(8192);
    area.set_property(key, old_value).unwrap();
    let before = area.get_property_info(key).unwrap().unwrap();
    assert!(!before.is_long);

    let err = area.set_property(key, &new_value).unwrap_err();
    assert!(matches!(err, PropAreaError::InPlaceUpdateTooLong { .. }));

    let after = area.get_property_info(key).unwrap().unwrap();
    assert_eq!(after.value, old_value);
    assert!(!after.is_long);
    assert_eq!(after.prop_offset, before.prop_offset);
}
