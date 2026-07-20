use std::{
    net::IpAddr,
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Sink, Stream};
use smoltcp::wire::IpProtocol;
use tokio::sync::mpsc::{Receiver, channel};
use tokio_util::sync::PollSender;
use tracing::{debug, trace};

use crate::{
    filter::{IpFilter, IpFilters},
    packet::{AnyIpPktFrame, IpPacket},
    runner::Runner,
    tcp::TcpListener,
    udp::UdpSocket,
};

pub struct StackBuilder {
    enable_udp: bool,
    enable_tcp: bool,
    enable_icmp: bool,
    stack_buffer_size: usize,
    udp_buffer_size: usize,
    tcp_buffer_size: usize,
    ip_filters: IpFilters<'static>,
}

impl Default for StackBuilder {
    fn default() -> Self {
        Self {
            enable_udp: false,
            enable_tcp: false,
            enable_icmp: false,
            stack_buffer_size: 1024,
            udp_buffer_size: 512,
            tcp_buffer_size: 512,
            ip_filters: IpFilters::with_non_broadcast(),
        }
    }
}

#[allow(unused)]
impl StackBuilder {
    pub fn enable_udp(mut self, enable: bool) -> Self {
        self.enable_udp = enable;
        self
    }

    pub fn enable_tcp(mut self, enable: bool) -> Self {
        self.enable_tcp = enable;
        self
    }

    pub fn enable_icmp(mut self, enable: bool) -> Self {
        self.enable_icmp = enable;
        self
    }

    pub fn stack_buffer_size(mut self, size: usize) -> Self {
        self.stack_buffer_size = size;
        self
    }

    pub fn udp_buffer_size(mut self, size: usize) -> Self {
        self.udp_buffer_size = size;
        self
    }

    pub fn tcp_buffer_size(mut self, size: usize) -> Self {
        self.tcp_buffer_size = size;
        self
    }

    pub fn set_ip_filters(mut self, filters: IpFilters<'static>) -> Self {
        self.ip_filters = filters;
        self
    }

    pub fn add_ip_filter(mut self, filter: IpFilter<'static>) -> Self {
        self.ip_filters.add(filter);
        self
    }

    pub fn add_ip_filter_fn<F>(mut self, filter: F) -> Self
    where
        F: Fn(&IpAddr, &IpAddr) -> bool + Send + Sync + 'static,
    {
        self.ip_filters.add_fn(filter);
        self
    }

    #[allow(clippy::type_complexity)]
    pub fn build(
        self,
    ) -> std::io::Result<(
        Stack,
        Option<Runner>,
        Option<UdpSocket>,
        Option<TcpListener>,
    )> {
        let (stack_tx, stack_rx) = channel(self.stack_buffer_size);

        let (udp_tx, udp_rx) = if self.enable_udp {
            let (udp_tx, udp_rx) = channel(self.udp_buffer_size);
            (Some(udp_tx), Some(udp_rx))
        } else {
            (None, None)
        };

        let (tcp_tx, tcp_rx) = if self.enable_tcp {
            let (tcp_tx, tcp_rx) = channel(self.tcp_buffer_size);
            (Some(tcp_tx), Some(tcp_rx))
        } else {
            (None, None)
        };

        // ICMP is handled by TCP's Interface.
        // smoltcp's interface will always send replies to EchoRequest
        if self.enable_icmp && !self.enable_tcp {
            use std::io::{Error, ErrorKind::InvalidInput};
            return Err(Error::new(InvalidInput, "ICMP requires TCP"));
        }
        let icmp_tx = if self.enable_icmp {
            tcp_tx.clone()
        } else {
            None
        };

        let udp_socket = udp_rx.map(|udp_rx| UdpSocket::new(udp_rx, stack_tx.clone()));

        let (tcp_runner, tcp_listener) = if let Some(tcp_rx) = tcp_rx {
            let (tcp_runner, tcp_listener) = TcpListener::new(tcp_rx, stack_tx)?;
            (Some(tcp_runner), Some(tcp_listener))
        } else {
            (None, None)
        };

        let stack = Stack {
            ip_filters: self.ip_filters,
            stack_rx,
            sink_buf: None,
            udp_tx: udp_tx.map(PollSender::new),
            tcp_tx: tcp_tx.map(PollSender::new),
            icmp_tx: icmp_tx.map(PollSender::new),
        };

        Ok((stack, tcp_runner, udp_socket, tcp_listener))
    }
}

pub struct Stack {
    ip_filters: IpFilters<'static>,
    sink_buf: Option<(AnyIpPktFrame, IpProtocol)>,
    udp_tx: Option<PollSender<AnyIpPktFrame>>,
    tcp_tx: Option<PollSender<AnyIpPktFrame>>,
    icmp_tx: Option<PollSender<AnyIpPktFrame>>,
    stack_rx: Receiver<AnyIpPktFrame>,
}

impl Stack {
    fn poll_send(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let (item, proto) = match self.sink_buf.take() {
            Some(val) => val,
            None => return Poll::Ready(Ok(())),
        };

        let sender = match proto {
            IpProtocol::Tcp => self.tcp_tx.as_mut(),
            IpProtocol::Udp => self.udp_tx.as_mut(),
            IpProtocol::Icmp | IpProtocol::Icmpv6 => self.icmp_tx.as_mut(),
            _ => unreachable!(),
        };

        let Some(sender) = sender else {
            return Poll::Ready(Ok(()));
        };

        // PollSender retains the channel reserve future and registers this task's
        // waker. Returning Pending after try_reserve(Full) loses that wakeup and can
        // permanently stop Leaf's TUN-to-stack sibling future once this queue fills.
        match sender.poll_reserve(cx) {
            Poll::Pending => {
                self.sink_buf.replace((item, proto));
                Poll::Pending
            }
            Poll::Ready(Ok(())) => {
                let result = sender
                    .send_item(item)
                    .map_err(|_| channel_closed_err("channel is closed"));
                Poll::Ready(result)
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(channel_closed_err("channel is closed"))),
        }
    }
}

// Recv from stack.
impl Stream for Stack {
    type Item = std::io::Result<AnyIpPktFrame>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.stack_rx.poll_recv(cx) {
            Poll::Ready(Some(pkt)) => Poll::Ready(Some(Ok(pkt))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

// Send to stack.
impl Sink<AnyIpPktFrame> for Stack {
    type Error = std::io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_send(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: AnyIpPktFrame) -> Result<(), Self::Error> {
        if item.is_empty() {
            return Ok(());
        }

        use std::io::{Error, ErrorKind::InvalidInput};
        let packet = IpPacket::new_checked(item.as_slice())
            .map_err(|err| Error::new(InvalidInput, format!("invalid IP packet: {err}")))?;

        let src_ip = packet.src_addr();
        let dst_ip = packet.dst_addr();

        let addr_allowed = self.ip_filters.is_allowed(&src_ip, &dst_ip);
        if !addr_allowed {
            trace!("IP packet {src_ip} -> {dst_ip} (allowed? {addr_allowed}) throwing away",);
            return Ok(());
        }

        let protocol = packet.protocol();
        if matches!(
            protocol,
            IpProtocol::Tcp | IpProtocol::Udp | IpProtocol::Icmp | IpProtocol::Icmpv6
        ) {
            self.sink_buf.replace((item, protocol));
        } else {
            debug!("tun IP packet ignored (protocol: {:?})", protocol);
        }

        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_send(cx)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.stack_rx.close();
        Poll::Ready(Ok(()))
    }
}

fn channel_closed_err<E>(err: E) -> std::io::Error
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, err)
}

#[cfg(test)]
mod tests {
    use std::{future::Future, time::Duration};

    use futures::{SinkExt, future::poll_fn};
    use tokio::sync::{mpsc::channel, oneshot};
    use tokio_util::sync::PollSender;

    use super::Stack;
    use crate::filter::IpFilters;

    fn tcp_packet() -> Vec<u8> {
        let mut packet = vec![0_u8; 40];
        let packet_len = packet.len() as u16;
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&packet_len.to_be_bytes());
        packet[8] = 64;
        packet[9] = 6;
        packet[12..16].copy_from_slice(&[10, 0, 0, 1]);
        packet[16..20].copy_from_slice(&[10, 0, 0, 2]);
        packet[20..22].copy_from_slice(&12345_u16.to_be_bytes());
        packet[22..24].copy_from_slice(&80_u16.to_be_bytes());
        packet[32] = 0x50;
        packet
    }

    #[tokio::test]
    async fn full_ingress_channel_wakes_waiting_stack_sender() {
        let (tcp_tx, mut tcp_rx) = channel(1);
        let (_stack_tx, stack_rx) = channel(1);
        let mut stack = Stack {
            ip_filters: IpFilters::with_non_broadcast(),
            sink_buf: None,
            udp_tx: None,
            tcp_tx: Some(PollSender::new(tcp_tx)),
            icmp_tx: None,
            stack_rx,
        };

        stack.send(tcp_packet()).await.unwrap();

        let (polled_tx, polled_rx) = oneshot::channel();
        let sender = tokio::spawn(async move {
            let mut send = Box::pin(stack.send(tcp_packet()));
            let mut polled_tx = Some(polled_tx);
            poll_fn(move |cx| {
                let result = send.as_mut().poll(cx);
                if let Some(polled_tx) = polled_tx.take() {
                    let _ = polled_tx.send(());
                }
                result
            })
            .await
        });

        polled_rx.await.unwrap();
        assert!(!sender.is_finished());
        assert!(tcp_rx.recv().await.is_some());

        tokio::time::timeout(Duration::from_millis(250), sender)
            .await
            .expect("sender was not woken after channel capacity returned")
            .expect("sender task panicked")
            .expect("stack send failed");
    }
}
