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
    let websocket = matches!(&proxy.transport, ProxyTransport::Websocket { .. });
    let wrapped = proxy.tls.is_some() || websocket;
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
        let mut settings = serde_json::json!({
            "serverName": tls.server_name.as_deref().unwrap_or(address),
            "insecure": tls.insecure,
        });
        if websocket {
            // A WSS actor sends an HTTP/1.1 Upgrade after TLS, so allowing h2
            // negotiation makes an otherwise valid WebSocket endpoint close the
            // connection. This follows Mihomo
            // adapter/outbound/vless.go::StreamConnContext and locked Leaf
            // config/conf/config.rs external WS actor compilation.
            settings["alpn"] = serde_json::json!(["http/1.1"]);
        }
        outbounds.push(serde_json::json!({
            "tag": tag,
            "protocol": "tls",
            "settings": settings,
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
