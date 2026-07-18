use serde_json::Value;

use crate::{Proxy, ProxyTransport};

// Mihomo parity references: adapter/outbound/{trojan,vmess,vless}.go build the
// protocol over a dialer enhanced by transport and TLS layers. Pinned Leaf uses
// explicit chain actors instead, with the externally visible protocol tag kept
// stable so existing EasyTier groups remain unaware of those internal layers.
pub(crate) fn compile_outbounds(
    name: &str,
    address: &str,
    protocol: &str,
    settings: Value,
    proxy: &Proxy,
) -> Vec<Value> {
    let wrapped = proxy.tls.is_some() || proxy.transport != ProxyTransport::Tcp;
    let protocol_tag = if wrapped {
        hidden_tag(name, "protocol")
    } else {
        name.to_owned()
    };
    let mut outbounds = vec![serde_json::json!({
        "tag": protocol_tag,
        "protocol": protocol,
        "settings": settings,
    })];
    if !wrapped {
        return outbounds;
    }

    let mut actors = Vec::new();
    if let Some(tls) = &proxy.tls {
        let tag = hidden_tag(name, "tls");
        outbounds.push(serde_json::json!({
            "tag": tag,
            "protocol": "tls",
            "settings": {
                "serverName": tls.server_name.as_deref().unwrap_or(address),
                "insecure": tls.insecure,
            },
        }));
        actors.push(tag);
    }
    if let ProxyTransport::Websocket { path, headers } = &proxy.transport {
        let tag = hidden_tag(name, "ws");
        outbounds.push(serde_json::json!({
            "tag": tag,
            "protocol": "ws",
            "settings": { "path": path, "headers": headers },
        }));
        actors.push(tag);
    }
    actors.push(protocol_tag);
    outbounds.push(serde_json::json!({
        "tag": name,
        "protocol": "chain",
        "settings": { "actors": actors },
    }));
    outbounds
}

fn hidden_tag(name: &str, layer: &str) -> String {
    format!("@et:{name}:{layer}")
}
