use std::{net::IpAddr, sync::Arc};

use arc_swap::ArcSwap;
use cidr::{IpCidr, Ipv4Cidr, Ipv6Cidr};
use prefix_trie::PrefixSet;
use thiserror::Error;

const MIN_IPV4_HEADER: usize = 20;
const MIN_IPV6_HEADER: usize = 40;
const MAX_PACKET_SIZE: usize = 65_535;

#[derive(Debug, Error)]
pub enum PacketError {
    #[error("empty packet")]
    Empty,
    #[error("truncated IPv{version} packet: {length} bytes")]
    Truncated { version: u8, length: usize },
    #[error("unsupported IP version {0}")]
    UnsupportedVersion(u8),
    #[error("malformed IPv{version} packet")]
    Malformed { version: u8 },
    #[error("packet exceeds {MAX_PACKET_SIZE} bytes")]
    TooLarge,
    #[cfg(unix)]
    #[error("packet bridge I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketClass {
    Mesh,
    Policy,
}

#[derive(Debug, Clone, Default)]
pub struct MeshRouteSnapshot {
    routes: Arc<[IpCidr]>,
    ipv4: PrefixSet<Ipv4Cidr>,
    ipv6: PrefixSet<Ipv6Cidr>,
}

impl MeshRouteSnapshot {
    pub fn new(mut routes: Vec<IpCidr>) -> Self {
        routes.sort_unstable_by(|left, right| {
            right
                .network_length()
                .cmp(&left.network_length())
                .then_with(|| left.to_string().cmp(&right.to_string()))
        });
        routes.dedup();
        let mut ipv4 = PrefixSet::new();
        let mut ipv6 = PrefixSet::new();
        for route in &routes {
            match route {
                IpCidr::V4(route) => {
                    ipv4.insert(*route);
                }
                IpCidr::V6(route) => {
                    ipv6.insert(*route);
                }
            }
        }
        Self {
            routes: routes.into(),
            ipv4,
            ipv6,
        }
    }

    pub fn owns(&self, destination: IpAddr) -> bool {
        match destination {
            IpAddr::V4(address) => {
                address.is_broadcast()
                    || address.is_multicast()
                    || self
                        .ipv4
                        .get_lpm(&Ipv4Cidr::new(address, 32).expect("host prefix is valid"))
                        .is_some()
            }
            IpAddr::V6(address) => {
                address.is_multicast()
                    || self
                        .ipv6
                        .get_lpm(&Ipv6Cidr::new(address, 128).expect("host prefix is valid"))
                        .is_some()
            }
        }
    }

    pub fn routes(&self) -> &[IpCidr] {
        &self.routes
    }
}

pub struct PacketClassifier {
    routes: ArcSwap<MeshRouteSnapshot>,
}

impl PacketClassifier {
    pub fn new(routes: MeshRouteSnapshot) -> Self {
        Self {
            routes: ArcSwap::from_pointee(routes),
        }
    }

    pub fn replace_routes(&self, routes: MeshRouteSnapshot) {
        self.routes.store(Arc::new(routes));
    }

    pub fn classify(&self, packet: &[u8]) -> Result<PacketClass, PacketError> {
        let destination = destination_ip(packet)?;
        Ok(if self.routes.load().owns(destination) {
            PacketClass::Mesh
        } else {
            PacketClass::Policy
        })
    }
}

fn destination_ip(packet: &[u8]) -> Result<IpAddr, PacketError> {
    if packet.len() > MAX_PACKET_SIZE {
        return Err(PacketError::TooLarge);
    }
    let Some(first) = packet.first() else {
        return Err(PacketError::Empty);
    };
    match first >> 4 {
        4 => {
            if packet.len() < MIN_IPV4_HEADER {
                return Err(PacketError::Truncated {
                    version: 4,
                    length: packet.len(),
                });
            }
            let header_length = usize::from(packet[0] & 0x0f) * 4;
            let total_length = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
            if header_length < MIN_IPV4_HEADER
                || header_length > packet.len()
                || total_length < header_length
                || total_length > packet.len()
            {
                return Err(PacketError::Malformed { version: 4 });
            }
            Ok(IpAddr::V4(
                [packet[16], packet[17], packet[18], packet[19]].into(),
            ))
        }
        6 => {
            if packet.len() < MIN_IPV6_HEADER {
                return Err(PacketError::Truncated {
                    version: 6,
                    length: packet.len(),
                });
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&packet[24..40]);
            Ok(IpAddr::V6(octets.into()))
        }
        version => Err(PacketError::UnsupportedVersion(version)),
    }
}

#[cfg(unix)]
mod unix_bridge {
    use std::os::fd::{AsRawFd, IntoRawFd, RawFd};

    use tokio::net::UnixDatagram;

    use super::{MAX_PACKET_SIZE, PacketError};

    /// Mux-owned packet endpoint. Datagram semantics preserve packet boundaries.
    pub struct LeafPacketBridge {
        socket: UnixDatagram,
    }

    /// Leaf-owned endpoint. Ownership of the FD is transferred exactly once.
    pub struct LeafPacketEndpoint {
        socket: Option<std::os::unix::net::UnixDatagram>,
    }

    impl LeafPacketBridge {
        pub fn pair() -> Result<(Self, LeafPacketEndpoint), PacketError> {
            let (mux, leaf) = std::os::unix::net::UnixDatagram::pair()?;
            mux.set_nonblocking(true)?;
            leaf.set_nonblocking(true)?;
            let socket = UnixDatagram::from_std(mux)?;
            Ok((Self { socket }, LeafPacketEndpoint { socket: Some(leaf) }))
        }

        pub async fn send_to_leaf(&self, packet: &[u8]) -> Result<(), PacketError> {
            if packet.len() > MAX_PACKET_SIZE {
                return Err(PacketError::TooLarge);
            }
            let sent = self.socket.send(packet).await?;
            if sent != packet.len() {
                return Err(PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "partial packet bridge write",
                )));
            }
            Ok(())
        }

        pub fn try_send_to_leaf(&self, packet: &[u8]) -> Result<(), PacketError> {
            if packet.len() > MAX_PACKET_SIZE {
                return Err(PacketError::TooLarge);
            }
            let sent = self.socket.try_send(packet)?;
            if sent != packet.len() {
                return Err(PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "partial packet bridge write",
                )));
            }
            Ok(())
        }

        pub async fn recv_from_leaf(&self, packet: &mut [u8]) -> Result<usize, PacketError> {
            Ok(self.socket.recv(packet).await?)
        }

        pub fn as_raw_fd(&self) -> RawFd {
            self.socket.as_raw_fd()
        }
    }

    impl LeafPacketEndpoint {
        pub fn into_raw_fd(mut self) -> RawFd {
            self.socket
                .take()
                .expect("Leaf endpoint already consumed")
                .into_raw_fd()
        }
    }

    #[cfg(test)]
    mod tests {
        use std::{
            io::{Read, Write},
            os::fd::FromRawFd,
        };

        use super::*;

        #[tokio::test]
        async fn preserves_boundaries_in_both_directions() {
            let (bridge, endpoint) = LeafPacketBridge::pair().unwrap();
            let fd = endpoint.into_raw_fd();
            let mut leaf = unsafe { std::fs::File::from_raw_fd(fd) };

            bridge.send_to_leaf(&[1, 2, 3]).await.unwrap();
            bridge.send_to_leaf(&[4, 5]).await.unwrap();

            let mut packet = [0u8; 16];
            assert_eq!(leaf.read(&mut packet).unwrap(), 3);
            assert_eq!(&packet[..3], &[1, 2, 3]);
            assert_eq!(leaf.read(&mut packet).unwrap(), 2);
            assert_eq!(&packet[..2], &[4, 5]);

            leaf.write_all(&[6, 7, 8, 9]).unwrap();
            assert_eq!(bridge.recv_from_leaf(&mut packet).await.unwrap(), 4);
            assert_eq!(&packet[..4], &[6, 7, 8, 9]);
        }

        #[tokio::test]
        async fn rejects_oversized_packet() {
            let (bridge, _endpoint) = LeafPacketBridge::pair().unwrap();
            let packet = vec![0; MAX_PACKET_SIZE + 1];
            assert!(matches!(
                bridge.send_to_leaf(&packet).await,
                Err(PacketError::TooLarge)
            ));
        }
    }
}

#[cfg(unix)]
pub use unix_bridge::{LeafPacketBridge, LeafPacketEndpoint};

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4(destination: [u8; 4]) -> [u8; MIN_IPV4_HEADER] {
        let mut packet = [0u8; MIN_IPV4_HEADER];
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&(MIN_IPV4_HEADER as u16).to_be_bytes());
        packet[16..20].copy_from_slice(&destination);
        packet
    }

    fn ipv6(destination: [u8; 16]) -> [u8; MIN_IPV6_HEADER] {
        let mut packet = [0u8; MIN_IPV6_HEADER];
        packet[0] = 0x60;
        packet[24..40].copy_from_slice(&destination);
        packet
    }

    #[test]
    fn classifies_ipv4_and_ipv6_without_payload_copy() {
        let classifier = PacketClassifier::new(MeshRouteSnapshot::new(vec![
            "10.44.0.0/16".parse().unwrap(),
            "fd00:44::/48".parse().unwrap(),
        ]));
        assert_eq!(
            classifier.classify(&ipv4([10, 44, 2, 3])).unwrap(),
            PacketClass::Mesh
        );
        assert_eq!(
            classifier.classify(&ipv4([1, 1, 1, 1])).unwrap(),
            PacketClass::Policy
        );
        assert_eq!(
            classifier
                .classify(&ipv6(
                    "fd00:44::8".parse::<std::net::Ipv6Addr>().unwrap().octets()
                ))
                .unwrap(),
            PacketClass::Mesh
        );
    }

    #[test]
    fn atomically_replaces_route_snapshot() {
        let classifier = PacketClassifier::new(MeshRouteSnapshot::default());
        let packet = ipv4([10, 44, 2, 3]);
        assert_eq!(classifier.classify(&packet).unwrap(), PacketClass::Policy);
        classifier.replace_routes(MeshRouteSnapshot::new(vec![
            "10.44.0.0/16".parse().unwrap(),
        ]));
        assert_eq!(classifier.classify(&packet).unwrap(), PacketClass::Mesh);
    }

    #[test]
    fn preserves_mesh_broadcast_and_multicast_paths() {
        let classifier = PacketClassifier::new(MeshRouteSnapshot::default());
        assert_eq!(
            classifier.classify(&ipv4([255, 255, 255, 255])).unwrap(),
            PacketClass::Mesh
        );
        assert_eq!(
            classifier.classify(&ipv4([224, 0, 0, 251])).unwrap(),
            PacketClass::Mesh
        );
        assert_eq!(
            classifier
                .classify(&ipv6(
                    "ff02::fb".parse::<std::net::Ipv6Addr>().unwrap().octets()
                ))
                .unwrap(),
            PacketClass::Mesh
        );
    }

    #[test]
    fn rejects_malformed_and_oversized_packets() {
        let classifier = PacketClassifier::new(MeshRouteSnapshot::default());
        let mut malformed = ipv4([1, 1, 1, 1]);
        malformed[0] = 0x44;
        assert!(matches!(
            classifier.classify(&malformed),
            Err(PacketError::Malformed { version: 4 })
        ));
        assert!(matches!(
            classifier.classify(&vec![0x60; MAX_PACKET_SIZE + 1]),
            Err(PacketError::TooLarge)
        ));
    }
}
