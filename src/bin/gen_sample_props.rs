//! Generate a sanitized prop-area fixture used by integration tests.
//!
//! Usage:
//!   gen_sample_props [output_path]
//!
//! Default output:
//!   tests/fixtures/sample_props.prop

use std::fs::OpenOptions;

use resetprop_rs::PropArea;

fn main() {
    let out_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tests/fixtures/sample_props.prop".to_owned());

    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&out_path)
        .unwrap_or_else(|err| panic!("failed to open '{}': {}", out_path, err));

    let mut area = PropArea::create(file, 128 * 1024)
        .unwrap_or_else(|err| panic!("failed to create prop area '{}': {}", out_path, err));

    let long_codenames = "Alpha,Beta,Cupcake,Donut,Eclair,Froyo,Gingerbread,Honeycomb,IceCreamSandwich,JellyBean,Kitkat,Lollipop,Marshmallow,Nougat,Oreo,Pie,Quince,Raspberry,Snowcone,Tiramisu,UpsideDownCake,VanillaIceCream,WinterBerry";

    let props = [
        ("ro.product.brand", "example"),
        ("ro.product.model", "Sample Device 2026"),
        ("ro.product.device", "sample_device"),
        ("ro.product.locale", "en-US"),
        ("ro.build.version.release", "16"),
        ("ro.build.version.sdk", "36"),
        ("ro.build.version.incremental", "eng.sample.20260312"),
        ("ro.build.version.known_codenames", long_codenames),
        ("ro.secure", "1"),
        ("ro.debuggable", "0"),
        ("ro.boot.verifiedbootstate", "green"),
        ("ro.boot.flash.locked", "1"),
        ("ro.hardware", "qcom"),
        ("ro.kernel.version", "6.1.0-test"),
        ("ro.telephony.sim_slots.count", "2"),
        ("ro.bluetooth.library_name", "libbluetooth.so"),
        ("ro.com.android.mobiledata", "true"),
        ("ro.postinstall.fstab.prefix", "/system"),
        ("ro.support_one_handed_mode", "true"),
        ("persist.sys.locale", "en-US"),
        ("persist.sys.timezone", "UTC"),
        ("persist.sys.usb.config", "mtp,adb"),
        ("persist.log.tag", "I"),
        ("persist.service.adb.enable", "1"),
        ("persist.backup.ntpServer", "pool.ntp.org"),
        ("persist.feature.graphics.pipeline", "skia_gl"),
        ("persist.feature.analytics.opt_out", "1"),
        ("persist.cache.cleaner.interval_hours", "24"),
        ("persist.example.flag.alpha", "true"),
        ("persist.example.flag.beta", "false"),
        ("persist.example.counter", "42"),
        ("sys.boot_completed", "1"),
        ("sys.usb.state", "mtp,adb"),
        ("sys.traced.enable", "1"),
        ("vendor.audio.offload.enable", "true"),
        ("vendor.camera.hdr.default", "on"),
        ("vendor.graphics.vsync.period_ns", "16666666"),
        ("vendor.power.profile", "balanced"),
        ("vendor.wifi.channels", "1,6,11"),
        ("dalvik.vm.heapsize", "512m"),
        ("dalvik.vm.heapstartsize", "8m"),
        ("dalvik.vm.heapgrowthlimit", "256m"),
        ("service.bootanim.exit", "1"),
        ("service.adb.tcp.port", "-1"),
        ("debug.sf.disable_backpressure", "0"),
        ("debug.hwui.renderer", "skiagl"),
        ("debug.stagefright.ccodec", "1"),
        ("init.svc.adbd", "running"),
    ];

    for (key, value) in props {
        area.set_property(key, value)
            .unwrap_or_else(|err| panic!("failed to set '{}': {}", key, err));
    }

    let file = area.into_inner();
    file.sync_all()
        .unwrap_or_else(|err| panic!("failed to sync '{}': {}", out_path, err));

    println!("generated fixture: {}", out_path);
    println!("property count: {}", props.len());
}
