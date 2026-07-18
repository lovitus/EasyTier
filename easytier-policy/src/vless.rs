use serde_json::Value;

use crate::Proxy;

pub(crate) fn compile_outbounds(name: &str, address: &str, port: u16, proxy: &Proxy) -> Vec<Value> {
    crate::stream_transport::compile_outbounds(
        name,
        address,
        "vless",
        serde_json::json!({
            "address": address,
            "port": port,
            "uuid": proxy.uuid.expect("validated VLESS uuid").to_string(),
        }),
        proxy,
    )
}
