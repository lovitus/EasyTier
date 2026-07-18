use serde_json::Value;

use crate::Proxy;

pub(crate) fn compile_outbounds(name: &str, address: &str, port: u16, proxy: &Proxy) -> Vec<Value> {
    crate::stream_transport::compile_outbounds(
        name,
        address,
        "trojan",
        serde_json::json!({
            "address": address,
            "port": port,
            "password": proxy.password.as_deref().expect("validated Trojan password"),
        }),
        proxy,
    )
}
