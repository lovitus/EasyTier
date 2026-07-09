# Stealth/Secure 加密性能 Profiling 报告

**日期**：2026-07-09  
**代码线**：`releases/v2.6.9`  
**测试环境**：`192.168.2.160` ↔ `192.168.1.38`，同 LAN，CentOS 7.9 / kernel 3.10  
**二进制**：`easytier-core-tuned`（musl 静态链接，dev build）

## 1. 测试配置

```
--secure-mode true
--stealth-mode true
--stealth-protocols tcp
--multi-thread true --multi-thread-count 4
```

拓扑：direct p2p，TCP underlay，tunnel_proto=tcp，cost=p2p。

## 2. 吞吐结果

| 模式 | 吞吐 | 倍数 |
| --- | ---: | ---: |
| plain TCP | 107.75 MB/s | 1.0x |
| explicit secure + stealth (TCP) | 6.31 MB/s | 17x 慢 |
| QUIC (plain, baseline) | 6.26 MB/s | 17x 慢 |
| QUIC (stream_receive_window=8MB) | 6.17 MB/s | 17x 慢 |

注意：explicit secure + stealth 的 TCP 吞吐与 QUIC plain 吞吐几乎相同（~6 MB/s），
说明两者受不同但同等严重的瓶颈限制。

## 3. Profiling 方法

- `perf record -F 999 -a -g -- sleep 35`（system-wide，999Hz 采样）
- kernel 3.10 的 `perf record -p PID` 对 musl 静态二进制采集 0 samples，改用 system-wide
- throughput 期间采样，throughput server（sender）在 192.168.2.160，client（receiver）在 192.168.1.38
- 157K samples，0 lost

## 4. CPU 占比分布

| 类别 | 占总采样 | 占非 idle |
| --- | ---: | ---: |
| idle (swapper / native_safe_halt) | 82.48% | — |
| easytier userspace (easytier-core-tuned) | 14.05% | 80.2% |
| easytier in kernel (tokio-rt-worker + kernel symbols) | 1.54% | 8.8% |
| 其他 (sshd, gsd-color, top 等) | ~1.9% | ~11% |

CPU 接近单核满载（82% idle 说明单线程瓶颈，一个 tokio worker 跑满）。

## 5. easytier userspace 热点函数

按 self-time 排序，占 easytier userspace CPU 比例：

| # | 函数 | 占 easytier | 占非 idle | 模块 | 说明 |
| --- | --- | ---: | ---: | --- | --- |
| 1 | `sha2::sha256::compress256` | 63.7% | 51.0% | sha2 crate | SHA-256 压缩函数核心 |
| 2 | `memcpy` | 12.7% | 10.2% | libc | 内存拷贝 |
| 3 | `digest::core_api::FixedOutputCore::finalize_fixed_core` | 5.0% | 4.0% | digest crate | SHA-256 finalize |
| 4 | `easytier::tunnel::stealth::apply_keystream` | 3.3% | 2.6% | stealth | HMAC-SHA256 流密码 |
| 5 | `core::slice::copy_from_slice_impl` | 1.4% | 1.1% | core | 内存拷贝 |
| 6 | `syscall` | 0.9% | 0.7% | libc | 系统调用入口 |
| 7 | `digest::mac::Mac::update` | 0.6% | 0.5% | digest | HMAC update |
| 8 | `__libc_malloc_impl` | 0.6% | 0.5% | musl | 内存分配 |
| 9 | `__libc_free` | 0.6% | 0.5% | musl | 内存释放 |
| 10 | `easytier::tunnel::stealth::hkdf_sha256` | 0.4% | 0.3% | stealth | HKDF 密钥派生 |
| 11 | `__lock` | 0.4% | 0.3% | musl | mutex lock |
| 12 | `__unlock` | 0.4% | 0.3% | musl | mutex unlock |
| 13 | `hmac::get_der_key` | 0.3% | 0.2% | hmac crate | HMAC 密钥派生 |
| 14 | `_aesni_ctr32_ghash_6x` | 0.3% | 0.2% | ring | AES-GCM 硬件加速 |
| 15 | `easytier::tunnel::stealth::outer_mac` | 0.1% | 0.1% | stealth | stealth MAC 计算 |
| 16 | `SecureDatagramSession::decrypt_payload` | 0.1% | 0.1% | peers | PeerSession 解密 |

**关键观察**：

- SHA-256 相关函数（#1, #3, #7, #10, #13）合计占 easytier userspace 的 **~70%**
- AES-GCM（#14）仅占 0.3%，有 AES-NI 硬件加速
- 锁竞争（#11, #12）合计 0.8%，不是瓶颈
- `memcpy` + `copy_from_slice`（#2, #5）合计 14.1%，内存拷贝开销显著
- 内存分配（#8, #9）合计 1.2%

## 6. 根因分析

### 6.1 双重加密架构

explicit secure + stealth 下，每个包经过两层加密：

**第一层：PeerSession 加密**（`SecureDatagramSession::encrypt_payload`）
- AES-GCM with AES-NI，0.3% CPU，**不是瓶颈**

**第二层：Stealth outer 加密**（`OuterSessionState::seal_datagram` → `seal()`）
- HMAC-SHA256 实现的流密码 + MAC，**占 ~70% CPU**

### 6.2 `seal()` 每包操作

`src/tunnel/stealth.rs:799-811`：

1. `outer_subkeys(key)` → 2 次 `hkdf_sha256()` → 4 次 HMAC-SHA256
   - **每包重复计算，enc_key/mac_key 在连接生命周期内不变**
2. `OsRng.fill_bytes(&mut nonce)` → 1 次 `getrandom()` 系统调用
3. `Vec::with_capacity()` + `extend_from_slice()` → 1 次分配 + 2 次拷贝
4. `apply_keystream(enc_key, nonce, data)` → 每 32 字节一次完整 HMAC-SHA256
   - 1200 字节包 = ⌈1200/32⌉ = 38 次 HMAC-SHA256
5. `outer_mac(mac_key, nonce, ciphertext)` → 1 次 HMAC-SHA256

**每包总计 ~41 次 HMAC-SHA256 = ~82 次 SHA-256 compress**

### 6.3 `apply_keystream` 的问题

`src/tunnel/stealth.rs:761-777`：

```rust
fn apply_keystream(enc_key: &[u8; 32], nonce: &[u8; OUTER_NONCE_LEN], data: &mut [u8]) {
    let mut counter: u32 = 0;
    let mut offset = 0;
    while offset < data.len() {
        let mut mac = HmacSha256::new_from_slice(enc_key).expect("hmac key");
        mac.update(b"et-outer-strm");
        mac.update(nonce);
        mac.update(&counter.to_be_bytes());
        let block = mac.finalize().into_bytes();
        // XOR 32 bytes
        offset += n;
        counter = counter.wrapping_add(1);
    }
}
```

用 HMAC-SHA256 模拟 CTR 流密码，每 32 字节需要完整的 HMAC setup + update + finalize。
标准 AES-CTR 只需一次 key schedule + 多次 block encrypt（AES-NI ~4 cycles/block vs
HMAC-SHA256 ~200 cycles/block）。

### 6.4 `open()` 的额外开销

`src/tunnel/stealth.rs:815-832`：

- `ciphertext.to_vec()` → 额外一次内存分配 + 拷贝
- 同样每次调用 `outer_subkeys()` 重复 HKDF

### 6.5 调用路径

stealth `seal()`/`open()` 被以下调用方每包调用：

| Transport | seal 调用点 | open 调用点 |
| --- | --- | --- |
| TCP | `tunnel/common.rs:332` `seal_datagram` | `tunnel/common.rs:241` `open_datagram` |
| UDP | `tunnel/udp.rs:345` `seal_datagram` | `tunnel/udp.rs:686` `open_datagram` |
| FakeTCP | (via common) | `tunnel/fake_tcp/mod.rs:778` `open_datagram` |
| WebSocket | `tunnel/websocket.rs:171` `seal_datagram` | `tunnel/websocket.rs:200` `open_datagram` |

所有 stealth-enabled transport 都走相同的 `seal()`/`open()` 热路径。

## 7. 排除的假设

| 假设 | profiling 结论 |
| --- | --- |
| 锁竞争是瓶颈 | ❌ `__lock`+`__unlock` = 0.8% |
| `SystemTime::now()` 是瓶颈 | ❌ 未出现在 top 20 |
| RelayPeerMap 额外分支 | ❌ 未出现在 top 20 |
| AES-GCM 加密是瓶颈 | ❌ 仅 0.3%，有 AES-NI |
| 内存泄漏 | ❌ RSS 稳定 ~15MB |

## 8. 原始 perf 数据

perf data 文件：`/tmp/perf-secure3.data`（192.168.2.160）

复现命令：
```bash
# Server (192.168.2.160)
easytier-core-tuned --secure-mode true --stealth-mode true --stealth-protocols tcp \
  --listeners tcp://0.0.0.0:11030 --ipv4 10.231.0.1/24 --hostname node-a ...

# Client (192.168.1.38)
easytier-core-tuned --secure-mode true --stealth-mode true --stealth-protocols tcp \
  --peers tcp://192.168.2.160:11030 --ipv4 10.231.0.2/24 --hostname node-b ...

# Profiling (on server)
perf record -F 999 -a -g -- sleep 35
perf report --stdio --no-children
```
