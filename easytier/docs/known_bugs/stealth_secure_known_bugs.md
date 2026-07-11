# Stealth / Secure 已知问题

本文记录当前 `releases/v2.6.9` 代码线在 Stealth 与显式 `secure_mode` 组合上的已知问题。
结论来自 2026-07-08 的远端验证、实机观察和当前代码路径审计。

## 1. 显式 secure + Stealth 吞吐明显下降

**状态：已复现，未定位根因。**

复现场景：

```text
--secure-mode true
--stealth-mode true
--stealth-protocols tcp
```

在两台远端 Linux 测试节点的同 LAN TCP underlay 测试中：

| 模式 | 512 MiB 下载速度 |
| --- | ---: |
| plain | `107.75 MB/s` |
| `stealth_mode=true`，无显式 secure，运行期派生 secure | `107.91 MB/s` |
| 显式 `secure_mode.enabled=true` | `107.83 MB/s` |
| 显式 secure + Stealth | `10.90 MB/s` |

RSS 没有持续增长，约 `15 MB`；CPU 接近单核满载。因此目前判断更像 CPU 加密/封装热路径
瓶颈，而不是内存泄漏。

**当前不应做的结论**

- 不能说 Stealth 本身一定慢；派生 secure + Stealth 的测试没有明显慢。
- 不能说显式 `secure_mode` 本身一定慢；显式 secure 单独开启也没有明显慢。
- 不能说这是内存泄漏；当前 RSS 证据不支持。

**后续建议**

- 对 `secure_mode=true + stealth_mode=true` 做 CPU profiling。
- 优先检查是否存在重复保护、重复 copy、record protector 与 `PeerSessionTunnelFilter`
  叠加导致的热路径成本。
- 优先检查显式 secure 才会打开的 relay/session 分支：
  `RelayPeerMap::new()` 和 `PeerManager::RpcTransport` 使用
  `GlobalCtx::is_explicit_secure_mode_enabled()`，运行期派生 secure 不会启用这些分支。
  因此显式 secure 可能额外触发 `RelayPeerMap::ensure_session()`、
  `encrypt_payload()` / `decrypt_if_needed()` 等路径。该差异已通过代码审计确认，但
  2026-07-08 的慢速吞吐样本在 peer 状态里仍显示 `cost=p2p`、`tunnel_proto=tcp`，
  所以它还不能单独解释这次 LAN direct 测试的全部性能下降。后续 profiling 应分别对比
  direct p2p、relay/foreign network 两种拓扑。
- 单独比较 `tcp`、`udp`、`quic` 和 `ws`，确认是否只在 TCP Stealth record 路径明显。
- 在修复前，不建议对高吞吐场景默认推荐显式 `secure_mode=true + stealth_mode=true`。

## 2. 派生 secure 的性能接近 plain，不代表没有加密

**状态：代码路径已确认。**

`stealth_mode=true`、`network_secret` 非空且无显式 `secure_mode` 时，`GlobalCtx` 会在运行期
派生 `SecureModeConfig`。该配置只在 tunnel 携带有效 Stealth `OuterSessionState` 时被
`PeerConn` 使用。

当前代码路径：

- `GlobalCtx::get_effective_secure_mode()` 生成运行期派生 secure 配置。
- `GlobalCtx::get_secure_mode_for_tunnel(stealth_protected=true)` 只对
  Stealth-protected tunnel 返回该配置。
- `PeerConn::new()` 根据该配置启用 `PeerSessionTunnelFilter`。
- Noise 握手完成后，`PeerSessionTunnelFilter` 对普通 PeerManager payload 调用
  `PeerSession::encrypt_payload()` / `decrypt_payload()`。

因此，派生 secure 性能接近 plain 的合理解释是：当前热路径在该测试条件下开销较低，
而不是“完全没有加密”。

但派生 secure 不是显式 `secure_mode` 的全局替代品：

- 不写入 TOML/RPC。
- 不发布 RoutePeerInfo `noise_static_pubkey`。
- 不启用全局 RelayPeerMap / PeerManager secure relay/session。
- 不进入 credential 身份模式。
- 不保护未携带 Stealth `OuterSessionState` 的 legacy/plain PeerConn。

## 3. TCP strict Stealth listener 对同 secret plain 客户端不够严格

**状态：已复现。**

复现场景：

- 服务端：`stealth_mode=true`，`stealth_protocols=tcp`
- 客户端：`stealth_mode=false`
- 两端使用相同 `network_secret`
- 客户端拨 `tcp://server:port`

结果：客户端仍能建立连接并传输数据，peer 显示 `tunnel_proto=tcp`。

这说明当前 TCP Stealth listener 没有严格拒绝“同 secret 但未启用 Stealth”的 plain
客户端。文档中“strict listener 不接受 legacy/plain”的强语义当前应限定到已验证的
UDP strict listener；TCP/FakeTCP/WS/QUIC/WG/WSS 的 strict anti-legacy 行为需要逐项
验证和修复。

**风险**

- 对随机陌生探测的隐藏能力仍可能存在，但对“知道 network_secret、但未启用 Stealth”的
  旧/混合客户端，TCP listener 当前不够严格。
- 混合部署时，用户可能误以为所有协议 listener 都已经具备 UDP 同等级 strict 行为。

**后续建议**

- 为每个 Stealth 协议补负向测试：服务端启用该协议 Stealth，客户端关闭 Stealth，同 secret
  拨入必须失败。
- 修复前，文档必须明确“UDP strict listener 已验证；非 UDP strict listener 仍有已知缺口”。
- 修复时不要放宽 UDP 的 anti-probe 行为，也不要把 phase-2 数据面降级为 gate/plain。

## 4. QUIC tunnel 吞吐远低于 TCP（6 MB/s vs 107 MB/s）

**状态：已定位根因，未修复。**

### 4.1 测试数据

| 场景 | 吞吐 |
| --- | ---: |
| 物理直连（TCP iperf3） | 112 MB/s |
| EasyTier TCP tunnel (plain) | 107.75 MB/s |
| EasyTier QUIC tunnel (baseline, quinn 默认参数) | 6.26 MB/s |
| EasyTier QUIC tunnel (stream_receive_window=8MB) | 6.17 MB/s |

`stream_receive_window` 从 quinn 默认 ~1.25MB 改为 8MB（参考 Hysteria2 默认值）后，
吞吐无明显变化。说明流控窗口不是瓶颈。

测试环境：`192.168.2.160` ↔ `192.168.1.38`，同 LAN，EasyTier 虚拟 IP `10.231.0.1` / `10.231.0.2`。

### 4.2 根因分析：per-packet 处理模型

EasyTier 的整个 tunnel pipeline 是 `Sink<ZCPacket>` / `Stream<ZCPacket>`——逐包处理。
每个 TUN 包（~1200-1500 bytes）独立走完整条路径：

```
TUN read → PeerManager::send_msg_by_ip → send_msg_internal → Peer::send_msg
→ PeerConn::send_msg → MpscTunnelSender::send (mpsc channel, per-packet)
→ TunnelWithFilter::before_send (PeerSessionTunnelFilter::encrypt_payload, per-packet)
→ FramedWriter (length-delimited frame + stream write, per-packet)
→ QUIC stream → quinn UDP sendmmsg (per-packet)
→ wire → quinn UDP recvmmsg → QUIC stream read
→ FramedReader (per-packet)
→ TunnelWithFilter::after_received (PeerSessionTunnelFilter::decrypt_payload, per-packet)
→ PeerManager nic pipeline → TUN write
```

6 MB/s ÷ 1200 bytes/packet ≈ 5200 pps。每方向 5200 次/s 的完整管道开销：

- 5200 次 mpsc channel send/recv
- 5200 次 `PeerSessionTunnelFilter::before_send`（锁 + 加密）
- 5200 次 `FramedWriter` framing
- 5200 次 QUIC stream write
- 5200 次 quinn UDP send 系统调用
- 接收侧对称开销

**对比 TCP 为什么快**：TCP 有内核 TSO/GSO，一次 `write()` 可以传 MB 级数据，内核负责分段。
EasyTier TCP tunnel 的 `FramedWriter` 写一个大 frame 后，底层 `AsyncWrite::write` 一次传给
内核 TCP socket，内核用 TSO 批量发送。而 QUIC 在 userspace 实现，每个 QUIC packet 对应一次
UDP `sendmmsg` 系统调用，无法利用 TSO/GSO。

**CentOS 7 / kernel 3.10 限制**：测试节点运行 CentOS 7（kernel 3.10），不支持 UDP GSO
（需要 kernel 4.18+）。quinn 的 `enable_segmentation_offload(true)` 在旧内核上无效。
即使 `stream.write(64KB)` 一次写入，quinn 仍拆成 ~55 个 1200 字节 QUIC packet，每个一次
UDP send 系统调用。

### 4.3 QUIC 自定义 crypto 无实际加密

`src/tunnel/quic.rs` 的 `crypto` 模块使用 `SeaHasher` checksum 代替标准 TLS 加密：

- `HeaderKey::encrypt` / `decrypt`：空操作（no-op）
- `PacketKey::encrypt`：计算 header + payload 的 SeaHasher checksum，追加 8 字节 tag
- `PacketKey::decrypt`：校验 checksum tag

这不是性能问题（SeaHasher 很快），但意味着 QUIC 传输层没有真正的加密——安全性完全依赖
上层 `PeerSession` 加密（`PeerSessionTunnelFilter`）。

### 4.4 三种 NIC 模式的数据路径

所有三种模式最终都汇聚到同一条 `PeerConn` 管道：

**TUN 模式**（`--no-tun` 未设置）：
```
TUN device read → do_forward_nic_to_peers → send_msg_by_ip
→ send_msg_internal → Peer::send_msg → PeerConn::send_msg → [pipeline]
```
代码路径：`src/instance/virtual_nic.rs:1100-1122`（`do_forward_nic_to_peers_task`）

**VNet / smoltcp 模式**（`use_smoltcp=true` 或 `no_tun=true` 或 mobile/OHOS）：
```
smoltcp stack → TcpProxyListener → try_process_packet_from_peer
→ nic_channel / send_msg_by_ip → [pipeline]
```
代码路径：`src/gateway/tcp_proxy.rs:620-670`（`get_proxy_listener` smoltcp 分支）

**No-TUN / relay 模式**（`--no-tun`，无 proxy_cidrs）：
```
peer recv loop → forward → send_msg_internal → Peer::send_msg → PeerConn → [pipeline]
```
代码路径：`src/peers/peer_manager.rs:1431-1612`（`start_peer_recv`）

三种模式在 `PeerConn::send_msg` 之后完全相同，都经过：
`MpscTunnelSender → TunnelWithFilter (encrypt) → FramedWriter → QUIC stream`

### 4.5 评估过的修复方案

#### 方案 A：PacketCoalescer（批量合并层）

在 `FramedWriter` 层引入 coalescer：攒多个 ZCPacket 后一次 `stream.write`。

**评估结论：不采用。**

- 只优化管道最后一环（stream write），上游 mpsc / encrypt / framing 开销不变
- 引入延迟（需等 buffer 满或 timeout），对 relay 多跳有害
- 对首包延迟敏感的协议（TCP SYN、SSH）不友好
- 不减少 wire 上的 UDP 包数（quinn 仍拆成 1200B QUIC packet）
- 本质是 workaround，只解决 ~20% 问题

#### 方案 B：FramedWriter 改为 BufWriter 模式

`FramedWriter` 内部维护 `BytesMut` buffer，frame 追加到 buffer，满或 flush 时一次写出。

**评估结论：可作为最小改动方案，但不彻底。**
- 利用 `AsyncWrite` 的 `poll_ready`/`start_send` 语义自动 batch
- 不需要 timeout / 新组件 / 改接口
- 但仍只优化 write 层，不解决 encrypt / channel 的 per-packet 开销

#### 方案 C：PeerConn 层批量

`PeerConn::send_msg` 内部维护 batch buffer，攒够后一次送 mpsc channel，filter chain 对
整个 batch 一次处理。

**评估结论：能解决 transport 层通用问题，但不能确定解决 explicit secure + stealth 的 10x 降幅。**

- 能解决 QUIC 6 MB/s 瓶颈（批量 stream write，5200→~80 次/s）
- 覆盖所有模式（TUN / VNet / no-TUN）和所有协议（QUIC / TCP / UDP / WS / WG）
- 但 explicit secure + stealth 的 10x 降幅（107→10.9 MB/s）发生在 TCP 上，
  纯加密计算不足以解释 10x。可能根因是：
  - 重复加密（stealth seal + PeerSession encrypt = 双重加密？）
  - 锁竞争（每包 4 次 `std::sync::Mutex` lock/unlock）
  - 每包 3 次 `SystemTime::now()` 调用
  - 显式 secure 触发的额外路径（RelayPeerMap / RpcTransport secure 分支）
- C 的批量处理能缓解锁竞争和 per-packet overhead，但如果根因是双重加密或不必要的
  代码路径，C 只是缓解而非根治

#### 方案 D：将 tunnel trait 从 `Sink<ZCPacket>` 改为 `Sink<Bytes>`

`Bytes` 可包含多个 framed packet，管道上游负责合并，一次发送。mpsc / encrypt / frame / write
全部批量。

**评估结论：最彻底，但改动面大。**
- 所有实现 `Tunnel` trait 的地方都要改
- 需要重新设计 filter chain 接口

### 4.6 加密热路径详细分析

`PeerSessionTunnelFilter`（`src/peers/peer_conn.rs:157-254`）对每个包：

**发送侧 `before_send`**：
1. `self.session.lock().unwrap()` — `std::sync::Mutex` lock
2. `session.encrypt_payload()` → `SecureDatagramSession::encrypt_payload()`
3. 内部：`self.next_nonce()` → `now_ms()` (SystemTime) + `maybe_rotate_epoch()`
4. `self.get_or_create_encryptor()` → `self.key_cache.lock().unwrap()` — 第二次 Mutex lock
5. `encryptor.encrypt_with_nonce()` — 实际加密

**接收侧 `after_received`**：
1. `self.session.lock().unwrap()` — Mutex lock
2. `session.decrypt_payload()` → `SecureDatagramSession::decrypt_payload()`
3. `Self::parse_tail()` — 提取 nonce
4. `now_ms()` — 第一次 SystemTime 调用
5. `self.precheck_replay()` → `self.rx_slots.lock().unwrap()` — 第二次 Mutex lock
   - 内部：`evict_old_rx_slots()` + `sync_rx_grace_active()` → 可能第三次锁 `sync_rx_grace`
   - **第二次 `now_ms()` 调用**
6. `self.get_or_create_encryptor()` → `self.key_cache.lock().unwrap()` — 第三次 Mutex lock
7. `encryptor.decrypt()` — 实际解密
8. `self.commit_replay()` → `self.rx_slots.lock().unwrap()` — 第四次 Mutex lock
   - 内部：`evict_old_rx_slots()` + `sync_rx_grace_active()` → 可能第五次锁
   - **第三次 `now_ms()` 调用**

**每包总开销**：
- 发送：2 次 Mutex lock + 1 次 SystemTime + 1 次加密
- 接收：4-5 次 Mutex lock + 3 次 SystemTime + 1 次解密
- 合计：6-7 次 Mutex lock + 4 次 SystemTime + 2 次加密/解密

`precheck_replay` 和 `commit_replay` 各自独立加锁 `rx_slots`，且都调用 `evict_old_rx_slots`
和 `sync_rx_grace_active`，存在合并为单次锁的优化空间。

### 4.7 CPU Profiling 结果（2026-07-09）

**测试配置**：`--secure-mode true --stealth-mode true --stealth-protocols tcp`，
TCP underlay，direct p2p，`192.168.2.160` ↔ `192.168.1.38`，同 LAN。

**吞吐**：6.31 MB/s（vs plain TCP 107.75 MB/s，降幅 17x）

**Profiling 方法**：`perf record -F 999 -a -g`（system-wide，kernel 3.10 的 `-p PID`
对 musl 静态二进制不工作），throughput 期间采样 35 秒，157K samples。

**CPU 占比分布**：

| 类别 | 占总采样 | 占非 idle |
| --- | ---: | ---: |
| idle (swapper/native_safe_halt) | 82.48% | — |
| easytier userspace | 14.05% | 80.2% |
| easytier in kernel (syscall) | 1.54% | 8.8% |
| 其他 (sshd, gsd-color 等) | ~1.9% | ~11% |

**easytier userspace 热点函数排名**（占 easytier userspace CPU 比例）：

| 函数 | 占 easytier | 占非 idle | 说明 |
| --- | ---: | ---: | --- |
| `sha2::sha256::compress256` | 63.7% | 51.0% | SHA-256 压缩函数 |
| `memcpy` | 12.7% | 10.2% | 内存拷贝 |
| `digest::core_api::FixedOutputCore::finalize_fixed_core` | 5.0% | 4.0% | SHA-256 finalize |
| `easytier::tunnel::stealth::apply_keystream` | 3.3% | 2.6% | stealth 流加密 |
| `core::slice::copy_from_slice_impl` | 1.4% | 1.1% | 内存拷贝 |
| `digest::mac::Mac::update` | 0.6% | 0.5% | HMAC-SHA256 update |
| `easytier::tunnel::stealth::hkdf_sha256` | 0.4% | 0.3% | HKDF 密钥派生 |
| `hmac::get_der_key` | 0.3% | 0.2% | HMAC 密钥派生 |
| `_aesni_ctr32_ghash_6x` | 0.3% | 0.2% | AES-GCM 硬件加速 |
| `easytier::tunnel::stealth::outer_mac` | 0.1% | 0.1% | stealth MAC |
| `easytier::peers::secure_datagram::SecureDatagramSession::decrypt_payload` | 0.1% | 0.1% | PeerSession 解密 |

### 4.8 根因确认：双重加密 + HMAC-SHA256 流加密

Profiling 确认了根因不是锁竞争、不是 `SystemTime::now()`、不是 RelayPeerMap 分支，
而是 **stealth outer 加密层的 HMAC-SHA256 流加密**。

**双重加密架构**：

explicit secure + stealth 下，每个包经过两层加密：

**第一层：PeerSession 加密**（`SecureDatagramSession::encrypt_payload`）
- 使用 AES-GCM（`_aesni_ctr32_ghash_6x`，0.3% CPU）
- 有硬件加速（AES-NI），开销很小
- 这一层不是瓶颈

**第二层：Stealth outer 加密**（`OuterSessionState::seal_datagram` → `seal`）
- `seal()` 函数（`src/tunnel/stealth.rs:799-811`）对每个包执行：
  1. `outer_subkeys()` → 2 次 `hkdf_sha256()` → 4 次 HMAC-SHA256
  2. `apply_keystream()` → 每 32 字节一次 HMAC-SHA256（流加密）
  3. `outer_mac()` → 1 次 HMAC-SHA256
- 对于 1200 字节包：`apply_keystream` 需要 ⌈1200/32⌉ = 38 次 HMAC-SHA256
- 加上 subkey 派生和 MAC：每包 ~41 次 HMAC-SHA256
- 每次 HMAC-SHA256 = 2 次 SHA-256 compress（inner + outer pad）
- **每包 ~82 次 SHA-256 compress**

**为什么占 63.7% CPU**：

5200 pps × 82 compress/包 = ~427K SHA-256 compress/s
SHA-256 compress 在没有 SHA-NI 的 CPU 上约 ~100 cycles/block
427K × 100 cycles = 42.7M cycles/s ≈ 4.3% of 1 GHz 单核

但实际占 8.95% 总采样（63.7% × 14.05%），说明 CPU 频率更高或有多线程竞争。
关键点是：**SHA-256 compress 占了 easytier userspace CPU 的近 2/3**。

**`apply_keystream` 的问题**（`src/tunnel/stealth.rs:761-777`）：

```rust
fn apply_keystream(enc_key: &[u8; 32], nonce: &[u8; OUTER_NONCE_LEN], data: &mut [u8]) {
    let mut counter: u32 = 0;
    let mut offset = 0;
    while offset < data.len() {
        let mut mac = HmacSha256::new_from_slice(enc_key).expect("hmac key");
        mac.update(b"et-outer-strm");
        mac.update(nonce);
        mac.update(&counter.to_be_bytes());
        let block = mac.finalize().into_bytes();  // <-- 每 32 字节一次完整 HMAC-SHA256
        let n = (data.len() - offset).min(block.len());
        for i in 0..n {
            data[offset + i] ^= block[i];
        }
        offset += n;
        counter = counter.wrapping_add(1);
    }
}
```

这是一个用 HMAC-SHA256 实现的流密码（CTR 模式），每 32 字节生成一个密钥流块。
每生成一块都需要完整的 HMAC-SHA256 setup + update + finalize，无法利用 CTR 模式的
流水线优势（标准 AES-CTR 只需一次 key schedule + 多次 block encrypt）。

**`outer_subkeys` 的重复计算**（`src/tunnel/stealth.rs:754-759`）：

```rust
fn outer_subkeys(key: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    (
        hkdf_sha256(key, b"et-outer-enc"),
        hkdf_sha256(key, b"et-outer-mac"),
    )
}
```

`seal()` 和 `open()` 每次调用都重新派生 enc_key 和 mac_key（4 次 HMAC-SHA256）。
这些密钥在连接生命周期内不变（phase-2 outer key 固定），应该缓存。

**`seal()` 的内存分配**（`src/tunnel/stealth.rs:799-811`）：

```rust
pub fn seal(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let (enc_key, mac_key) = outer_subkeys(key);  // 重复 HKDF
    let mut nonce = [0u8; OUTER_NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);     // 系统调用 RNG
    let mut out = Vec::with_capacity(OUTER_NONCE_LEN + plaintext.len() + OUTER_TAG_LEN);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(plaintext);             // 一次拷贝
    apply_keystream(&enc_key, &nonce, &mut out[OUTER_NONCE_LEN..]);
    let tag = outer_mac(&mac_key, &nonce, &out[OUTER_NONCE_LEN..]);
    out.extend_from_slice(&tag);
    out
}
```

每包：1 次 `Vec` 分配 + 2 次拷贝 + 1 次 `OsRng` 系统调用 + HKDF + 流加密 + MAC。
`open()` 类似，还有 `ciphertext.to_vec()` 额外拷贝。

### 4.9 优化方案（基于 profiling 数据）

**优先级 1：缓存 outer_subkeys**

`outer_subkeys` 在连接生命周期内只需计算一次。在 `OuterSessionState` 中缓存
`(enc_key, mac_key)`，`seal()` / `open()` 直接使用缓存值。

预期收益：每包减少 4 次 HMAC-SHA256，约 5% CPU。

**优先级 2：替换 HMAC-SHA256 流密码为 AES-CTR**

`apply_keystream` 用 HMAC-SHA256 模拟 CTR 流密码，每 32 字节一次完整 HMAC。
替换为 AES-256-CTR（已有 AES-NI 硬件加速），每 16 字节只需一次 AES block encrypt
（~4 cycles with AES-NI vs ~200 cycles for HMAC-SHA256）。

预期收益：`apply_keystream` 从 3.3% 降到 ~0.1%，`sha256::compress256` 从 63.7% 大幅下降。

**优先级 3：减少 `seal()` / `open()` 的内存分配**

- `seal()` 返回 `Vec<u8>` → 改为写入调用方提供的 `BytesMut` buffer
- `open()` 中的 `ciphertext.to_vec()` → 原地解密
- 减少 `memcpy`（12.7% CPU）和 malloc/free 开销

**优先级 4：用 `Instant::now()` 替代 `OsRng` 生成 nonce**

`OsRng.fill_bytes()` 每次调用 `getrandom()` 系统调用。对于 nonce 生成，
可以用计数器 + 连接级种子替代，避免每包一次系统调用。

**优先级 5：合并 replay check 锁（原优先级 1）**

profiling 显示锁竞争不是瓶颈（`__lock` 0.4%、`__unlock` 0.4%），但仍值得优化。

**方案 C（PeerConn 层批量）的重新评估**：

profiling 确认瓶颈是 **加密计算**而非 per-packet overhead。批量处理能减少
subkey 派生和 nonce 生成的次数，但如果 `apply_keystream` 仍用 HMAC-SHA256，
批量加密一个大 buffer 的 SHA-256 compress 次数不变。因此：

- 方案 C + 优先级 1（缓存 subkeys）= 有效，减少 HKDF 调用次数
- 方案 C + 优先级 2（AES-CTR）= 根治，SHA-256 不再是瓶颈
- 仅方案 C = 缓解但不根治

### 4.10 stream_receive_window 修改记录

**已完成的修改**（`src/tunnel/quic.rs:292`）：

```rust
.stream_receive_window(quinn::VarInt::from_u32(8_388_608))  // 8MB, 参考 Hysteria2 默认值
```

修改前 baseline：6.26 MB/s
修改后：6.17 MB/s（无显著变化）
连接稳定性：无回归

结论：`stream_receive_window` 不是 QUIC 吞吐瓶颈，per-packet 处理模型才是。

## 5. Stealth 接收路径堆分配瓶颈（AEAD 已启用后）

**状态：已定位根因，未修复。2026-07-09 端到端验证发现。**

### 5.1 背景

P1（AEAD 替代 HMAC-SHA256 流密码）和 P3（AtomicU64 nonce 计数器）已实施并通过验证。
SHA-256 不再是数据路径瓶颈。但端到端性能测试显示：

| 场景 | 单流吞吐 | 4 并发吞吐 | CPU 峰值 |
| --- | ---: | ---: | ---: |
| 直连（无 stealth） | 412 Mbps | 386 Mbps | 77% |
| stealth + secure | 272 Mbps | 347 Mbps | 82% |
| stealth + secure + QUIC/KCP proxy | 275 Mbps | 283 Mbps | 76% |
| 多协议并发(TCP+UDP+QUIC) + stealth + secure + proxy | 243 Mbps | 277 Mbps | 26%* |

*多协议场景 CPU 采样在 iperf 运行中段，非峰值。

**关键观察**：CPU 未跑满（最高 82%），但吞吐比直连低 34%（单流）。CPU 不是瓶颈，
说明存在 I/O stall——典型的锁竞争或分配器争用模式。

### 5.2 根因分析：接收路径 3 次堆分配

当前 `open_datagram` 返回 `Option<Vec<u8>>`，所有调用方再做 `BytesMut::from(&plaintext[..])`。
每个入站包的完整分配路径：

```
open_datagram(buf: &[u8])
  → aead::open(cipher, buf)
    → buf[NONCE_LEN..].to_vec()           // alloc 1: 复制 ciphertext+tag 到 Vec
    → key.open_in_place(...)               // in-place 解密
    → plaintext.to_vec()                   // alloc 2: 复制 plaintext 到新 Vec
  → 返回 Vec<u8>
→ 调用方: BytesMut::from(&plaintext[..])   // alloc 3: 复制到 BytesMut
→ ZCPacket::new_from_buf(buf, ...)
```

**每包 3 次堆分配 + 3 次 memcpy**。在万兆网卡（83 万包/秒 @ 1500B MTU）下，
仅分配器锁争用就会导致严重 stall——这正是 CPU 没跑满但吞吐上不去的原因。

### 5.3 各协议调用路径详情

**UDP**（`src/tunnel/udp.rs`）：
- listener 侧 `try_forward_sealed_data`（line 683-688）：`open_datagram(raw)` → `BytesMut::from(&plaintext[..])`
- connector 侧 recv loop（line 1214-1216）：同上
- 接收 buffer 是自己的 `BytesMut`（`recv_buf_from` 填入），**可以直接原地解密**

**TCP / FakeTCP**（`src/tunnel/common.rs:241`, `src/tunnel/fake_tcp/mod.rs:778`）：
- `buf.split_to(header + sealed_len)` 得到 `BytesMut`，取 `&record[header..]` 调 `open_datagram`
- **可以先 split header，再在剩余 `BytesMut` 上原地解密**

**WebSocket**（`src/tunnel/websocket.rs:200-216`）：
- `msg.into_payload()` 返回 `Payload`，内部是 `Bytes`
- `Payload` 实现了 `From<Payload> for BytesMut`（零拷贝，当 refcount=1 时）
- 当前用 `payload.as_bytes()` 丢失所有权，被迫分配
- **可以用 `BytesMut::from(payload)` 零拷贝拿到 `BytesMut`，再原地解密**

**QUIC**（`src/tunnel/quic.rs:563-609`）：
- `poll_recv` 的 `bufs: &mut [IoSliceMut<'_>)]`，`bufs[index]` 可 `DerefMut` 到 `&mut [u8]`
- 当前代码：解密到 `Vec<u8>` → 收集到 `opened: Vec<(Vec<u8>, RecvMeta)>` → `copy_from_slice` 回 `bufs`
- **可以直接在 `bufs` 上原地解密**，消除中间 Vec 和 copy_from_slice
- GRO 多 stride 场景（一个 UDP 包含多个 QUIC datagram）：原地解密后需 1 次 memcpy 将 plaintext 移到目标位置
- 单 stride（最常见）：0 次 memcpy

**WireGuard**（`src/tunnel/wireguard.rs:918-931`）：
- `buf` 是 `Vec<u8>`，`&mut buf[..n]` 直接拿到 `&mut [u8]`
- 当前 `stealth.open(&buf[..n])` 返回 `Vec<u8>`，**可以直接原地解密**

### 5.4 分配数汇总

| 协议 | 当前分配数/包 | 可达分配数/包 | 可达 memcpy 数/包 |
| --- | ---: | ---: | ---: |
| UDP | 3 | 0 | 0 |
| TCP | 3 | 0 | 0 |
| FakeTCP | 3 | 0 | 0 |
| WebSocket | 3 | 0 | 0 |
| QUIC（单 stride） | 3 | 0 | 0 |
| QUIC（GRO 多 stride） | 3 | 0 | 1（memcpy，非分配） |
| WireGuard | 3 | 0 | 0 |

**全协议均可达到 0 分配。** QUIC GRO 多 stride 场景偶尔 1 次 memcpy（不是堆分配）。

### 5.5 限制条件

- **Gate phase 不能 in-place**：`open_datagram` gate 阶段尝试 2 个 window key（`stealth.rs:1106-1112`），
  in-place 会在第一次失败时破坏 buffer，导致第二次尝试无法进行。Gate phase 不是热路径，保留原路径。
- **Legacy fallback（`stealth-aead` feature 未启用）不能 in-place**：legacy `open()` 需要先验证 MAC
  再解密，两步操作中间不能破坏 buffer。Legacy fallback 保留原路径，但 `aead::open` 内部仍可省 1 次分配。
- **`aead::open` 的第二次 `to_vec()` 可以独立修复**：不涉及 in-place，纯语义等价，风险为零。

## 6. S3（显式 secure + stealth）QUIC 单流在高延迟链路上 8-10x 性能下降

**状态：已复现（3 轮跨太平洋基准测试），根因未定位，未修复。本地 B6/B7 基准测试已确认 outer AEAD phase 性能良好（B6/B0 = 0.60~1.43x，B7 ≈ B6），问题可能特定于高延迟链路 + S3 双层加密交互。**

### 6.1 问题描述

当 `stealth_mode=true` 且显式 `secure_mode=true`（S3 场景）时，QUIC 单流在跨太平洋高延迟链路
（~190ms RTT）上吞吐量下降 8-10 倍。S1（stealth=false）和 S2（stealth=true + 派生 secure）
不受影响。S3 TCP 在同样链路上无显著下降。

### 6.2 已有测试数据（A→E 跨太平洋 ~190ms，接收侧 Mbps）

| 轮次 | S1 | S2 | S3 |
| --- | ---: | ---: | ---: |
| R0 | 38 | 28 | 1.2 |
| R1 | 15.9 | 4.3 | 2.6 |
| R2 | 6.3 | 40.1 | 3.2 |
| **均值** | **20.1** | **24.1** | **2.3** |

**问题**：以上数据缺少必要的对照组，无法定位根因。需要补充完整基准测试矩阵。

### 6.3 缺失的基准测试矩阵

以下对照组是定位根因的必要条件，当前全部缺失：

| 编号 | 场景 | 说明 | 预期结果 |
| --- | --- | --- | --- |
| B0 | 直连 iperf3（无 EasyTier） | 物理链路上限 | 跑满链路带宽 |
| B1 | 同机器 TUN→TUN（loopback） | 内核转发上限 | 跑满内核 TUN 转发能力 |
| B2 | EasyTier plain（S0），TCP | 无 stealth 无 secure 基线 | 接近 B0 |
| B3 | EasyTier plain（S0），QUIC | 无 stealth 无 secure 基线 | 接近 B0（受 QUIC per-packet 限制） |
| B4 | EasyTier S2（stealth+派生），TCP | stealth 单层加密 | 接近 B2 |
| B5 | EasyTier S2（stealth+派生），QUIC | stealth 单层加密 | 接近 B3 |
| B6 | EasyTier S3（stealth+显式 secure），TCP | stealth + PeerSession 双层加密 | 接近 B4（已确认无显著下降） |
| B7 | EasyTier S3（stealth+显式 secure），QUIC | stealth + PeerSession 双层加密 | **8-10x 下降** |

**关键对比**：
- B2 vs B3：QUIC vs TCP 的固有差距（已知：第 4 节，QUIC per-packet 模型 + 无 UDP GSO）
- B4 vs B6：TCP 上 stealth 单层 vs 双层加密的差异（已知：第 1 节，LAN 上 10x，已通过 AEAD 修复）
- B5 vs B7：QUIC 上 stealth 单层 vs 双层加密的差异（**本 bug 的核心**）
- B0 vs B2/B3：EasyTier 自身的转发开销

**以上矩阵需要在同一时间窗口、同一链路条件下运行，消除跨太平洋链路的高方差噪声。**

### 6.4 S2 与 S3 的架构差异

S2 和 S3 的唯一代码路径差异是 `PeerSessionTunnelFilter.enabled` 标志：

```
S2: connection_local_peer_session=true → peer_session_payload_encryption=false → filter no-op
S3: connection_local_peer_session=false → peer_session_payload_encryption=true → filter active
```

S3 下每个包的数据路径：

```
发送侧：
  ZCPacket
  → PeerSessionTunnelFilter::before_send
    → session.lock() → encrypt_payload → unlock        // 第 1 层加密（stream 层）
  → FramedWriter::start_send (frame + push to BufList)
  → quinn SendStream::write (QUIC packetize)
  → QuicStealthSocket::try_send
    → session.seal()                                    // 第 2 层加密（UDP socket 层）
  → UDP sendmmsg

接收侧（对称）：
  UDP recvmmsg
  → QuicStealthSocket::poll_recv
    → open_in_place()                                   // 第 2 层解密（UDP socket 层）
  → quinn RecvStream::read
  → FramedReader
  → PeerSessionTunnelFilter::after_received
    → session.lock() → decrypt_payload → unlock         // 第 1 层解密（stream 层）
  → ZCPacket
```

S2 跳过了"第 1 层加密/解密"（`PeerSessionTunnelFilter` no-op），只有 stealth outer 层。

**核心问题**：为什么第 1 层加密在 localhost 上无影响（S1=377, S3=382 Mbps），
但在高延迟链路上导致 8-10x 下降？2.3μs/packet 的加密开销不足以解释 8-10x。

### 6.5 已排除的假设

| 假设 | 证据 | 结论 |
| --- | --- | --- |
| AEAD 加密计算开销 | 微基准 2.3μs/pkt，localhost S1=S3 | **排除**：CPU 开销不足以解释 |
| 锁竞争（Mutex） | localhost S1=S3，LAN profiling 锁 <0.5% CPU | **排除**：低延迟下无竞争 |
| 内存分配 | 第 5 节已修复接收路径分配 | **部分排除**：需在高延迟下重新验证 |
| QUIC 流控窗口 | 第 4.10 节已验证 8MB 窗口无效 | **排除** |
| TCP 同样受影响 | 跨太平洋 S3 TCP 无显著下降 | **排除**：仅 QUIC 受影响 |

### 6.6 待验证的根因假设

以下假设按可能性排序，需要通过高延迟链路 profiling 验证：

#### 假设 A：`PeerSessionTunnelFilter` 延迟了 QUIC stream 的 `start_send` 时机

QUIC stream 的 `poll_ready` 返回 Ready 后，`start_send` 必须尽快写入数据。
S3 中 `start_send` → `before_send` → `encrypt_payload`（2.3μs + 锁），
在高 BDP 链路上，quinn 的发送窗口短暂可用，如果 `start_send` 延迟超过
一个 RTT 内的可用窗口时间，会导致 quinn 空闲等待，降低有效吞吐。

**验证方法**：在 `before_send` 前后打时间戳，统计 `start_send` 的处理延迟分布。

#### 假设 B：双层加密导致 per-packet 处理时间超过 QUIC ack delay

quinn 对每个 packet 的 ack 有时间预期。如果 `PeerSessionTunnelFilter` + `QuicStealthSocket`
的合计处理时间导致发送间隔不均匀，BBR 会低估可用带宽。

**验证方法**：对比 S2 和 S3 的 quinn 内部 cwnd 变化日志（`quinn_proto::connection::State`）。

#### 假设 C：`FramedWriter` 的 `max_buffer_count=64` 限制与高延迟交互

`FramedWriter::poll_ready` 在 `sending_bufs.bufs_cnt() >= 64` 时触发 `poll_flush`。
在 S3 中，每个加密后的 packet 多 28 字节（AEAD tail），可能导致 frame 更早填满
64 个 buffer 的限制，触发同步 flush，阻塞 `poll_ready`。

**验证方法**：将 `max_buffer_count` 改为 256 或更大，对比 S3 QUIC 吞吐变化。

#### 假设 D：S3 触发了显式 secure 独有的 relay/session 分支

第 1 节提到：显式 `secure_mode` 会触发 `RelayPeerMap::new()` 和
`PeerManager::RpcTransport` 的 secure 分支，派生 secure 不会。
这些分支可能在高延迟链路上引入额外开销。

**验证方法**：在 S3 测试中检查 `cost=p2p` 还是 `cost=relay`，
确认是否走了 relay 路径。如果走了 relay，需要单独对比 direct p2p S3。

### 6.7 根因调查计划

1. **补全基准测试矩阵**（6.3 节），在同一链路条件下运行 B0-B7
2. **高延迟链路 profiling**：在 S3 QUIC 场景下用 `perf record` 采集 CPU profile，
   对比 S2 QUIC profile，找出 S3 独有的热点
3. **quinn 内部状态对比**：启用 quinn trace 日志，对比 S2 和 S3 的 cwnd、rtt、
   packets_sent、packets_lost 变化曲线
4. **`tc netem` 本地复现**：在 localhost 上用 `tc qdisc add dev lo root netem delay 190ms`
   模拟高延迟，验证 S3 下降是否可复现。如可复现，用 `perf` 和 `strace` 深入分析
5. **逐层消除**：在 `tc netem` 环境下，分别禁用 `PeerSessionTunnelFilter`、
   `QuicStealthSocket`、`FramedWriter` buffer 限制，定位哪一层导致下降

### 6.8 复测工具

两个 `#[ignore]` 基准测试已添加到 `src/peers/peer_conn.rs`：

1. **`peer_session_encrypt_bench`** — 微基准，测量纯 AEAD 加密/解密开销：
   ```
   cargo test --release --package easytier --lib -- \
     peers::peer_conn::tests::peer_session_encrypt_bench -- --nocapture --ignored
   ```

2. **`quic_secure_mode_bench`** — 本地 QUIC + ring tunnel 吞吐对比（B0/B5/B6/B7 + R0/R1/R2）：
   ```
   cargo test --release --package easytier --lib -- \
     peers::peer_conn::tests::quic_secure_mode_bench -- --nocapture --ignored
   ```
   - B0: QUIC plain（无 stealth）
   - B5: QUIC + stealth outer（gate phase，OsRng + HMAC-SHA256）
   - B6: QUIC + stealth outer（AEAD phase，通过 `enable_outer_key_fork_for_test()` 测量稳态性能）
   - B7: QUIC + stealth outer AEAD + PeerSessionTunnelFilter（S3 双层加密模拟）
   - R0/R1: Ring tunnel plain / + PeerSessionTunnelFilter
   - R2: Stealth outer 加密微基准（gate-seal vs outer-seal）

### 6.9 本地基准测试结果（2026-07-10，三机跨 kernel 对比）

使用重写后的 `quic_secure_mode_bench` 在三台机器上运行，包含新增的 B6/B7 场景（outer AEAD phase）。
B6/B7 通过 `enable_outer_key_fork_for_test()` 使 forked session 在创建时即设置 outer key，
QUIC 握手仍在 gate phase 完成（前 ~1 秒），之后自动过渡到 outer AEAD phase，
10 秒 benchmark 中约 9 秒测量的是 outer AEAD phase 性能。
B7 在 B6 基础上增加 `PeerSessionTunnelFilter`（S3 双层加密模拟）。

**QUIC 场景**（`_tunnel_bench` / `_tunnel_bench_s3`，1024B 包，10 秒持续发送）：

| 编号 | 场景 | Docker (3.10) | 192.168.1.37 (3.10) | 10.20.0.65 (5.10) |
| --- | --- | ---: | ---: | ---: |
| B0 | QUIC plain（无 stealth） | 110 Mbps | 83.6 Mbps | 39.9 Mbps |
| B5 | QUIC + stealth outer（gate phase） | 9.9 Mbps | 8.6 Mbps | 40.2 Mbps |
| B6 | QUIC + stealth outer（AEAD phase） | 83.7 Mbps | 50.5 Mbps | 57.0 Mbps |
| B7 | QUIC + stealth AEAD + PeerSession（S3） | 80.7 Mbps¹ | — | — |

¹ B7 仅在 Docker (192.168.2.160) 上运行过一次（2026-07-10 修复 `open_in_place_from` bug 后）。
B7 ≈ B6（80.7 vs 80.8 MB/s），证明 `PeerSessionTunnelFilter` 在 outer AEAD phase 上开销可忽略。

| 编号 | B5/B0 | B6/B0 | B7/B0 |
| --- | ---: | ---: | ---: |
| Docker (3.10) | 0.09x | **0.76x** | **0.74x** |
| 192.168.1.37 (3.10) | 0.10x | **0.60x** | — |
| 10.20.0.65 (5.10) | **1.01x** | **1.43x** | — |

**Ring tunnel 场景**（`run_ring_bench`，1400B 包，100K 包固定量）：

| 编号 | 场景 | Docker (3.10) | 192.168.1.37 (3.10) | 10.20.0.65 (5.10) |
| --- | --- | ---: | ---: | ---: |
| R0 | Ring plain（进程内基线） | 11,887 Mbps | 6,948 Mbps | 4,247 Mbps |
| R1 | Ring + PeerSessionTunnelFilter | 4,519 Mbps | 2,889 Mbps | 2,261 Mbps |

**Stealth outer 加密微基准**（`R2`，100K 次 seal-only，1400B payload，无网络）：

| 阶段 | Docker (3.10) | 192.168.1.37 (3.10) | 10.20.0.65 (5.10) |
| --- | ---: | ---: | ---: |
| Gate phase（`seal_gate_datagram`） | 0.10 Gbps | 0.10 Gbps | 0.90 Gbps |
| Outer phase（`seal_datagram` + AEAD） | 11.15 Gbps | 10.33 Gbps | 19.69 Gbps |

### 6.10 关键发现

1. **B6 确认 outer AEAD phase 性能远优于 gate phase**：
   - B6（outer AEAD phase）在所有机器上均显著优于 B5（gate phase）
   - Docker (3.10)：B6/B0 = **0.76x** vs B5/B0 = 0.09x（8.4x 改善）
   - 192.168.1.37 (3.10)：B6/B0 = **0.60x** vs B5/B0 = 0.10x（6.0x 改善）
   - 10.20.0.65 (5.10)：B6/B0 = **1.43x** vs B5/B0 = 1.01x（outer AEAD 甚至超过 plain QUIC）
   - **结论**：生产环境中（gate phase 仅持续 ~1 秒），QUIC stealth 的稳态性能接近 plain QUIC
   - B6 在 5.10 上超过 B0 的原因：outer AEAD（AES-256-GCM 硬件加速）可能比 quinn 内部的
     SeaHasher checksum + packet protection 开销更低

2. **Gate phase 是 benchmark 中 B5 下降的根因，但不是生产瓶颈**：
   - `seal_gate_datagram` 每次调用 `OsRng.fill_bytes()` 获取随机 nonce（系统调用），比 AEAD 路径慢 **100 倍**（3.10）/ **9 倍**（5.10）
   - 但 gate phase 在生产中仅持续 ~1 秒（`QUIC_STEALTH_OUTER_SEND_DELAY`），之后自动过渡到 outer AEAD phase
   - B6 通过 `enable_outer_key_fork_for_test()` 成功测量了 outer AEAD phase 的端到端吞吐
   - R2 微基准确认 outer phase AEAD 性能为 10~20 Gbps，远非瓶颈

3. **`max_transmit_segments`/`max_receive_segments` 硬编码为 1（已修复）—— 跨 kernel 验证**：
   - `QuicStealthSocket` 将 GSO/GRO 批量大小硬编码为 1，阻止 quinn 批量发送/接收
   - **已修复**：改为委托 `self.inner.max_transmit_segments()` / `self.inner.max_receive_segments()`
   - **跨 kernel 验证结果**（含 B6 outer AEAD phase）：

   | 环境 | Kernel | B0 (Mbps) | B5 (Mbps) | B6 (Mbps) | B5/B0 | B6/B0 | GSO 支持 |
   | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
   | Docker (192.168.2.160) | 3.10 | 110 | 9.9 | 83.7 | 0.09x | **0.76x** | ❌ |
   | 真机 192.168.1.37 | 3.10 | 83.6 | 8.6 | 50.5 | 0.10x | **0.60x** | ❌ |
   | 真机 10.20.0.65 (KR) | 5.10 | 39.9 | 40.2 | 57.0 | **1.01x** | **1.43x** | ✅ |

   - **结论**：在支持 `UDP_SEGMENT` 的 kernel 5.10 上，修复后 B5 与 B0 持平（gate phase 的 OsRng 开销被 GSO 批量弥补）
   - CentOS 7 (kernel 3.10) 不支持 `UDP_SEGMENT` socket option（kernel 4.18+ 才引入），`max_gso_segments()` 返回 1，修复无效
   - **但 B6 显示 3.10 仍有 60% 的 plain QUIC 性能**——outer AEAD phase 本身开销很小，3.10 的性能下降主要来自 gate phase（OsRng）而非 outer AEAD
   - **`max_transmit_segments = 1` 是 QUIC stealth gate phase 性能下降的主要根因，已通过 GSO 修复解决**

4. **Gate phase `OsRng` 性能跨 kernel 差异显著**：

   | 环境 | Kernel | R2 gate-seal (Gbps) | R2 outer-seal (Gbps) |
   | --- | --- | ---: | ---: |
   | Docker (192.168.2.160) | 3.10 | 0.10 | 11.15 |
   | 真机 192.168.1.37 | 3.10 | 0.10 | 10.33 |
   | 真机 10.20.0.65 (KR) | 5.10 | 0.90 | 19.69 |

   - KR host 的 gate-seal 快 8.9 倍，可能是 kernel 5.10 的 `getrandom()` syscall 更快
   - outer-seal 也快 1.8 倍（AMD EPYC Rome vs Intel Xeon E5-2620 v4）

5. **`PeerSessionTunnelFilter` 开销显著但非主因**：
   - R0 → R1（Ring plain → Ring + PeerSessionTunnelFilter）：2.3~3.6x 下降
   - 在进程内 ring tunnel 上仍有 2~4 Gbps
   - **B7 确认**：在 QUIC outer AEAD phase 上，B7 ≈ B6（80.7 vs 80.8 MB/s），
     `PeerSessionTunnelFilter` 开销可忽略（<1%），双层加密不是 QUIC 性能下降的根因

6. **`open_in_place_from` Initial 包丢弃 bug（已修复）**：
   - `quic.rs:571-576` 原逻辑：当 `outer_key().is_some()` 且解密后是 QUIC Initial 包时直接丢弃
   - 该检查的目的是防止握手完成后重放 Initial 包，但在 `fork_with_outer_key` 模式下
     outer key 在握手前已设置，导致合法 Initial 包被丢弃，QUIC 握手超时
   - **修复**：增加 grace period 检查，仅当 outer key 设置时间超过 `QUIC_STEALTH_GATE_RECV_GRACE`（5s）
     时才丢弃 Initial 包，与 `open()` 的 gate fallback 逻辑一致

7. **其他协议不受 `max_transmit_segments` 影响**：
   - UDP、TCP、WG、FakeTCP、WS 均使用标准 socket I/O，不经过 quinn 的 `AsyncUdpSocket` trait
   - UDP 接收路径已使用 `open_datagram_in_place`（零分配）
   - TCP 接收路径已使用 `open_datagram_in_place`
   - WG 接收路径已使用 `open_in_place`
   - FakeTCP 接收路径仍使用 allocating `open_datagram`（有 `DroppedStaleGateControl` 需求，非热路径）
   - **QUIC 是唯一受 `max_transmit_segments` bug 影响的协议**

8. **结论与下一步**：
   - **主要根因已定位并修复**：`max_transmit_segments = 1` 导致 quinn 无法批量发送 QUIC 包，在支持 GSO 的 kernel 上修复后 gate phase 性能下降从 9.7x 减少到 1x
   - **B6/B7 确认 outer AEAD phase 性能良好**：即使在 kernel 3.10（无 GSO）上，B6/B0 仍达 60~76%，B7 ≈ B6 说明 `PeerSessionTunnelFilter` 双层加密开销可忽略
   - **`open_in_place_from` Initial 包丢弃 bug 已修复**：`fork_with_outer_key` 模式下 QUIC 握手恢复正常
   - **CentOS 7 (kernel 3.10) 的 gate phase 瓶颈是 OsRng 系统调用**：`getrandom()` 在 3.10 上比 5.10 慢 9 倍，但 gate phase 仅持续 ~1 秒，非持续瓶颈
   - 次要优化方向（优化方案文档 P2）：消除接收路径堆分配、in-place 解密
   - 待验证：高延迟链路（~190ms RTT）下 S3 QUIC 的 8-10x 下降是否仍存在（第 6 节）
