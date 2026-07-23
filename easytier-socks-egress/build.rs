fn main() {
    println!("cargo:rerun-if-env-changed=HEV_SOCKS5_LIB_DIR");
    println!("cargo:rerun-if-env-changed=HEV_SERVER_COMMIT");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let android_inprocess =
        std::env::var_os("CARGO_FEATURE_HEV_INPROCESS").is_some() && target_os == "android";
    let sidecar = std::env::var_os("CARGO_FEATURE_HEV_SIDECAR_BIN").is_some();
    if !android_inprocess && !sidecar {
        return;
    }
    if sidecar && !matches!(target_os.as_str(), "linux" | "macos") {
        panic!("the managed HEV sidecar is supported only on Linux and macOS");
    }
    let directory = std::env::var_os("HEV_SOCKS5_LIB_DIR")
        .expect("HEV_SOCKS5_LIB_DIR is required for managed HEV builds");
    println!(
        "cargo:rustc-link-search=native={}",
        std::path::Path::new(&directory).display()
    );
    println!("cargo:rustc-link-lib=static=hev-socks5-server");
    println!("cargo:rustc-link-lib=static=yaml");
    println!("cargo:rustc-link-lib=static=hev-task-system");
    if sidecar {
        let commit = std::env::var("HEV_SERVER_COMMIT")
            .expect("HEV_SERVER_COMMIT is required for the managed HEV sidecar");
        println!("cargo:rustc-env=HEV_SERVER_COMMIT={commit}");
    }
}
