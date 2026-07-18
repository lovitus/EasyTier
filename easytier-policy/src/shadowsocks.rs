use serde_json::Value;

use crate::Proxy;

pub(crate) fn compile_outbound(name: &str, address: &str, port: u16, proxy: &Proxy) -> Value {
    serde_json::json!({
        "tag": name,
        "protocol": "shadowsocks",
        "settings": {
            "address": address,
            "port": port,
            "method": proxy.cipher.as_deref().expect("validated Shadowsocks cipher"),
            "password": proxy.password.as_deref().expect("validated Shadowsocks password"),
            "uotV2": proxy.udp.is_uot_v2(),
        },
    })
}
