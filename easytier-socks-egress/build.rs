fn main() {
    println!("cargo:rerun-if-env-changed=HEV_SOCKS5_LIB_DIR");
    if std::env::var_os("CARGO_FEATURE_HEV_INPROCESS").is_none()
        || std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("android")
    {
        return;
    }
    let directory = std::env::var_os("HEV_SOCKS5_LIB_DIR")
        .expect("HEV_SOCKS5_LIB_DIR is required for Android HEV in-process builds");
    println!(
        "cargo:rustc-link-search=native={}",
        std::path::Path::new(&directory).display()
    );
    println!("cargo:rustc-link-lib=static=hev-socks5-server");
    println!("cargo:rustc-link-lib=static=yaml");
    println!("cargo:rustc-link-lib=static=hev-task-system");
}
