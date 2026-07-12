# EasyTier vendoring note

This directory vendors `eycorsican/netstack-smoltcp` commit
`25e6ad4019dc74119f68a106e3eb5625b0a8c8d4`, the revision selected by the
pinned Leaf dependency.

EasyTier carries one lifecycle fix in `src/tcp.rs`: a requested TCP write-half
shutdown waits until the adapter's pending send buffer has been transferred to
the smoltcp socket before calling `socket.close()`. The upstream revision closes
the socket first, which can discard a fast final response immediately followed
by EOF while still emitting FIN.

Keep the upstream MIT/Apache-2.0 license files intact. Any future upstream
refresh must retain the fast-EOF regression test and revalidate transparent TCP
through both DIRECT and mesh SOCKS actors.
