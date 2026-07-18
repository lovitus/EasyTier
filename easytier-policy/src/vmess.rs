use serde_json::Value;

use crate::Proxy;

pub(crate) fn compile_outbounds(name: &str, address: &str, port: u16, proxy: &Proxy) -> Vec<Value> {
    crate::stream_transport::compile_outbounds(
        name,
        address,
        "vmess",
        serde_json::json!({
            "address": address,
            "port": port,
            "uuid": proxy.uuid.expect("validated VMess uuid").to_string(),
            "security": normalized_security(
                proxy.cipher.as_deref().expect("validated VMess cipher")
            ),
        }),
        proxy,
    )
}

fn normalized_security(cipher: &str) -> &str {
    match cipher {
        // Mihomo transport/vmess/vmess.go selects AES where hardware-accelerated
        // AES is the platform default and ChaCha20 elsewhere.
        "auto"
            if cfg!(any(
                target_arch = "x86_64",
                target_arch = "aarch64",
                target_arch = "s390x"
            )) =>
        {
            "aes-128-gcm"
        }
        "auto" => "chacha20-poly1305",
        "chacha20-ietf-poly1305" => "chacha20-poly1305",
        cipher => cipher,
    }
}
