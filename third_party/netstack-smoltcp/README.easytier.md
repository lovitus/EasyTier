# EasyTier vendoring note

This directory vendors `eycorsican/netstack-smoltcp` commit
`25e6ad4019dc74119f68a106e3eb5625b0a8c8d4`, the revision selected by the
pinned Leaf dependency.

EasyTier carries two lifecycle fixes in `src/tcp.rs`:

- a requested TCP write-half shutdown waits until the adapter's pending send
  buffer has been transferred to the smoltcp socket before calling
  `socket.close()`, preventing a fast final response followed by EOF from being
  discarded;
- after FIN is committed, the adapter wakes the AsyncWrite shutdown waiter and
  completes at its local `Closing` state instead of waiting for the peer and
  TIME-WAIT state machine. smoltcp continues its normal TCP cleanup in the
  background, while upstream proxy streams can be released.

Keep the upstream MIT/Apache-2.0 license files intact. Any future upstream
refresh must retain the fast-EOF and shutdown-completion regression tests and
revalidate transparent TCP through both DIRECT and mesh SOCKS actors.
