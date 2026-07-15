const COMMANDS: [(&str, &str); 4] = [
    ("PREPARE_VPN_COMMAND", "prepareVpn"),
    ("START_VPN_COMMAND", "startVpn"),
    ("STOP_VPN_COMMAND", "stopVpn"),
    ("GET_VPN_STATUS_COMMAND", "getVpnStatus"),
];

#[test]
fn rust_wrapper_matches_android_plugin_exports() {
    let rust_wrapper = include_str!("../src/mobile.rs");
    let android_plugin = include_str!("../android/src/main/java/VpnServicePlugin.kt");

    for (rust_constant, native_command) in COMMANDS {
        assert!(
            rust_wrapper.contains(&format!("crate::{rust_constant}")),
            "Rust mobile wrapper does not use {rust_constant}"
        );
        assert!(
            android_plugin.contains(&format!("fun {native_command}(invoke: Invoke)")),
            "Android VPN plugin does not export {native_command}"
        );
    }
}
