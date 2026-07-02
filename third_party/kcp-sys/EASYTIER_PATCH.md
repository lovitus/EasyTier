# EasyTier kcp-sys Patch

This directory vendors `EasyTier/kcp-sys` at upstream commit
`d7427c22d764deb1860a7d37acc446ed5033464c`.

EasyTier carries a local lifecycle patch because that upstream revision does not
provide an observable graceful shutdown and sends the protocol FIN only once.
The patch:

- waits for the KCP send queue to drain before closing;
- drains data already accepted by KCP before reporting peer FIN as stream EOF;
- retransmits FIN until the peer confirms closure or a fixed deadline expires;
- uses a `LastAck` state and FIN+ACK confirmation for reliable passive close;
- force-cleans local connection state at the deadline and reports that outcome;
- caps live KCP connection state at 4096 entries; and
- releases closed state immediately instead of waiting for periodic cleanup.

Keep the upstream commit above synchronized when refreshing this directory.
Run `cargo test -p kcp-sys` and the EasyTier KCP proxy tests after every refresh.
