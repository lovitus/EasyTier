# KCP 生命周期安全修复方案（实施版）

## Implementation Status

- 已实现：`third_party/kcp-sys::KcpEndpoint` 增加 connect lifecycle guard，同步清理取消/超时/发送失败/异常状态留下的 `state_map` 和 `conn_map`，取消路径使用 best-effort `try_send(RST)`。
- 已实现：stale cleanup 同步裁剪 orphan `state_map` / `conn_map`，并增加 crate-internal stats。
- 已实现：`add_conn()` 使用 endpoint 的 `kcp_config_factory` 创建 `KcpConnection`，proxy endpoint 的 5ms interval 配置不再被默认 turbo 配置覆盖。
- 保持不变：SOCKS KCP 仍是 KCP-only fast path；失败时返回 SOCKS 错误，不回退 smoltcp/native。
- 保持不变：不新增 wire/protobuf/RPC/GUI 字段，stats 只用于 tracing 和测试断言。
- 已在远程 builder 验证：`cargo test -p kcp-sys` 16/16 通过；`cargo test -p easytier proxy_destination_refused_falls_back_to_native_without_health_penalty -- --nocapture` 3/3 通过。

## Summary

修复集中在 `third_party/kcp-sys::KcpEndpoint`，不改变 SOCKS KCP-only 语义，不做 SOCKS fallback，不改 wire/protobuf/RPC/GUI。

目标：

- 保留 SOCKS KCP 的 5 路 hedge + 200ms stagger 性能。
- 修复 connect future 取消、超时、发送失败、迟到包导致的 KCP state 泄露。
- 普通 TCP Proxy KCP 同步受益。
- 用内部 stats/tracing/tests 验证资源回收。

## Key Changes

- `KcpEndpoint::connect()` 增加 lifecycle guard：
  - `state_map` 插入后由 guard 接管。
  - future 被 drop、超时、发送失败、状态异常或 `add_conn()` 失败时，guard 清理 `state_map` 和可能存在的 `conn_map`。
  - guard `Drop` 只能做同步清理和 `output_sender.try_send(rst)`，不能依赖 async Drop。
  - 显式错误路径可继续 async send RST，但取消路径必须靠同步 cleanup。
  - `add_conn()` 成功后 guard disarm，owner 转移给 `conn_map` / close watcher。

- 保持 SOCKS KCP 行为不变：
  - 命中 KCP 条件时仍走 `Socks5KcpConnector`。
  - KCP connect 失败仍返回 SOCKS 错误，不回退 smoltcp/native。
  - smoltcp 只在现有非 KCP 条件下使用。
  - 不新增配置项。

- 远端 orphan 边界：
  - 本端取消时 best-effort RST。
  - RST 丢失时，远端依赖 pong timeout + cleanup tick 回收。
  - `state_map` cleanup 必须同步裁剪 `conn_map`。
  - 不给正常 KCP stream 增加短 idle timeout。

- KCP 配置修复：
  - `add_conn()` 使用 endpoint 的 `kcp_config_factory` 创建 `KcpConnection`。
  - 不再固定 `KcpConfig::new_turbo()`。
  - 确保 proxy endpoint 的 5ms interval 配置真实生效。

- 内部 stats：
  - 新增 `KcpEndpoint::stats()` 内部方法。
  - 首版指标只包含 endpoint 能准确归因的值：`state_map_len`、`conn_map_len`、`connect_cancel_cleanup_total`、`forced_cleanup_total`、`orphan_timeout_cleanup_total`。
  - 不在 endpoint 内统计 `hedge_loser_cleanup_total`，避免耦合上层 hedge 语义。
  - 首版 stats 只用于 tracing 和测试断言，不进 RPC/protobuf/GUI。

## Test Plan

- `kcp-sys` 单元测试：
  - connect future 在 `state_map` 插入后被 drop，`state_map_len/conn_map_len` 回到 0，`connect_cancel_cleanup_total` 增加。
  - connect 超时和发送失败后无残留 state。
  - 5 路 hedge 中 1 路成功、4 路取消，endpoint 不残留 loser state。
  - 迟到 SYN-ACK 不会让已取消 active connection 重新进入 Established。
  - connect 成功后立即 drop stream，不长期残留。
  - 模拟 `conn_map/state_map` 被并发 cleanup 移除后 `KcpStream::new()` 返回失败；只验证无残留，不新增生产逻辑。
  - RST 丢失时，远端 orphan 在 pong timeout + cleanup tick 后释放，`orphan_timeout_cleanup_total` 增加。
  - `kcp_config_factory` 生成的配置被实际连接使用。

- SOCKS 集成测试：
  - KCP 可用时 SOCKS 仍选择 KCP，不回退 smoltcp。
  - KCP connect 失败时客户端收到明确 SOCKS 错误，不残留后台 KCP state。
  - 100/500/1000 个 SOCKS KCP 短连接后，`state_map_len/conn_map_len` 回到基线。
  - 远端目标服务不残留长期空 TCP 连接。

- Proxy 回归测试：
  - 普通 TCP Proxy 的 `QUIC → KCP → Native` 顺序不变。
  - KCP prepare 失败仍按现有 selector 进入下一候选。
  - READY ACK 行为不被 SOCKS legacy KCP 修复影响。

- 实机验收：
  - Mihomo TUN 开启时执行 SOCKS KCP 压测，EasyTier 主实例和 Mihomo 不再长期互相拉满 CPU。
  - no-TUN / smoltcp 场景下，SOCKS KCP 性能保持当前高性能水平。
  - 压测结束后 2 分钟内 KCP stats 回到基线，FD、任务数、目标 TCP 连接数无持续增长。

## Assumptions

- 本轮只修 KCP lifecycle，不做 SOCKS fallback。
- 本轮不改 SOCKS KCP legacy `proxy_prepare_version=0`。
- 本轮不新增 RPC/protobuf/GUI 指标。
- `KcpEndpoint` 不感知 hedge loser 概念；如后续要区分 hedge loser，由 `NatDstKcpConnector` 上层统计。
- “安全”定义为资源有 owner、有 cleanup、有上限、有内部指标验证；不承诺异常网络下零瞬时 orphan。
