# Stealth/Secure 加密性能优化方案（v2 — 评审修订版）

**日期**：2026-07-09  
**基于**：`stealth_secure_profiling_2026_07_09.md` 的 profiling 数据  
**修订**：采纳代码评审意见，改用 AEAD 方案，补充缓存架构、nonce 安全、兼容性协商、
测试计划、写锁优化、吞吐预估修正  
**目标**：将 explicit secure + stealth 的 TCP 吞吐从 ~6 MB/s 尽可能提升

## 问题概述

explicit `secure_mode=true` + `stealth_mode=true` 下，TCP 吞吐从 107 MB/s 降至 6 MB/s（17x）。
Profiling 确认根因是 stealth outer 加密层用 HMAC-SHA256 实现流密码，`sha2::sha256::compress256`
占 easytier userspace CPU 的 63.7%。

## 测试环境 CPU 信息

- 测试机：Intel Xeon E5-2620 v4 @ 2.10GHz（两台相同）
- AES-NI：**有**（`flags: aes sse4_1 sse4_2 avx avx2`）
- 部署目标可能包括无 AES-NI 的 ARM / 老 x86 平台

## 优化方案

### P1：用 AEAD 替代整个 seal()/open() — 核心改动

**问题**：`seal()` 和 `open()` 使用 HMAC-SHA256 实现流密码 + MAC，两个操作都基于 SHA-256，
合计占 easytier userspace CPU 的 ~70%。

**方案**：用 `ring::aead` 的 AEAD（AES-256-GCM 或 ChaCha20-Poly1305）替代整个
`apply_keystream` + `outer_mac` + `outer_subkeys`：

- **加密 + 认证一步完成**，彻底消除所有 SHA-256 计算（包括 `outer_mac`）
- 复用已有 `ring` 依赖，通过新增 `stealth-aead` feature 启用（加入 `default` 和 `full`）
- 复用 `peers/encrypt/ring.rs` 中已有的 `LessSafeKey` 封装模式
- 消除 `outer_subkeys` 的 HKDF 派生（AEAD key 直接用 outer key 的 32 字节）

**算法选择**：

| 算法 | 有 AES-NI | 无 AES-NI | 代码库已有 |
| --- | --- | --- | --- |
| AES-256-GCM | ~0.25 cycles/byte（最快） | ~15 cycles/byte（慢） | `ring::aead::AES_256_GCM` ✅ |
| ChaCha20-Poly1305 | ~3 cycles/byte | ~3 cycles/byte（恒定） | `ring::aead::CHACHA20_POLY1305` ✅ |

**策略**：运行时 cipher suite 协商（见下文兼容性设计），优先 AES-256-GCM，
无 AES-NI 时 fallback 到 ChaCha20-Poly1305。

**改动细节**：

1. 新增 `OuterCipher` 枚举，封装 AEAD 加解密：
```rust
enum OuterCipher {
    Legacy { enc_key: [u8; 32], mac_key: [u8; 32] },  // 旧版兼容
    Aes256Gcm(LessSafeKey, [u8; 32]),       // raw key 用于 Clone
    ChaCha20Poly1305(LessSafeKey, [u8; 32]), // raw key 用于 Clone
}
```

`LessSafeKey` 本身不是 `Clone`/`Debug`，因此保存 raw key 在变体中（用于需要重建时）。
手动 impl `Debug`（不暴露 key 内容）。

2. 在 `OuterSessionState` 中新增 `outer_cipher: RwLock<Option<OuterCipher>>` 字段，
   与 `key_phase` 并行管理。`OuterKeyPhase` 保持 `Copy + Debug` 不变。

3. `set_outer_key_from_handshake_hash()` 时根据协商结果构建 `OuterCipher` 并写入
   `outer_cipher` 字段：
   - AEAD：`LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap())`
   - Legacy：`outer_subkeys(key)` 预计算 enc_key/mac_key（兼容旧版本）
   - 签名需增加 `outer_cipher_suite: Option<&str>` 参数

4. `seal_datagram()` / `open_datagram()` 读 `outer_cipher` 字段获取 `OuterCipher`，
   有 cipher 时用 AEAD，无 cipher（gate phase 或 feature 未启用）时走 legacy `seal()`/`open()`

5. gate 阶段（handshake 前）仍用 legacy `seal()`/`open()`（gate key 每次派生，不是热路径）
   `outer_cipher` 为 `None` 时自动 fallback

**文件**：`src/tunnel/stealth.rs`

**wire format 兼容性**：AEAD 改动**不改变 wire format**。当前格式为
`nonce(12) || ciphertext || tag(16)`，总开销 `OUTER_OVERHEAD = 28` 字节。
AEAD 的 tag 同样是 16 字节，`seal_in_place_separate_tag` / `open_in_place`
保持完全相同的 wire layout，`OUTER_OVERHEAD` 不变。

**预期收益**：数据路径 SHA-256 从 63.7% 降到 **0%**（AEAD 彻底消除数据路径中的
SHA-256 计算；连接建立和 gate 阶段仍有少量 SHA-256）。

### P2：消除接收路径堆分配 — 全协议 in-place 解密

**状态：已验证可行性，待评审。2026-07-09 端到端性能测试 + 代码审计确认。**

**问题**：
- `open_datagram` 返回 `Option<Vec<u8>>`，每个入站包 3 次堆分配 + 3 次 memcpy
- `aead::open` 内部：`buf[NONCE_LEN..].to_vec()`（alloc 1）+ `plaintext.to_vec()`（alloc 2）
- 调用方：`BytesMut::from(&plaintext[..])`（alloc 3）
- 端到端验证：CPU 未跑满（82%）但吞吐比直连低 34%，典型分配器争用 stall
- 万兆网卡下（83 万包/秒）仅分配器锁争用就会成为瓶颈

**关键发现**：所有协议的接收路径都拥有 `&mut [u8]` 或可零拷贝转换为 `BytesMut` 的 buffer，
均可在原 buffer 上原地解密，不需要分配新内存。

**改动**：

#### P2.1：`aead::open` 消除第二次 `to_vec()`（2 行，零风险）

```rust
// src/tunnel/stealth.rs, aead::open

// before:
let mut data = buf[NONCE_LEN..].to_vec();
let plaintext = key
    .open_in_place(ring_nonce, aead::Aad::empty(), &mut data)
    .ok()?;
Some(plaintext.to_vec())

// after:
let mut data = buf[NONCE_LEN..].to_vec();
let plaintext_len = key
    .open_in_place(ring_nonce, aead::Aad::empty(), &mut data)
    .ok()?
    .len();
data.truncate(plaintext_len);
Some(data)
```

`open_in_place` 返回 `&[u8]` 借用 `data`，直接 `.len()` 返回 `usize`（Copy），
借用立即释放，`data.truncate()` 可编译。语义完全等价。

#### P2.2：新增 `aead::open_in_place_mut`（~15 行）

```rust
/// Open a sealed buffer in-place. Buffer contains `nonce || ciphertext || tag`.
/// On success, plaintext is moved to buffer start and its length returned.
/// On failure, buffer is left in unspecified state.
#[cfg(feature = "stealth-aead")]
pub fn open_in_place_mut(cipher: &OuterCipher, buf: &mut [u8]) -> Option<usize> {
    if buf.len() < NONCE_LEN + TAG_LEN {
        return None;
    }
    let key = match cipher {
        OuterCipher::Aes256Gcm(k) | OuterCipher::ChaCha20Poly1305(k) => k,
    };
    let nonce: [u8; NONCE_LEN] = buf[..NONCE_LEN].try_into().ok()?;
    let ring_nonce = aead::Nonce::assume_unique_for_key(nonce);
    let plaintext_len = key
        .open_in_place(ring_nonce, aead::Aad::empty(), &mut buf[NONCE_LEN..])
        .ok()?
        .len();
    buf.copy_within(NONCE_LEN..NONCE_LEN + plaintext_len, 0);
    Some(plaintext_len)
}
```

`copy_within` 是 `ptr::copy` 的安全包装，处理重叠区域。plaintext 比 ciphertext+tag 短 28 字节，
移动方向是向前（从 offset 12 到 offset 0），不会覆盖未读数据。

#### P2.3：新增 `OuterSessionState::open_datagram_in_place`（~25 行）

```rust
/// Open a sealed datagram in-place. On success returns plaintext length.
/// Caller truncates buffer to that length. On failure returns None (drop packet).
///
/// Handles ALL key phases internally:
/// - Outer + AEAD: zero-alloc in-place (hot path)
/// - Outer + legacy: allocating `open()` + copy back (cold path)
/// - Gate / PromoteAfterNextSeal: allocating `open()` with 2 window keys + copy back
///
/// **安全关键**：调用方不需要 fallback。返回 `None` = 丢弃包。
/// ring::aead::open_in_place 失败后 buffer 损坏，但调用方不再使用 buffer，
/// 所以没有安全风险。gate/legacy 路径不调用 open_in_place，buffer 不被损坏。
pub fn open_datagram_in_place(&self, buf: &mut [u8]) -> Option<usize> {
    if !self.enabled {
        return None;
    }
    match *self.key_phase.read().unwrap() {
        OuterKeyPhase::Outer(key, _) => {
            #[cfg(feature = "stealth-aead")]
            {
                let cipher = self.outer_cipher.read().unwrap();
                if let Some(ref c) = *cipher {
                    // AEAD in-place: zero alloc. Buffer may be corrupted on failure,
                    // but caller drops the packet on None, so safe.
                    return aead::open_in_place_mut(c, buf);
                }
            }
            // Legacy outer: allocating path + copy back
            let pt = open(&key, buf);
            pt.map(|p| { buf[..p.len()].copy_from_slice(&p); p.len() })
        }
        OuterKeyPhase::Gate | OuterKeyPhase::PromoteAfterNextSeal(_) => {
            // Gate: allocating path with 2 window keys + copy back.
            // Cannot in-place because first window failure would corrupt buffer
            // for second window attempt.
            let cur = window_for(now_secs(), self.window_secs);
            for window in [cur, cur.wrapping_sub(1)] {
                let key = derive_gate_key(&self.network_secret, window);
                if let Some(pt) = open(&key, buf) {
                    buf[..pt.len()].copy_from_slice(&pt);
                    return Some(pt.len());
                }
            }
            None
        }
    }
}
```

**设计决策**：`open_datagram_in_place` 内部处理所有 key phase，调用方不需要
fallback 到 `open_datagram`。这消除了 ring buffer 损坏导致的安全风险——如果
AEAD 认证失败，buffer 已损坏，但调用方直接丢弃包，不再使用 buffer。

**分配数**：
- Outer + AEAD（热路径）：0 次分配
- Outer + legacy：1 次分配（`open()` 内部 `to_vec()`）
- Gate phase：1-2 次分配（`open()` 尝试 1-2 个 window key）

#### P2.4：调用方改动（6 个文件，每处 2-5 行）

**UDP — `src/tunnel/udp.rs`（2 处）**：

listener `try_forward_sealed_data`（line 677, 683-688）：

**签名改动**：`try_forward_sealed_data` 从 `raw: &BytesMut` 改为 `raw: BytesMut`（owned），
因为 in-place 需要可变访问，`BytesMut::clone()` 会共享 allocation 导致 copy-on-write，不是零拷贝。
调用方（line 754）从 `self.try_forward_sealed_data(&raw, addr)` 改为 `self.try_forward_sealed_data(raw, addr)`。

```rust
// before (line 677):
fn try_forward_sealed_data(&self, raw: &BytesMut, addr: SocketAddr) -> bool {
    // ...
    let opened = self.sock_map.get(&addr)
        .and_then(|conn| conn.stealth.open_datagram(raw));
    if let Some(plaintext) = opened {
        match get_zcpacket_from_buf(BytesMut::from(&plaintext[..]), false) { ... }
    }
}

// after:
fn try_forward_sealed_data(&self, mut raw: BytesMut, addr: SocketAddr) -> bool {
    // ...
    // open_datagram_in_place handles ALL key phases (outer AEAD, outer legacy, gate).
    // No caller fallback needed. Returns None = drop packet.
    if let Some(conn) = self.sock_map.get(&addr) {
        if let Some(pt_len) = conn.stealth.open_datagram_in_place(&mut raw) {
            raw.truncate(pt_len);
            match get_zcpacket_from_buf(raw, false) { ... }
            return true;
        }
    }
    // No conn for addr, or open failed (auth failure / parse error)
    false
}
```

调用方（line 751-756）：
```rust
// before:
let raw = buf.split();
if addr_has_conn {
    if self.try_forward_sealed_data(&raw, addr) { continue; }
    match get_zcpacket_from_buf(raw, false) { ... }
}

// after:
let raw = buf.split();
if addr_has_conn {
    if self.try_forward_sealed_data(raw, addr) { continue; }
    // try_forward_sealed_data consumed raw and returned false.
    // This means stealth open failed (auth failure). Fall through to
    // cleartext fallback parsing, but raw was moved...
    // Problem: caller needs raw for cleartext fallback (line 757-769).
    // Solution: try_forward_sealed_data should return Option<BytesMut>:
    //   Some(handled) = success, raw consumed
    //   None = failed, return raw to caller for cleartext fallback
    match self.try_forward_sealed_data(raw, addr) {
        Some(()) => continue,
        None => {
            // raw was consumed; for cleartext fallback we need a different approach.
            // See below for refined signature.
        }
    }
}
```

**签名修正**：`try_forward_sealed_data` 返回 `bool` 时调用方仍需要 `raw` 做 cleartext fallback
（line 757-769：`allow_cleartext_fallback_for_established_addr`）。但 `raw` 被 move 后调用方
没有了。两种处理方式：
1. **改返回类型为 `enum { Handled, Failed(BytesMut) }`**，失败时返回 `raw` 给调用方
2. **在 `try_forward_sealed_data` 内部处理 cleartext fallback**，调用方不需要 `raw`

推荐方式 2：将 cleartext fallback 逻辑移入 `try_forward_sealed_data`，
函数内部在 `open_datagram_in_place` 返回 `None` 后尝试 cleartext 解析。
这样调用方只需 `if self.try_forward_sealed_data(raw, addr) { continue; }`，
`raw` 被 move，所有处理在函数内部完成。

注意：cleartext fallback 时 buffer 可能已被 `open_in_place_mut` 损坏（AEAD 认证失败场景）。
需要在 `open_datagram_in_place` 返回 `None` 后**不使用原 buffer 做 cleartext 解析**。
但当前 cleartext fallback 逻辑是：stealth open 失败后，尝试把 raw 当明文包解析。
如果 buffer 已被 AEAD 部分覆盖，cleartext 解析会失败（header 不对），被丢弃。
这和当前行为一致——当前 `open_datagram` 返回 `None` 后，`raw` 是原始密文，
cleartext fallback 尝试解析也会失败（因为密文不是有效的 tunnel header）。

**结论**：AEAD 认证失败后 buffer 损坏不影响 cleartext fallback 的正确性——
cleartext fallback 本来就不应该成功（密文不是明文包）。但如果要严格保证，
可以在 `try_forward_sealed_data` 内部对 cleartext fallback 使用原始 buffer 的副本。
不过这引入分配，违背零分配目标。推荐接受当前行为：AEAD 失败后 cleartext fallback
也会失败，包被丢弃。

connector recv loop（line 1214-1216）：
```rust
// before:
let parsed = if recv_stealth.is_enabled() {
    match recv_stealth.open_datagram(&raw) {
        Some(pt) => get_zcpacket_from_buf(BytesMut::from(&pt[..]), false),
        None => { continue; }
    }
} else {
    get_zcpacket_from_buf(raw, false)
};

// after:
// open_datagram_in_place handles ALL key phases internally.
// No fallback needed. None = drop packet.
let parsed = if recv_stealth.is_enabled() {
    match recv_stealth.open_datagram_in_place(&mut raw) {
        Some(pt_len) => { raw.truncate(pt_len); get_zcpacket_from_buf(raw, false) }
        None => { continue; }  // auth failure or parse error
    }
} else {
    get_zcpacket_from_buf(raw, false)
};
```

**TCP — `src/tunnel/common.rs`（1 处，line 240-254）**：
```rust
// before:
let record = buf.split_to(TCP_TUNNEL_HEADER_SIZE + sealed_len);
let Some(plaintext) = stealth.open_datagram(&record[TCP_TUNNEL_HEADER_SIZE..]) else { ... };
Some(Ok(ZCPacket::new_from_buf(BytesMut::from(plaintext.as_slice()), ZCPacketType::TCP)))

// after:
// open_datagram_in_place handles ALL key phases internally.
let mut record = buf.split_to(TCP_TUNNEL_HEADER_SIZE + sealed_len);
let sealed = record.split_off(TCP_TUNNEL_HEADER_SIZE);
match stealth.open_datagram_in_place(&mut sealed) {
    Some(pt_len) => {
        sealed.truncate(pt_len);
        Some(Ok(ZCPacket::new_from_buf(sealed, ZCPacketType::TCP)))
    }
    None => Some(Err(TunnelError::InvalidPacket(
        "TCP stealth authentication failed".into()))),
}
```

**FakeTCP — `src/tunnel/fake_tcp/mod.rs`**：**不改动**。

FakeTCP 有额外的 gate-key fallback 逻辑（`DroppedStaleGateControl`，line 791-816），
用于检测和丢弃 stale Noise handshake duplicates。这需要区分"outer open 成功"和
"gate open 成功"来做不同处理（gate 成功后要检查是否是 Noise handshake packet）。
`open_datagram_in_place` 内部处理所有 phase 但不区分 outer/gate 成功，无法满足
FakeTCP 的 `DroppedStaleGateControl` 需求。

此外 FakeTCP 不是热路径（TCP 有 Nagle/coalescing，单次调用可能处理多个 record），
改动风险大于收益。保留 allocating `open_datagram` + `open_gate_datagram` 路径。

**WebSocket — `src/tunnel/websocket.rs`（1 处，line 200-216）**：
```rust
// before:
let payload = msg.into_payload();
let plaintext = if stealth.is_enabled() {
    match stealth.open_datagram(payload.as_bytes()) {
        Some(plaintext) => plaintext,
        None => { return Some(Err(...)); }
    }
} else {
    payload.as_bytes().to_vec()
};
Some(Ok(ZCPacket::new_from_buf(BytesMut::from(plaintext.as_slice()), ZCPacketType::DummyTunnel)))

// after:
// open_datagram_in_place handles ALL key phases internally.
if stealth.is_enabled() {
    let mut buf = BytesMut::from(msg.into_payload());  // zero-copy: Payload→BytesMut
    match stealth.open_datagram_in_place(&mut buf) {
        Some(pt_len) => {
            buf.truncate(pt_len);
            Some(Ok(ZCPacket::new_from_buf(buf, ZCPacketType::DummyTunnel)))
        }
        None => Some(Err(TunnelError::InvalidPacket("invalid WS stealth record".into()))),
    }
} else {
    let payload = msg.into_payload();
    Some(Ok(ZCPacket::new_from_buf(
        BytesMut::from(payload.as_bytes()), ZCPacketType::DummyTunnel,
    )))
}
```

**`Payload` → `BytesMut` 零拷贝**：`Payload` 内部是 `Bytes`，
`From<Payload> for BytesMut` 调用 `Bytes::into()`，当 refcount=1 时是零拷贝。
WS 消息刚从 stream 读取，refcount=1，满足条件。

**QUIC — `src/tunnel/quic.rs`（line 563-609）**：

新增 `open_in_place` 方法到 `QuicStealthSocket`：
```rust
fn open_in_place(&self, addr: SocketAddr, buf: &mut [u8]) -> Option<(usize, Arc<QuicStealthSession>)> {
    if let Some(session) = self.session(addr) {
        if let Some(pt_len) = session.open_in_place(buf) {
            if session.state.outer_key().is_some() && Self::is_quic_initial(&buf[..pt_len]) {
                return None;
            }
            return Some((pt_len, session));
        }
        return None;
    }
    // Candidate path: need to try open, but in-place would corrupt buf on failure.
    // Use allocating path for candidate (rare, only first packet).
    let candidate = Arc::new(QuicStealthSession::new(
        self.template.fork_for_transport_delayed_transition(),
    ));
    let plaintext = candidate.open(buf)?;  // allocating path
    if !Self::is_quic_initial(&plaintext) || !self.accept_initial_nonce(buf) {
        return None;
    }
    // Copy plaintext back into buf (candidate path, rare)
    buf[..plaintext.len()].copy_from_slice(&plaintext);
    // ... register session ...
    Some((plaintext.len(), candidate))
}
```

`QuicStealthSession::open_in_place`：

由于 `open_datagram_in_place` 已内部处理所有 key phase（outer AEAD in-place，
outer legacy / gate 用 allocating + copy back），`QuicStealthSession::open_in_place`
只需直接委托：

```rust
fn open_in_place(&self, buf: &mut [u8]) -> Option<usize> {
    // open_datagram_in_place handles ALL phases:
    // - Outer + AEAD: zero-alloc in-place
    // - Outer + legacy: allocating + copy back
    // - Gate / PromoteAfterNextSeal: allocating with 2 window keys + copy back
    // - Grace period: handled by key_phase state, not by elapsed time
    self.state.open_datagram_in_place(buf)
}
```

**注意**：当前 `QuicStealthSession::open` 有 grace period 逻辑——先试 `open_datagram`，
失败后试 `open_gate_datagram`。但 `open_datagram` 在 `Outer` phase 下只试 outer key，
不试 gate key。grace period 的 gate fallback 是通过 `open_gate_datagram` 显式调用的。

`open_datagram_in_place` 内部在 `Outer` phase 下也只试 outer key（和 `open_datagram`
一致），在 `Gate` / `PromoteAfterNextSeal` phase 下试 gate key。所以如果
`key_phase` 是 `Outer`，`open_datagram_in_place` 不会试 gate key——和当前
`open_datagram` 行为一致。

**差异**：当前 `QuicStealthSession::open` 在 grace period（`elapsed <= GRACE`）时，
即使 `key_phase` 是 `Outer`，也会在 `open_datagram` 失败后 fallback 到
`open_gate_datagram`。但 `open_datagram_in_place` 在 `Outer` phase 下不会做这个 fallback。

**影响分析**：grace period 的 gate fallback 是为了处理"outer key 已设置但对端仍在发
gate-key 包"的过渡期。如果 `open_datagram_in_place` 在 `Outer` phase 下不 fallback
到 gate key，过渡期的 gate-key 包会被丢弃。

**修正**：需要在 `open_datagram_in_place` 中增加 grace period gate fallback。
但这引入 buffer 损坏风险——AEAD `open_in_place` 失败后 buffer 已损坏。

**方案**：`open_datagram_in_place` 在 `Outer` + AEAD 路径失败时返回 `None`，
调用方（`QuicStealthSession::open_in_place`）在 grace period 内用 allocating 路径
重试 gate key。但 buffer 已损坏，不能直接传给 `open_gate_datagram`。

**最终方案**：QUIC grace period 保留 allocating 路径，不使用 in-place：

```rust
fn open_in_place(&self, buf: &mut [u8]) -> Option<usize> {
    match self.outer_elapsed() {
        Some(elapsed) if elapsed >= QUIC_STEALTH_OUTER_SEND_DELAY => {
            // Definite outer phase: safe to in-place.
            // open_datagram_in_place handles outer AEAD (in-place) and
            // outer legacy (allocating + copy back).
            self.state.open_datagram_in_place(buf)
        }
        Some(elapsed) => {
            // Grace period: CANNOT use in-place.
            // ring::open_in_place 失败后 buffer 损坏，gate fallback 会得到错误结果。
            // 使用 allocating 路径，解密后 copy 回 buf。
            self.state.open_datagram(buf)
                .or_else(|| (elapsed <= QUIC_STEALTH_GATE_RECV_GRACE)
                    .then(|| self.state.open_gate_datagram(buf))
                    .flatten())
                .map(|pt| { buf[..pt.len()].copy_from_slice(&pt); pt.len() })
        }
        None => {
            // Gate only: open_datagram_in_place handles this (allocating + copy back)
            self.state.open_datagram_in_place(buf)
        }
    }
}
```

**关键安全约束**：grace period 期间不能用 in-place。`ring::aead::open_in_place` 失败后
buffer 处于 unspecified state（可能已部分解密），再传给 `open_gate_datagram` 会得到
错误结果。只有 `elapsed >= QUIC_STEALTH_OUTER_SEND_DELAY`（确定已进入 outer phase）
时才使用 in-place。

`poll_recv` 改动：
```rust
// before (line 578-608):
let mut opened = Vec::with_capacity(count);
for index in 0..count {
    let sealed = &bufs[index][offset..end];
    if let Some((plaintext, _)) = self.open_from(received.addr, sealed) {
        opened.push((plaintext, received));
    }
}
// then copy_from_slice back to bufs

// after:
let mut opened_count = 0;
for index in 0..count {
    let received = meta[index];
    let stride = received.stride.max(1);
    let mut offset = 0;
    while offset < received.len && opened_count < bufs.len().min(meta.len()) {
        let end = (offset + stride).min(received.len);
        if let Some((pt_len, _)) = self.open_in_place(received.addr, &mut bufs[index][offset..end]) {
            // Move plaintext to destination buffer
            if opened_count != index || offset != 0 {
                bufs[opened_count][..pt_len].copy_from_slice(&bufs[index][offset..offset + pt_len]);
            }
            meta[opened_count] = RecvMeta {
                len: pt_len, stride: pt_len,
                addr: received.addr, port: received.port,
            };
            opened_count += 1;
        }
        offset = end;
    }
}
if opened_count == 0 { continue; }
return Poll::Ready(Ok(opened_count));
```

消除 `opened: Vec<(Vec<u8>, RecvMeta)>` 中间分配。单 stride（最常见）时
`opened_count == index && offset == 0`，不触发 copy_from_slice，为 0 memcpy。

**WireGuard — `src/tunnel/wireguard.rs`（2 处）**：

需新增 `WgStealthSession::open_in_place` 方法（当前只有 `open`，line 73）。
与 `QuicStealthSession::open_in_place` 对称，包含 grace period 安全约束：

```rust
/// WgStealthSession 新增方法
fn open_in_place(&self, buf: &mut [u8]) -> Option<usize> {
    match self.outer_elapsed() {
        Some(elapsed) if elapsed >= WG_STEALTH_OUTER_SEND_DELAY => {
            // Definite outer phase: safe to in-place.
            self.state.open_datagram_in_place(buf)
        }
        Some(elapsed) => {
            // Grace period: CANNOT use in-place.
            // ring::open_in_place 失败后 buffer 损坏，gate fallback 会得到错误结果。
            // 使用 allocating 路径，解密后 copy 回 buf。
            self.state.open_datagram(buf)
                .or_else(|| (elapsed <= WG_STEALTH_GATE_RECV_GRACE)
                    .then(|| self.state.open_gate_datagram(buf))
                    .flatten())
                .map(|pt| { buf[..pt.len()].copy_from_slice(&pt); pt.len() })
        }
        None => {
            // Gate only: open_datagram_in_place handles gate phase internally
            // (allocating + copy back, tries 2 window keys)
            self.state.open_datagram_in_place(buf)
        }
    }
}
```

`open_network_packet`（line 558-562）：**不改动**。

此方法只在 handshake 阶段调用（line 709/712/938，用于打开 handshake response），
不是热路径。保留 allocating `stealth.open()` 路径。

recv loop（line 926-931）：
```rust
// before:
let Some(packet) = data.stealth.as_ref().map_or_else(
    || Some(buf[..n].to_vec()),
    |stealth| stealth.open(&buf[..n]),
) else { continue; };
let _ = data.handle_one_packet_from_peer(&mut sink, &packet).await;

// after:
let pt_len = if let Some(stealth) = data.stealth.as_ref() {
    match stealth.open_in_place(&mut buf[..n]) {
        Some(len) => len,
        None => continue,
    }
} else { n };
let _ = data.handle_one_packet_from_peer(&mut sink, &buf[..pt_len]).await;
```

需确认 `handle_one_packet_from_peer` 接受 `&[u8]` 而非 `Vec<u8>`。

#### P2.5：不修改的部分

- `seal_datagram`：发送路径只有 1 次分配（`Vec::with_capacity`），不是瓶颈，不改
- `seal_gate_datagram` / `open_gate_datagram`：gate phase 非热路径，不改
- legacy `seal()` / `open()`：`stealth-aead` feature 未启用时使用，不改签名
  （但 P2.1 的 2 行修复对 legacy `open()` 无影响，仅对 `aead::open` 生效）

#### P2.6：测试计划

- 单元测试：`stealth::tests` 和 `stealth::aead_tests` 全部通过（不改测试）
- 新增单元测试：`open_in_place_mut` 正确性 + 边界条件（空 buffer、过短 buffer、tag 错误）
- 新增单元测试：`open_datagram_in_place` 在 outer phase 成功、gate phase 返回 None
- 端到端验证：与 P1 验证相同的测试矩阵（UDP/TCP/QUIC/WS/WG + stealth + secure + proxy）
- 性能对比：直连 vs stealth+secure，单流和 4 并发，对比改前改后

#### P2.7：风险评估

| 改动 | 风险 | 说明 |
| --- | --- | --- |
| P2.1 `aead::open` 2 行 | 零 | 纯语义等价，`len()` 返回 Copy 类型 |
| P2.2 `open_in_place_mut` | 低 | `copy_within` 是 stdlib 安全 API，处理重叠区域 |
| P2.3 `open_datagram_in_place` | 低 | 内部处理所有 key phase，调用方不需 fallback |
| UDP 调用方 | 低 | 签名从 `&BytesMut` 改为 `BytesMut`（owned），调用方 move 而非 clone |
| TCP 调用方 | 低 | `split_off` 后 in-place，refcount=1 零拷贝 |
| WS 调用方 | 低 | `Payload→BytesMut` 零拷贝已验证 |
| QUIC 调用方 | 中 | GRO 多 stride 逻辑较复杂；grace period 用 allocating 路径避免 buffer 损坏 |
| WG 调用方 | 低 | 新增 `WgStealthSession::open_in_place`，grace period 用 allocating 路径 |
| FakeTCP | 不改动 | `DroppedStaleGateControl` 需区分 outer/gate 成功，非热路径 |
| WG `open_network_packet` | 不改动 | 仅 handshake 阶段调用，非热路径 |
| **grace period in-place 安全** | **关键** | `ring::open_in_place` 失败后 buffer 损坏，**只有 `elapsed >= delay` 时才 in-place**，grace period 期间用 allocating 路径 |

#### P2.8：改动量统计

| 文件 | 新增/改动行数 | 说明 |
| --- | ---: | --- |
| `src/tunnel/stealth.rs` | ~35 | P2.1 (2 行) + P2.2 (~15 行) + P2.3 (~20 行) |
| `src/tunnel/udp.rs` | ~10 | 2 处调用点 |
| `src/tunnel/common.rs` | ~8 | 1 处调用点 |
| `src/tunnel/fake_tcp/mod.rs` | 0 | **不改动**：`DroppedStaleGateControl` 需区分 outer/gate |
| `src/tunnel/websocket.rs` | ~10 | 1 处调用点 |
| `src/tunnel/quic.rs` | ~25 | `QuicStealthSession::open_in_place` + `QuicStealthSocket::open_in_place` + `poll_recv` 改动 |
| `src/tunnel/wireguard.rs` | ~20 | 新增 `WgStealthSession::open_in_place` (~15 行) + recv loop (5 行)；`open_network_packet` 不改动 |
| **合计** | **~119** | |

#### P2.9：预期收益

| 指标 | 改前 | 改后 |
| --- | --- | --- |
| 入站包堆分配 | 3 次 | 0 次（全协议） |
| 入站包 memcpy | 3 次 | 0 次（QUIC GRO 多 stride 偶尔 1 次） |
| `memcpy` CPU 占比 | 12.7%* | < 3% |
| malloc/free CPU 占比 | 1.2%* | ~0% |
| 万兆网卡下分配器锁争用 | 严重 | 消除 |

*profiling 数据来自 HMAC-SHA256 时代；AEAD 后 SHA-256 消除，memcpy 和分配占比会相对升高。

### P3：用计数器替代 OsRng 生成 nonce

**问题**：`OsRng.fill_bytes()` 每包调用 `getrandom()` 系统调用。

**改动**：用 `AtomicU64` 计数器生成 96-bit AEAD nonce：

**nonce 构造**（12 字节）：
```
[ 8 bytes: counter (big-endian) | 4 bytes: connection salt ]
```

**salt 生成**：
- 从 Noise handshake hash 派生：`hkdf_sha256(handshake_hash, b"et-outer-nonce-salt")` 取前 4 字节
- 每连接不同，重启后不同（handshake hash 包含 ephemeral keys）
- **不能从 outer key 派生**——否则重启后相同 key + counter=0 = nonce 重用

**计数器溢出**：
- u64 计数器，2^64 包溢出。@5200 pkt/s = ~10^11 年，实际不会发生
- 文档化：计数器溢出时连接必须重新握手

**与 AEAD 内部计数器的关系**：
- `ring::aead` 的 nonce 是 96-bit，直接作为完整 nonce 传入，不涉及内部计数器
- 不使用 `ring` 的 `generate_nonce()`（它用 OsRng），而是显式传入计数器构造的 nonce

**安全性**：
- 96-bit nonce + per-connection salt：nonce 空间 2^96，单连接内 counter 单调递增，无 nonce 重用风险
- birthday bound：2^48 包（~300 年 @ 5200 pkt/s），可接受

**文件**：`src/tunnel/stealth.rs`

**预期收益**：每包减少 1 次系统调用（`syscall` 占 0.9%）。

### P4：seal_datagram 读锁优先

**问题**：`seal_datagram()` 每包获取 `key_phase.write()`（`stealth.rs:857`），
即使在 `Outer` 热路径中 phase 不会改变。在 `--multi-thread-count 4` 下导致不必要争用。

**改动**：
```rust
pub fn seal_datagram(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
    // 快速路径：读锁
    let phase = self.key_phase.read().unwrap();
    if let OuterKeyPhase::Outer(key, _) = *phase {
        drop(phase);
        let cipher = self.outer_cipher.read().unwrap();
        if let Some(c) = cipher.as_ref() {
            return Some(seal_aead(c, &key, plaintext));
        }
        // cipher 为 None：feature 未启用或协商降级到 legacy
        return Some(seal(&key, plaintext));
    }
    drop(phase);
    // 慢速路径：写锁（仅 Gate → PromoteAfterNextSeal → Outer 转换时）
    let mut phase = self.key_phase.write().unwrap();
    // ... 原有逻辑 ...
}
```

**注意**：并行方案下 `OuterCipher` 不需要 `Clone`（读锁直接引用，不取出值）。

**文件**：`src/tunnel/stealth.rs`

**预期收益**：消除热路径写锁争用。当前 0.8%，AEAD 优化后可能成为新瓶颈，提前消除。

### P5：合并 replay check 锁

**问题**：`precheck_replay` 和 `commit_replay` 各自独立加锁 `rx_slots`。

**改动**：在 `decrypt_payload` 中合并为单次锁。

**文件**：`src/peers/secure_datagram.rs`

**预期收益**：锁竞争从 0.8% 降到 ~0.4%。低优先级。

## 兼容性设计

### Cipher suite 协商机制

利用现有 Noise handshake protobuf 字段扩展：

**proto 改动**（`src/proto/peer_rpc.proto`）：
```protobuf
message PeerConnNoiseMsg1Pb {
  // ... existing fields 1-5 ...
  optional string outer_cipher_suite = 6;  // 新增：客户端支持的 outer cipher
}

message PeerConnNoiseMsg2Pb {
  // ... existing fields 1-10 ...
  optional string outer_cipher_suite = 11;  // 新增：服务端选择的 outer cipher
}

message RelayNoiseMsg1Pb {
  // ... existing fields 1-5 ...
  optional string outer_cipher_suite = 6;  // 新增
}

message RelayNoiseMsg2Pb {
  // ... existing fields 1-10 ...
  optional string outer_cipher_suite = 11;  // 新增
}
```

**协商逻辑**：
1. 客户端在 msg1 中发送 `outer_cipher_suite`（如 `"aes-256-gcm"` 或 `"chacha20-poly1305"`）
2. 服务端在 msg2 中回复选择的 `outer_cipher_suite`
3. 如果客户端不发 `outer_cipher_suite`（旧版本），服务端不回复，双方使用 legacy HMAC-SHA256
4. protobuf unknown field 行为：旧版本自动忽略新字段，**向后兼容**

**outer_cipher_suite 取值**：
- 不设置 / absent：legacy HMAC-SHA256（兼容旧版本）
- `"aes-256-gcm"`：AES-256-GCM AEAD
- `"chacha20-poly1305"`：ChaCha20-Poly1305 AEAD
- 客户端可发送 `"aes-256-gcm,chacha20-poly1305"` 表示支持多个，服务端选择第一个它也支持的

**降级行为**：
- 服务端不支持客户端提议的 cipher → 回复不设置 `outer_cipher_suite` → 双方用 legacy
- 客户端不支持服务端选择的 cipher → 连接失败（不应发生，因为客户端只提议自己支持的）

**feature flag**：
- 需要新增 `stealth-aead` feature（`stealth-aead = ["dep:ring"]`），加入 `default` 和 `full`
- 无该 feature 时 `stealth.rs` 通过 `#[cfg(feature = "stealth-aead")]` fallback 到 legacy HMAC-SHA256
- 运行时协商确保旧二进制自动降级到 legacy

## 实施建议

### 阶段一（核心改动，最大收益）

P1 + P4 一起实施：
- AEAD 替代 HMAC-SHA256 + 读锁优先
- 改 `src/tunnel/stealth.rs` + `src/proto/peer_rpc.proto`
- 数据路径 SHA-256 从 63.7% 降到 **0%**
- 预期吞吐：6 MB/s → **15-25 MB/s**（2.5-4x）
  - 理论最大加速 = 1/(1-0.7) ≈ 3.3x，即 ~20 MB/s
  - 考虑其他开销（memcpy 12.7%、lock 0.8%、syscall 0.9%），实际 15-25 MB/s

### 阶段二（减少内存开销）

P2 + P3：
- in-place seal/open + 计数器 nonce
- 改 `stealth.rs` + 调用方
- 预期吞吐：**25-40 MB/s**
  - memcpy 从 12.7% 降到 ~5%，malloc/free 从 1.2% 降到 ~0.5%
  - syscall 从 0.9% 降到 ~0%

### 阶段三（收尾 + 突破单线程瓶颈）

P5 + 方案 C（PeerConn 批量）：
- 合并 replay check 锁
- 批量处理减少 per-packet overhead
- 预期吞吐：**40-80 MB/s**（接近 plain 需要批量处理）
  - 要达到接近 100 MB/s 可能还需要解决单线程处理模型

### 吞吐预估修正说明

原方案预估 P1+P2 后 30-50 MB/s，P3+P4 后 70-90 MB/s，过于乐观。

修正依据：
- SHA-256 占 easytier userspace 的 ~70%，但 easytier userspace 仅占总 CPU 的 14%
  （82% idle）。SHA-256 占总 CPU 的 ~10%。
- 即使完全消除 SHA-256，理论最大加速 = 1/(1-0.10) ≈ 1.11x 总 CPU，但单核视角下
  SHA-256 占该核 ~70%，单核加速 = 1/(1-0.7) ≈ 3.3x
- 82% idle（4 核）= 18% 总利用率 = 0.72 核，并非单核满载
  → 可能存在 I/O 等待或其他非 CPU 瓶颈，限制纯 CPU 优化的吞吐提升
- 更合理的预估：P1 → 15-25 MB/s，P1+P2+P3 → 25-40 MB/s，+方案 C → 40-80 MB/s

## 测试计划

### 正确性测试

- **seal/open roundtrip**：新 AEAD cipher 的 seal → open 往返测试（已有测试框架 `aead_tests`）
- **已知向量测试**：用 NIST/RFC 测试向量验证 AES-256-GCM 和 ChaCha20-Poly1305 实现正确
- **cipher suite 交叉测试**：AES-GCM ↔ ChaCha20、AEAD ↔ Legacy 混合通信测试
- **gate phase 兼容测试**：gate 阶段仍用 legacy `seal()`/`open()`，验证不受 AEAD 改动影响

### 兼容性测试

- **旧版本混合通信**：新版本（支持 AEAD）与旧版本（仅 legacy）互通，验证降级到 legacy
- **protobuf 向后兼容**：旧版本收到 `outer_cipher_suite` 字段时正确忽略
- **cipher suite 协商失败**：客户端提议不支持的 cipher，验证降级到 legacy

### 性能回归测试

- **优化前 baseline**：6.3 MB/s（已记录）
- **P1 后**：预期 15-25 MB/s
- **P1+P2+P3 后**：预期 25-40 MB/s
- **测试方法**：Python2 TCP throughput test，256 MB，同 LAN 两节点
- **profiling 验证**：优化后重新 perf record，确认 SHA-256 消失

### nonce 安全测试

- **nonce 单调递增**：验证计数器 nonce 在连续 seal 调用中严格递增
- **nonce 不重复**：同一连接内 10000 次 seal，验证所有 nonce 唯一
- **重启后 nonce 不同**：两次连接（相同 key），验证 nonce salt 不同
- **计数器溢出**：u64 接近溢出时的行为（单元测试，手动设置高 counter）

## 与方案 C（PeerConn 批量）的关系

profiling 确认瓶颈是加密计算而非 per-packet overhead。但消除 SHA-256 后，
per-packet overhead（memcpy、lock、syscall）可能成为新的瓶颈。

- P1（AEAD）+ 方案 C = 根治 + 突破单线程瓶颈
- 仅 P1 = 消除 SHA-256 瓶颈，但单线程处理模型限制吞吐在 ~25 MB/s
- P1 + P2 + P3 + 方案 C = 最优组合，预期 40-80 MB/s

建议先实施 P1+P4，测量吞吐，再根据结果决定是否需要 P2+P3 和方案 C。

## 自审发现（v2 复审）

### 问题 1：`ring` feature 依赖 — P1 可能无法编译

**现状**：`ring` 是 `optional = true`（`Cargo.toml:184`），仅在 `wireguard` feature 中启用
（`Cargo.toml:379: wireguard = ["dep:boringtun", "dep:ring"]`）。`default` 包含 `wireguard`，
所以默认构建有 `ring`。但 `stealth.rs` 当前**不依赖 `ring`**——只用 `hmac` + `sha2`
（都是非 optional 依赖）。

**影响**：P1 引入 `ring::aead` 后，`stealth.rs` 将依赖 `ring`。如果用户构建时
禁用 `wireguard`（`--no-default-features --features smoltcp,tun,kcp,quic,...`），
`stealth.rs` 将无法编译。

**修正**（方案 A + C 组合）：
- 新增 `stealth-aead` feature（`stealth-aead = ["dep:ring"]`）
- `default` 列表（`Cargo.toml:353-364`）加入 `"stealth-aead"`
- `full` 列表（`Cargo.toml:365-378`）加入 `"stealth-aead"`
- `stealth.rs` 中 AEAD 代码路径用 `#[cfg(feature = "stealth-aead")]` 条件编译
- 无 `stealth-aead` feature 时 fallback 到 legacy HMAC-SHA256（当前代码不变）
- 这与代码库现有模式一致：加密 backend 跟着 feature 走（如 `aes-gcm` feature
  独立于 `wireguard`，PeerSession 加密用的是 `ring` 通过 `wireguard` feature）

### 问题 2：`OuterKeyPhase` 的 `Copy + Debug` 约束 — P1 架构阻碍

**现状**：`OuterKeyPhase` 派生了 `Debug, Clone, Copy`（`stealth.rs:421`）。
`LessSafeKey` 不是 `Copy`，不是 `Debug`，手动 `Clone`（通过保存 raw key 重建）。

**影响**：方案中 `OuterKeyPhase::Outer([u8; 32], OuterCipher, Instant)` 无法派生
`Copy` 和 `Debug`。这影响多处代码：

1. `seal_datagram:863` — `PromoteAfterNextSeal(outer_key) => { *phase = OuterKeyPhase::Outer(outer_key, ...); }`
   当前代码靠 `Copy` 从 enum 中取出 `outer_key`。没有 `Copy`，这是 move，
   在 `*phase` 的 match 中会报错（不能从 `&mut` 后的 enum 中 move 出值）。

2. `open_datagram:882` — `match *self.key_phase.read().unwrap()`
   对 `&OuterKeyPhase` 做 `*` deref + match by value 需要 `Copy`。
   没有 `Copy` 需要改为 `match &*self.key_phase.read().unwrap()`（match by reference）。

3. `set_outer_key_from_handshake_hash:479` — `matches!(*phase, OuterKeyPhase::Outer(current, _) if current == key)`
   `matches!` 宏内部用 `Copy` 取出 `current`。没有 `Copy` 需要改为手动 match by reference。

4. `promote_outer_key_after_next_seal:494-498` — 同上 `matches!` 问题。

**主方案（并行字段，推荐）**：不把 `OuterCipher` 放入 `OuterKeyPhase`，而是在
`OuterSessionState` 中新增一个 `RwLock<Option<OuterCipher>>` 字段，与 `key_phase`
并行管理。`key_phase` 仍保持 `Copy`，`OuterCipher` 在 `set_outer_key_from_handshake_hash`
时单独构建。

优势：
- `OuterKeyPhase` 保持 `Copy + Debug` 不变，所有 match/matches! 代码不需要改
- 只需新增一个字段 + 在 set/promote 时同时写 cipher
- `seal_datagram` / `open_datagram` 读 cipher 字段（读锁）
- 改动面最小，风险最低

```rust
pub struct OuterSessionState {
    // ... existing fields ...
    outer_cipher: RwLock<Option<OuterCipher>>,  // 并行于 key_phase
}
```

- `set_outer_key_from_handshake_hash` 时：写 `key_phase`（如现有）+ 写 `outer_cipher`
- `promote_outer_key_after_next_seal` 时：写 `key_phase` + 预构建 `outer_cipher` 存入
- `seal_datagram` / `open_datagram`：读 `key_phase` 判断 phase，读 `outer_cipher` 获取 cipher
- Gate phase：`outer_cipher` 为 `None`，走 legacy `seal()`/`open()`

**备选方案（嵌入枚举）**：将 `OuterCipher` 放入 `OuterKeyPhase` 变体。需要：
- 移除 `OuterKeyPhase` 的 `Copy` 和 `Debug` derive
- 所有 `match *phase` 改为 `match &*phase`
- `matches!` 宏改为手动 match by reference
- `Debug` 改为手动 impl
- `Clone` 保留手动 impl（从 raw key 重建 `LessSafeKey`）
- `PromoteAfterNextSeal` 变体中取出值时用 `std::mem::replace` 或 `Option` 包装
- 改动面大，4+ 处代码需要重写，风险较高

### 问题 3：SHA-256 不会降到 0% — P1 预期收益不准确

**现状**：P3 nonce salt 生成用 `hkdf_sha256(handshake_hash, b"et-outer-nonce-salt")`，
`derive_outer_key` 也用 `hkdf_sha256(handshake_hash, b"et-outer")`。gate key 派生
`derive_gate_key` 也用 `hkdf_sha256`。gate token 验证用 `HmacSha256`。

**影响**：AEAD 替代 outer seal/open 后，SHA-256 仍在以下场景使用：
- **每连接一次**：`derive_outer_key` + nonce salt 派生（2 次 HKDF = 4 次 HMAC-SHA256）
- **gate 阶段每包**：`derive_gate_key` + `gate_tag`（gate key 用于握手包，非数据热路径）
- **gate token 验证**：SYN/SACK 握手时（非数据热路径）

**修正**：P1 预期收益应改为"SHA-256 从 **数据热路径** 中完全消除"，而非"降到 0%"。
在 profiling 的数据路径采样中 SHA-256 确实降到 0%，但连接建立和 gate 阶段仍有 SHA-256。
表述应精确为："数据路径 SHA-256 从 63.7% 降到 0%"。

### 问题 4：cipher suite 信息传递路径未说明

**现状**：`set_outer_key_from_handshake_hash(&self, handshake_hash: &[u8])` 和
`promote_outer_key_after_next_seal(&self, handshake_hash: &[u8])` 只接收
`handshake_hash`，不接收 cipher suite 信息。

调用方 `peer_conn.rs:1467` — `install_stealth_outer_key()` 只传 `noise.handshake_hash`。
`peer_conn.rs:837` — `promote_outer_key_after_next_seal(hs.get_handshake_hash())`。

**影响**：P1 方案说"根据协商结果构建 `OuterCipher`"，但 `set_outer_key_from_handshake_hash`
和 `promote_outer_key_after_next_seal` 的签名需要扩展，加入 cipher suite 参数。

**修正**：
- `set_outer_key_from_handshake_hash` 增加 `outer_cipher_suite: Option<&str>` 参数
- `promote_outer_key_after_next_seal` 同上
- `install_stealth_outer_key()` 从 `NoiseHandshakeResult` 中获取协商的 cipher suite
- `NoiseHandshakeResult` 需要新增 `outer_cipher_suite: Option<String>` 字段
- `do_noise_handshake_as_client` 从 `msg2_pb.outer_cipher_suite` 获取
- `do_noise_handshake_as_server` 从 `msg1_pb.outer_cipher_suite` 获取并写入 `msg2_pb`
- `fork_for_connection()` 不需要改（新连接默认 Gate phase，无 cipher）

### 问题 5：`PromoteAfterNextSeal` 需要预构建 cipher — P1 设计遗漏

**现状**：`promote_outer_key_after_next_seal` 在 Noise msg3 发送前调用，此时
handshake 已完成，cipher suite 已协商。但 `PromoteAfterNextSeal` 变体当前只存
`[u8; 32]`（outer key），如果 `OuterCipher` 需要在此时构建，`PromoteAfterNextSeal`
也要携带 `OuterCipher`。

**影响**：方案中 `OuterKeyPhase::PromoteAfterNextSeal([u8; 32], OuterCipher)`
需要在 `promote_outer_key_after_next_seal` 时构建 `OuterCipher`。但此时是
**客户端 msg3 发送路径**，cipher suite 已从 msg2 获知，可以构建。

**修正**：确认 `promote_outer_key_after_next_seal` 签名也需要 cipher suite 参数，
在构建 `PromoteAfterNextSeal` 变体时同时构建 `OuterCipher`。方案中的枚举定义
`PromoteAfterNextSeal([u8; 32], OuterCipher)` 是正确的，但需要补充签名改动说明。

### 问题 6：nonce 计数器存储位置未明确 — P3 设计遗漏

**现状**：P3 方案说用 `AtomicU64` 计数器，但没说放在哪里。

**约束**：
- `OuterSessionState` 是 per-connection 的（通过 `fork_for_connection` 创建）
- `seal_datagram` 是 `&self`（不可变引用）
- 计数器需要 `AtomicU64`（内部可变性）
- salt 需要在 `set_outer_key_from_handshake_hash` / `promote_outer_key_after_next_seal`
  时计算并存储

**修正**：在 `OuterSessionState` 中新增：
```rust
pub struct OuterSessionState {
    // ... existing fields ...
    nonce_counter: AtomicU64,
    nonce_salt: [u8; 4],  // 在 set_outer_key / promote 时从 handshake_hash 派生
}
```
- `fork_for_connection`（`stealth.rs:533-538`）时 `nonce_counter = AtomicU64::new(0)`，`nonce_salt = [0; 4]`
- `fork_for_transport_delayed_transition`（`stealth.rs:544-556`）同样需要初始化
  `nonce_counter = AtomicU64::new(0)`，`nonce_salt = [0; 4]`——**两个 fork 路径都不能遗漏**
- `set_outer_key_from_handshake_hash` 时计算 salt：
  `hkdf_sha256(handshake_hash, b"et-outer-nonce-salt")[..4]`
- 计数器在每次 `seal_datagram`（Outer phase）时 `fetch_add(1, Relaxed)`
- Gate phase 仍用 `OsRng`（gate 不是热路径，且 gate key 每次不同）

### 问题 7：`open_datagram` Outer phase 的 in-place 问题 — P2 设计遗漏

**现状**：P2 方案说 Outer phase 用 in-place，gate phase 保留原 `open()`。
但 `open_datagram:882-883` 的 Outer 分支是：
```rust
match *self.key_phase.read().unwrap() {
    OuterKeyPhase::Outer(key, _) => return open(&key, buf),
    ...
}
```
当前 `open(&key, buf)` 接收 `&[u8]`，返回 `Vec<u8>`。

**问题**：如果改为 `open_inplace`，`open_datagram` 的签名需要从 `buf: &[u8]`
改为 `buf: &mut Vec<u8>`（或类似），这会影响所有调用方（TCP/UDP/WebSocket/QUIC/FakeTCP）。
但 `open_datagram` 的调用方有些传入的是 `&[u8]` slice（如 `udp.rs:686` 的 `raw: &BytesMut`
取 `&raw[..]`），改为 `&mut` 需要上游也改。

**修正**：
- 方案 A：`open_datagram` 保持 `&[u8]` 签名，内部 `open_inplace` 时先 `to_vec()` 再
  in-place——但这没有减少分配
- 方案 B：新增 `open_datagram_inplace(&self, buf: &mut Vec<u8>)` 方法，仅 Outer phase
  用 in-place，gate phase fallback 到 `open()` + clone。调用方按需迁移
- **推荐方案 B**：渐进式迁移，不破坏现有 API

### 问题 8：`OuterCipher` 的 `Debug` impl 遗漏

**现状**：`OuterSessionState` 派生了 `Debug`（`stealth.rs:385`），`OuterKeyPhase`
派生了 `Debug`（`stealth.rs:421`）。如果 `OuterCipher` 包含 `LessSafeKey`（不实现
`Debug`），需要手动 impl `Debug` for `OuterCipher` 和 `OuterKeyPhase`。

**修正**：手动 impl `Debug` for `OuterCipher`，不暴露 key 内容：
```rust
impl Debug for OuterCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Legacy { .. } => write!(f, "OuterCipher::Legacy"),
            Self::Aes256Gcm(..) => write!(f, "OuterCipher::Aes256Gcm"),
            Self::ChaCha20Poly1305(..) => write!(f, "OuterCipher::ChaCha20Poly1305"),
        }
    }
}
```

### 问题 9：QUIC `seal_gate_datagram` 不经过 `seal_datagram` — P4 遗漏

**现状**：QUIC 的 `seal` 方法（`quic.rs:351-357`）在 outer key 未到 delay 时调用
`seal_gate_datagram`，到达后调用 `seal_datagram`。P4 只优化了 `seal_datagram` 的
读锁路径，但 `seal_gate_datagram` 不经过 `seal_datagram`，不受 P4 影响。

**影响**：无问题。`seal_gate_datagram` 始终用 gate key（每次派生），不在 Outer 热路径。
P4 只优化 Outer phase 是正确的。但方案应明确 `seal_gate_datagram` 不受 P4 影响。

### 问题 10：`open_datagram` 已用读锁 — P4 描述不完整

**现状**：`open_datagram:882` 已经用 `self.key_phase.read().unwrap()`。P4 只需优化
`seal_datagram` 的写锁。方案中 P4 只提到 `seal_datagram`，是正确的，但应明确
`open_datagram` 不需要改动（已经是读锁）。

### 修正汇总

| 问题 | 影响优先级 | 修正 |
| --- | --- | --- |
| 1. ring feature 依赖 | P1 编译失败 | 新增 `stealth-aead` feature + `#[cfg]` 条件编译 fallback（方案 A+C 组合） |
| 2. OuterKeyPhase Copy 约束 | P1 架构阻碍 | **主方案**：并行 `RwLock<Option<OuterCipher>>` 字段，`key_phase` 保持 Copy；备选：嵌入枚举 |
| 3. SHA-256 非 0% | P1 表述不准 | 改为"数据路径 SHA-256 降到 0%" |
| 4. cipher suite 传递路径 | P1 实施遗漏 | 扩展 set/promote 签名 + NoiseHandshakeResult |
| 5. PromoteAfterNextSeal cipher | P1 实施遗漏 | 补充签名改动说明 |
| 6. nonce 计数器位置 | P3 设计遗漏 | 新增 AtomicU64 + salt 字段；**两个 fork 路径**都需初始化 |
| 7. open_datagram in-place | P2 实施遗漏 | 新增 open_datagram_inplace 方法，渐进迁移 |
| 8. OuterCipher Debug | P1 编译失败 | 手动 impl Debug |
| 9. QUIC seal_gate_datagram | 无问题 | 明确不受 P4 影响 |
| 10. open_datagram 已读锁 | 无问题 | 明确不需要改动 |

## 第三轮复审发现

### 问题 11：P1 改动细节与自审主方案矛盾 — P1 正文未更新

**现状**：P1 正文（第 60-66 行）仍展示 `OuterKeyPhase::Outer([u8; 32], OuterCipher, Instant)`
枚举定义，但自审问题 2 已将主方案改为并行 `RwLock<Option<OuterCipher>>` 字段。

**影响**：实施者读 P1 正文会按嵌入枚举方案编码，与自审主方案冲突。

**修正**：P1 正文第 2 点应改为：
> 在 `OuterSessionState` 中新增 `outer_cipher: RwLock<Option<OuterCipher>>` 字段，
> 与 `key_phase` 并行管理。`set_outer_key_from_handshake_hash` 时同时写 `key_phase`
> 和 `outer_cipher`。`seal_datagram` / `open_datagram` 读 `key_phase` 判断 phase，
> 读 `outer_cipher` 获取 cipher。

P1 正文第 4 点应改为：
> `seal_datagram()` / `open_datagram()` 读 `outer_cipher` 字段获取 `OuterCipher`，
> 有 cipher 时用 AEAD，无 cipher（gate phase）时走 legacy `seal()`/`open()`

### 问题 12：P4 代码示例与主方案矛盾 — P4 正文未更新

**现状**：P4 代码示例（第 146-161 行）展示从 `OuterKeyPhase::Outer(key, ref cipher, _)`
中取 `cipher.clone()`，但主方案中 cipher 在并行字段 `outer_cipher` 中，不在 `OuterKeyPhase`。

**修正**：P4 代码示例应改为：
```rust
pub fn seal_datagram(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
    // 快速路径：读锁
    let phase = self.key_phase.read().unwrap();
    if let OuterKeyPhase::Outer(key, _) = *phase {
        drop(phase);
        let cipher = self.outer_cipher.read().unwrap();
        if let Some(c) = cipher.as_ref() {
            return Some(seal_aead(c, &key, plaintext));
        }
        // cipher 为 None：feature 未启用或协商降级到 legacy
        return Some(seal(&key, plaintext));
    }
    drop(phase);
    // 慢速路径：写锁（仅 Gate → PromoteAfterNextSeal → Outer 转换时）
    let mut phase = self.key_phase.write().unwrap();
    // ... 原有逻辑 ...
}
```

注意：并行方案下 `OuterCipher` 不需要 `Clone`（读锁直接引用），简化了实现。
P4 正文第 164-165 行关于 `Clone` 的说明应删除或改为"并行方案下不需要 Clone"。

### 问题 13：`disabled()` 构造函数遗漏初始化 — 问题 6 补充

**现状**：问题 6 列举了 `fork_for_connection` 和 `fork_for_transport_delayed_transition`
两个 fork 路径，但 `OuterSessionState` 有 **4 个构造函数**：
- `new()`（`stealth.rs:430-438`）
- `disabled()`（`stealth.rs:442-450`）
- `fork_for_connection()`（`stealth.rs:533-538`）— 调用 `new()`
- `fork_for_transport_delayed_transition()`（`stealth.rs:544-556`）— 直接构造

`fork_for_connection` 调用 `new()`，所以只需改 `new()` 和 `disabled()` 和
`fork_for_transport_delayed_transition()`。但 `disabled()` 也直接构造 struct，
需要初始化 `nonce_counter` 和 `nonce_salt`（虽然 disabled 状态下不会使用）。

同样，新增的 `outer_cipher` 字段也需要在所有 4 个构造函数中初始化为 `None`。

**修正**：问题 6 应补充：
- `new()`（`stealth.rs:430`）：`nonce_counter = AtomicU64::new(0)`，
  `nonce_salt = [0; 4]`，`outer_cipher = RwLock::new(None)`
- `disabled()`（`stealth.rs:442`）：同上（disabled 状态下不会使用，但 struct
  必须完整初始化）
- `fork_for_transport_delayed_transition()`（`stealth.rs:544`）：同上

### 问题 14：P1 正文第 32 行 feature flag 描述与自审矛盾

**现状**：P1 正文第 32 行写"复用已有 `ring` 依赖（`wireguard` feature 已在 `default`
中启用 `ring`）"，但自审问题 1 已改为新增 `stealth-aead` feature + 条件编译。

兼容性设计第 226-228 行也写"不需要编译时 feature flag。`ring` 已在 `default`
features 中"，与自审问题 1 矛盾。

**修正**：
- P1 第 32 行改为"复用已有 `ring` 依赖，通过新增 `stealth-aead` feature 启用"
- 兼容性设计第 226-228 行改为"需要新增 `stealth-aead` feature（加入 `default`
  和 `full`）。无该 feature 时 `stealth.rs` 通过 `#[cfg]` fallback 到 legacy
  HMAC-SHA256。运行时协商确保旧二进制自动降级"

### 问题 15：P1 正文第 84 行 SHA-256 "0%" 表述未更新

**现状**：P1 正文第 84 行仍写"SHA-256 从 63.7% 降到 **0%**（AEAD 彻底消除所有
SHA-256 计算）"，但自审问题 3 已修正为"数据路径 SHA-256 降到 0%"。

实施建议第 237 行也写"SHA-256 从 63.7% 降到 **0%**"。

**修正**：
- P1 第 84 行改为"数据路径 SHA-256 从 63.7% 降到 **0%**（AEAD 彻底消除数据路径中
  的 SHA-256 计算；连接建立和 gate 阶段仍有少量 SHA-256）"
- 实施建议第 237 行改为"数据路径 SHA-256 从 63.7% 降到 **0%**"

### 第三轮修正汇总

| 问题 | 位置 | 修正 |
| --- | --- | --- |
| 11. P1 枚举定义与主方案矛盾 | P1 正文 60-66 行 | 改为并行字段描述 |
| 12. P4 代码示例与主方案矛盾 | P4 正文 146-165 行 | 改为读 `outer_cipher` 字段，删除 Clone 说明 |
| 13. disabled() 遗漏初始化 | 问题 6 | 补充 `new()` / `disabled()` / `fork_for_transport_delayed_transition()` |
| 14. feature flag 描述矛盾 | P1 第 32 行 + 兼容性 226-228 行 | 改为 `stealth-aead` feature |
| 15. SHA-256 "0%" 表述未更新 | P1 第 84 行 + 实施建议 237 行 | 改为"数据路径 SHA-256 降到 0%" |

## 附录：全代码库加密路径审计（2026-07-09）

### A. 加密路径总览

EasyTier 有以下加密层，按数据流顺序：

| 层 | 模块 | 算法 | 热路径？ | profiling 占比 |
| --- | --- | --- | --- | --- |
| 1. PeerManager 压缩+加密 | `peer_manager.rs` | zstd + AES-GCM/XOR | 仅非 secure_mode | 未采样（secure_mode 跳过） |
| 2. PeerSession 加密 | `secure_datagram.rs` | AES-GCM/ChaCha20 | 是（secure_mode） | 0.3% (_aesni_ctr32_ghash_6x) |
| 3. Stealth outer 加密 | `tunnel/stealth.rs` | HMAC-SHA256 流密码 | 是（stealth_mode） | ~70% (sha256::compress256) |
| 4. QUIC transport crypto | `tunnel/quic.rs` crypto | SeaHasher checksum | 是（QUIC tunnel） | 未采样（本次测试用 TCP） |
| 5. Noise handshake | `peer_conn.rs` | Noise XX | 仅握手阶段 | 不是热路径 |

### B. 各路径详细审计

#### B.1 PeerManager: `try_compress_and_encrypt`

`src/peers/peer_manager.rs:2122-2137`

```rust
pub async fn try_compress_and_encrypt(...) {
    compressor.compress(msg, compress_algo).await?;
    if !secure_mode_enabled {
        encryptor.encrypt(msg)?;
    }
}
```

**关键发现**：当 `secure_mode_enabled=true` 时，**跳过 PeerManager 层加密**。
加密由下游 `PeerSessionTunnelFilter` 处理。这是正确的设计——避免双重 PeerSession 加密。

但 `compress` 仍然每包执行。zstd 压缩/解压不在 profiling 热点中（未出现在 top 20），
说明压缩开销可接受，或测试场景下压缩率低（随机数据不可压缩，`compress` 检测后跳过）。

**潜在问题**：
- `compress_raw` 返回 `Vec<u8>`，每包一次分配
- `decompress_raw` 尝试 5 次逐渐增大 buffer，最坏情况 5 次分配
- 但这些不在 hot path 热点中，低优先级

**结论**：无性能问题。

#### B.2 PeerSession: `SecureDatagramSession`

`src/peers/secure_datagram.rs:708-767`

**encrypt_payload**：
1. `next_nonce()` → `now_ms()` (SystemTime) + `maybe_rotate_epoch()` (atomic ops)
2. `get_or_create_encryptor()` → `key_cache.lock()` (Mutex)，缓存命中时不调 `hkdf_traffic_key`
3. `encryptor.encrypt_with_nonce()` → AES-GCM（AES-NI 硬件加速）

**decrypt_payload**：
1. `parse_tail()` → 提取 nonce
2. `now_ms()` → SystemTime
3. `precheck_replay()` → `rx_slots.lock()` + 可能 `sync_rx_grace.lock()`
4. `get_or_create_encryptor()` → `key_cache.lock()`
5. `encryptor.decrypt()` → AES-GCM
6. `commit_replay()` → `rx_slots.lock()` + 可能 `sync_rx_grace.lock()` + `prune_key_cache()` → `key_cache.lock()`

**profiling 数据**：
- `_aesni_ctr32_ghash_6x`：0.3% — AES-GCM 本身不是瓶颈
- `SecureDatagramSession::decrypt_payload`：0.1%
- `__lock` + `__unlock`：0.8% 合计 — 锁竞争不是瓶颈
- `hkdf_traffic_key` 未出现在 top 20 — key cache 命中率高，HKDF 仅在 epoch rotation 时调用

**潜在优化**（低优先级）：
- `precheck_replay` 和 `commit_replay` 各自独立加锁 `rx_slots`，可合并为单次锁
- `commit_replay` 内调用 `prune_key_cache` 额外加锁 `key_cache`，但仅在 epoch 切换时执行
- `now_ms()` 使用 `SystemTime::now()` → `duration_since(UNIX_EPOCH)`，每包 3 次调用
  （profiling 中 `__vdso_clock_gettime` 0.05%，不是瓶颈）
- `get_or_create_encryptor` 返回 `Arc<dyn Encryptor>` 的 clone（引用计数原子操作），每包 1 次

**结论**：PeerSession 加密本身效率良好，AES-NI 硬件加速有效。锁竞争和 SystemTime
开销在 profiling 中占比很小，不是当前瓶颈。低优先级优化。

#### B.3 Stealth outer: `tunnel/stealth.rs`

`src/tunnel/stealth.rs:749-832`

**这是已确认的主要瓶颈**（profiling 70% CPU）。

**seal()** 每包操作：
1. `outer_subkeys(key)` → 2× `hkdf_sha256()` → 4× HMAC-SHA256 — **每包重复，应缓存**
2. `OsRng.fill_bytes(&mut nonce)` → `getrandom()` 系统调用 — **每包一次**
3. `Vec::with_capacity()` + `extend_from_slice()` — **每包一次分配 + 两次拷贝**
4. `apply_keystream(enc_key, nonce, data)` → 每 32 字节一次完整 HMAC-SHA256 — **主要瓶颈**
5. `outer_mac(mac_key, nonce, ciphertext)` → 1× HMAC-SHA256

**open()** 每包操作：
1. `outer_subkeys(key)` → 4× HMAC-SHA256 — **同上，重复计算**
2. `outer_mac()` 验证 → 1× HMAC-SHA256
3. `ciphertext.to_vec()` — **额外一次分配 + 拷贝**
4. `apply_keystream()` 解密 → 同 seal

**调用方**（所有 stealth-enabled transport）：
- TCP: `tunnel/common.rs:332` (seal) / `tunnel/common.rs:241` (open)
- UDP: `tunnel/udp.rs:345` (seal) / `tunnel/udp.rs:686` (open)
- FakeTCP: `tunnel/fake_tcp/mod.rs:778` (open)
- WebSocket: `tunnel/websocket.rs:171` (seal) / `tunnel/websocket.rs:202` (open)
- QUIC: `tunnel/quic.rs:354` (seal via `seal_gate_datagram` or `seal_datagram`) / `tunnel/quic.rs:362` (open)

**结论**：主要瓶颈。优化方案见上文优先级 1-4。

#### B.4 QUIC transport crypto

`src/tunnel/quic.rs:38-138`

使用 `SeaHasher` checksum 代替标准 TLS：
- `HeaderKey::encrypt/decrypt`：空操作（no-op）
- `PacketKey::encrypt`：计算 header + payload 的 SeaHasher checksum，追加 8 字节 tag
- `PacketKey::decrypt`：校验 checksum tag

**性能分析**：
- SeaHasher 非常快（~3 cycles/byte），不是瓶颈
- 但这意味着 QUIC 传输层**没有真正加密**，安全性完全依赖上层 PeerSession + stealth
- QUIC 的性能瓶颈是 per-packet 处理模型（5200 次/s QUIC stream write + UDP send），
  不是 crypto

**结论**：无加密性能问题。但存在安全设计问题——QUIC 层无真正加密（已记录在 known_bugs）。

#### B.5 Noise handshake

`src/peers/peer_conn.rs:980-1143`

Noise XX handshake 仅在连接建立时执行，不在数据热路径中。
- `snow::Builder` → `HandshakeState` → 3 次消息交换
- 握手完成后生成 `handshake_hash` → `derive_outer_key()` → 安装到 `OuterSessionState`
- 握手完成后生成 `root_key` → `SecureDatagramSession`

**结论**：不是热路径，无性能问题。

#### B.6 RelayPeerMap 加密

`src/peers/relay_peer_map.rs:176-179, 216-217, 611-634`

Relay 路径的加密：
- `send_msg()`: `session.encrypt_payload(my_peer_id, dst_peer_id, &mut msg)` — 使用 PeerSession
- `decrypt_if_needed()`: `session.decrypt_payload(from_peer_id, my_peer_id, packet)` — 使用 PeerSession

**关键发现**：RelayPeerMap 使用与 direct p2p 相同的 `PeerSession` 加密机制。
当 relay 数据包到达最终接收方时，`start_peer_recv` 中的 `relay_peer_map.decrypt_if_needed()`
执行解密。但 relay 路径的包**不经过 stealth seal/open**（stealth 只在 transport 层，
relay 包在 PeerManager 层转发）。

**潜在问题**：
- relay 包可能经过多次 PeerSession 加密/解密（每跳一次），但这是协议设计要求
- `decrypt_if_needed` 每包加锁 `states` (DashMap entry) + `session.lock()`

**结论**：使用标准 PeerSession 加密，性能特征与 direct p2p 相同。不是额外瓶颈。

#### B.7 OpenSSL cipher 实现

`src/peers/encrypt/openssl.rs`

**潜在问题**：
- `encrypt_with_nonce()` 每包创建 `vec![0u8; payload_len + cipher.block_size()]` 临时 buffer
- `decrypt()` 每包创建 `vec![0u8; text_len + cipher.block_size()]` 临时 buffer
- 每包 2 次 `Crypter::new()` + `update()` + `finalize()`（创建 OpenSSL cipher context）

但 OpenSSL backend 仅在 `--features openssl-crypto` 时使用，默认使用 `ring` 或 `aes-gcm` crate。
profiling 中 `_aesni_ctr32_ghash_6x`（ring 的 AES-GCM 实现）占 0.3%，说明 ring backend 效率良好。

**结论**：OpenSSL backend 有额外内存分配，但默认不使用。ring/aes-gcm backend 无问题。

#### B.8 XOR cipher

`src/peers/encrypt/xor.rs`

逐字节 XOR，无 nonce/tag 开销。仅用于 `--encryption-algorithm xor` 或调试。
性能不是问题（O(n) 简单操作），但安全性极低。

**结论**：无性能问题。

#### B.9 Compressor

`src/common/compressor.rs`

zstd 压缩/解压使用 thread-local `DashMap` 缓存 `Compressor`/`Decompressor` context。
- `compress_raw` 返回 `Vec<u8>` — 每包一次分配
- `decompress_raw` 尝试 5 次逐渐增大 buffer — 最坏 5 次分配
- 但压缩/解压不在 profiling 热点中

**潜在问题**：
- `compress` 中 `compress_raw` 返回 `Vec<u8>` 后比较长度，如果压缩后更大则丢弃——浪费了一次压缩 + 分配
- `decompress_raw` 的 5 次重试机制在极端情况下可能浪费 CPU

**结论**：不在热点中，低优先级。可优化为先检查数据可压缩性再压缩。

### C. 审计总结

| 路径 | 是否瓶颈 | 优先级 | 说明 |
| --- | --- | --- | --- |
| Stealth outer `apply_keystream` | **是（主要）** | P1 | HMAC-SHA256 流密码，占 70% CPU |
| Stealth outer `outer_subkeys` | **是（次要）** | P1 | 每包重复 HKDF，应缓存 |
| Stealth `seal()`/`open()` 内存分配 | **是（次要）** | P2 | 每包 Vec 分配 + 多次拷贝 |
| Stealth `OsRng` nonce 生成 | 次要 | P3 | 每包一次 getrandom() 系统调用 |
| PeerSession AES-GCM | 否 | — | 0.3% CPU，AES-NI 加速 |
| PeerSession 锁竞争 | 否 | P5 | 0.8%，可合并但非瓶颈 |
| PeerSession `now_ms()` | 否 | — | 0.05%，vdso 加速 |
| QUIC crypto | 否 | — | SeaHasher 很快，无真正加密 |
| Noise handshake | 否 | — | 仅握手阶段 |
| RelayPeerMap 加密 | 否 | — | 复用 PeerSession |
| OpenSSL backend | 否 | — | 默认不使用，有额外分配 |
| Compressor | 否 | — | 不在热点中 |
| XOR cipher | 否 | — | 简单 O(n) |

**最终结论**：所有加密性能问题集中在 **stealth outer 加密层**（`tunnel/stealth.rs`）。
其他加密路径（PeerSession AES-GCM、QUIC crypto、Noise、RelayPeerMap）均无性能问题。
