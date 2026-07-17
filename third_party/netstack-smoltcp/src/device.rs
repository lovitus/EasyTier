use std::io::{Error, ErrorKind};

use smoltcp::{
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
    time::Instant,
};
use tokio::sync::mpsc::{channel, error::TrySendError, OwnedPermit, Permit, Receiver, Sender};

use crate::packet::AnyIpPktFrame;

pub(super) struct VirtualDevice {
    in_buf: Receiver<Vec<u8>>,
    out_buf: Sender<AnyIpPktFrame>,
    reserved_output: Option<OwnedPermit<AnyIpPktFrame>>,
    output_blocked_this_poll: bool,
}

impl VirtualDevice {
    pub(super) fn new(
        iface_egress_tx: Sender<AnyIpPktFrame>,
        ingress_capacity: usize,
    ) -> (Self, Sender<Vec<u8>>) {
        let (iface_ingress_tx, iface_ingress_rx) = channel(ingress_capacity);
        (
            Self {
                in_buf: iface_ingress_rx,
                out_buf: iface_egress_tx,
                reserved_output: None,
                output_blocked_this_poll: false,
            },
            iface_ingress_tx,
        )
    }

    pub(super) fn begin_poll(&mut self) {
        self.output_blocked_this_poll = false;
    }

    pub(super) fn output_blocked_this_poll(&self) -> bool {
        self.output_blocked_this_poll
    }

    pub(super) fn output_closed(&self) -> bool {
        self.out_buf.is_closed()
    }

    pub(super) async fn wait_output_capacity(&mut self) -> std::io::Result<()> {
        if self.reserved_output.is_some() {
            return Ok(());
        }

        self.reserved_output =
            Some(self.out_buf.clone().reserve_owned().await.map_err(|_| {
                Error::new(ErrorKind::BrokenPipe, "stack output channel is closed")
            })?);
        Ok(())
    }

    pub(super) fn release_unused_output_permit(&mut self) {
        drop(self.reserved_output.take());
    }

    pub(super) async fn wait_output_closed(&self) {
        self.out_buf.closed().await;
    }

    fn reserve_output(&mut self) -> Option<VirtualOutputPermit<'_>> {
        if let Some(permit) = self.reserved_output.take() {
            return Some(VirtualOutputPermit::Owned(permit));
        }

        match self.out_buf.try_reserve() {
            Ok(permit) => Some(VirtualOutputPermit::Borrowed(permit)),
            Err(TrySendError::Full(_)) => {
                // Keep this sticky for the complete smoltcp poll. Capacity may be
                // restored before the runner examines it, but the interrupted poll
                // still has immediate work that must be retried.
                self.output_blocked_this_poll = true;
                None
            }
            Err(TrySendError::Closed(_)) => None,
        }
    }
}

impl Device for VirtualDevice {
    type RxToken<'a> = VirtualRxToken;
    type TxToken<'a> = VirtualTxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // smoltcp requires an RxToken and a TxToken together. Reserve output before
        // removing ingress so output backpressure cannot silently drop the packet.
        let Self {
            in_buf,
            out_buf,
            reserved_output,
            output_blocked_this_poll,
        } = self;
        let permit = if let Some(permit) = reserved_output.take() {
            VirtualOutputPermit::Owned(permit)
        } else {
            match out_buf.try_reserve() {
                Ok(permit) => VirtualOutputPermit::Borrowed(permit),
                Err(TrySendError::Full(_)) => {
                    *output_blocked_this_poll = true;
                    return None;
                }
                Err(TrySendError::Closed(_)) => return None,
            }
        };
        let buffer = match in_buf.try_recv() {
            Ok(buffer) => buffer,
            Err(_) => {
                if let VirtualOutputPermit::Owned(permit) = permit {
                    *reserved_output = Some(permit);
                }
                return None;
            }
        };

        Some((Self::RxToken { buffer }, Self::TxToken { permit }))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        self.reserve_output().map(|permit| Self::TxToken { permit })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ip;
        capabilities.max_transmission_unit = 1504;
        capabilities
    }
}

pub(super) struct VirtualRxToken {
    buffer: Vec<u8>,
}

impl RxToken for VirtualRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buffer[..])
    }
}

enum VirtualOutputPermit<'a> {
    Borrowed(Permit<'a, AnyIpPktFrame>),
    Owned(OwnedPermit<AnyIpPktFrame>),
}

pub(super) struct VirtualTxToken<'a> {
    permit: VirtualOutputPermit<'a>,
}

impl<'a> TxToken for VirtualTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = vec![0u8; len];
        let result = f(&mut buffer);
        match self.permit {
            VirtualOutputPermit::Borrowed(permit) => permit.send(buffer),
            VirtualOutputPermit::Owned(permit) => drop(permit.send(buffer)),
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{future::poll_fn, poll};
    use std::time::Duration as StdDuration;
    use tokio_util::sync::PollSender;

    #[tokio::test]
    async fn full_output_preserves_ingress_until_capacity_returns() {
        let (out_tx, mut out_rx) = channel(1);
        out_tx.send(vec![0xaa]).await.unwrap();
        let (mut device, in_tx) = VirtualDevice::new(out_tx, 2);
        in_tx.send(vec![1, 2, 3]).await.unwrap();

        device.begin_poll();
        assert!(device.receive(Instant::from_millis(0)).is_none());
        assert!(device.output_blocked_this_poll());
        assert_eq!(in_tx.capacity(), 1, "ingress packet must remain queued");

        assert_eq!(out_rx.recv().await.unwrap(), vec![0xaa]);
        tokio::time::timeout(StdDuration::from_secs(1), device.wait_output_capacity())
            .await
            .unwrap()
            .unwrap();
        assert!(
            device.output_blocked_this_poll(),
            "blocked state must remain sticky until the next poll"
        );

        let (rx, tx) = device.receive(Instant::from_millis(0)).unwrap();
        let packet = rx.consume(|buffer| buffer.to_vec());
        drop(tx);
        assert_eq!(packet, vec![1, 2, 3]);
        assert_eq!(in_tx.capacity(), 2);
    }

    #[tokio::test]
    async fn bounded_ingress_backpressures_and_preserves_order() {
        let (out_tx, _out_rx) = channel(1);
        let (mut device, in_tx) = VirtualDevice::new(out_tx, 2);
        in_tx.send(vec![1]).await.unwrap();
        in_tx.send(vec![2]).await.unwrap();

        let blocked_sender = in_tx.clone();
        let blocked_send = tokio::spawn(async move { blocked_sender.send(vec![3]).await });
        tokio::task::yield_now().await;
        assert!(!blocked_send.is_finished());

        let mut packets = Vec::new();
        let (rx, tx) = device.receive(Instant::from_millis(0)).unwrap();
        packets.push(rx.consume(|buffer| buffer.to_vec()));
        drop(tx);
        tokio::time::timeout(StdDuration::from_secs(1), blocked_send)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        for _ in 0..2 {
            let (rx, tx) = device.receive(Instant::from_millis(0)).unwrap();
            packets.push(rx.consume(|buffer| buffer.to_vec()));
            drop(tx);
        }
        assert_eq!(packets, vec![vec![1], vec![2], vec![3]]);
    }

    #[tokio::test]
    async fn capacity_wait_makes_progress_with_queued_output_sender() {
        let (out_tx, mut out_rx) = channel(1);
        out_tx.send(vec![0xaa]).await.unwrap();
        let (mut device, in_tx) = VirtualDevice::new(out_tx.clone(), 1);
        in_tx.send(vec![1, 2, 3]).await.unwrap();

        device.begin_poll();
        assert!(device.receive(Instant::from_millis(0)).is_none());
        assert!(device.output_blocked_this_poll());

        // Queue the runner first, then a second output producer that models the
        // UDP PollSender sharing stack_tx. Tokio's bounded channel assigns newly
        // released capacity to queued reserve calls in FIFO order.
        let mut runner_wait = Box::pin(device.wait_output_capacity());
        assert!(poll!(runner_wait.as_mut()).is_pending());

        let mut competing_sender = PollSender::new(out_tx);
        let mut competing_reserve = Box::pin(poll_fn(|cx| competing_sender.poll_reserve(cx)));
        assert!(poll!(competing_reserve.as_mut()).is_pending());

        assert_eq!(out_rx.recv().await.unwrap(), vec![0xaa]);
        runner_wait.await.unwrap();

        device.begin_poll();
        assert!(
            device.receive(Instant::from_millis(0)).is_some(),
            "capacity granted to the runner must produce smoltcp progress before a queued sender takes it"
        );
    }

    #[tokio::test]
    async fn unused_reserved_output_capacity_is_released_after_poll() {
        let (out_tx, _out_rx) = channel(1);
        let (mut device, _in_tx) = VirtualDevice::new(out_tx.clone(), 1);

        device.wait_output_capacity().await.unwrap();
        assert_eq!(
            out_tx.capacity(),
            0,
            "runner must retain the granted permit"
        );

        device.release_unused_output_permit();
        assert_eq!(
            out_tx.capacity(),
            1,
            "an idle poll must not permanently reduce output capacity"
        );
    }
}
