use std::{
    pin::Pin,
    task::ready,
    task::{Context, Poll},
};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;

use crate::endpoint::{ConnId, KcpCloseStatus, KcpEndpoint, KcpStreamOwner, KcpStreamReceiver};

pub struct KcpStream {
    sender: tokio_util::sync::PollSender<BytesMut>,
    receiver: KcpStreamReceiver,
    closed: watch::Receiver<KcpCloseStatus>,
    send_drained: watch::Receiver<bool>,
    conn_id: ConnId,
    conn_data: Bytes,
    owner: KcpStreamOwner,

    partial_recv_buf: Option<BytesMut>,
}

impl std::fmt::Debug for KcpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KcpStream")
            .field("conn_id", &self.conn_id)
            .finish()
    }
}

impl KcpStream {
    pub fn new(endpoint: &KcpEndpoint, conn_id: ConnId) -> Option<Self> {
        let (sender, receiver, closed, send_drained) = endpoint.conn_stream_parts(conn_id)?;
        let conn_data = endpoint.conn_data(&conn_id)?;
        let owner = endpoint.stream_owner(conn_id);
        Some(Self {
            sender: tokio_util::sync::PollSender::new(sender),
            receiver,
            closed,
            send_drained,
            conn_id,
            conn_data,
            owner,

            partial_recv_buf: None,
        })
    }

    pub fn conn_data(&self) -> &Bytes {
        &self.conn_data
    }

    pub fn conn_id(&self) -> ConnId {
        self.conn_id
    }

    pub async fn shutdown_gracefully(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<()> {
        self.sender.close();
        match *self.closed.borrow() {
            KcpCloseStatus::Graceful => {
                self.owner.disarm();
                return Ok(());
            }
            KcpCloseStatus::Forced => {
                self.owner.disarm();
                return Err(forced_close_error());
            }
            KcpCloseStatus::Open => {}
        }

        match tokio::time::timeout(
            timeout,
            self.closed
                .wait_for(|status| *status != KcpCloseStatus::Open),
        )
        .await
        {
            Ok(Ok(status)) if *status == KcpCloseStatus::Graceful => {
                self.owner.disarm();
                Ok(())
            }
            Ok(Ok(_)) => {
                self.owner.disarm();
                Err(forced_close_error())
            }
            Ok(Err(_)) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "KCP endpoint closed before graceful shutdown completed",
            )),
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for KCP graceful shutdown",
            )),
        }
    }

    pub async fn drain_send_buffer(&mut self, timeout: std::time::Duration) -> std::io::Result<()> {
        self.sender.close();
        if *self.send_drained.borrow() {
            return Ok(());
        }
        match tokio::time::timeout(timeout, self.send_drained.wait_for(|drained| *drained)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(_)) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "KCP endpoint closed before the send buffer drained",
            )),
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for the KCP send buffer to drain",
            )),
        }
    }
}

fn forced_close_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "KCP graceful shutdown deadline expired; connection was force-closed",
    )
}

impl AsyncRead for KcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut ReadBuf,
    ) -> Poll<std::io::Result<()>> {
        let mut partial_recved = false;
        if let Some(partial_recv_buf) = &mut self.partial_recv_buf {
            assert!(!partial_recv_buf.is_empty());
            partial_recved = true;

            let len = std::cmp::min(buf.remaining(), partial_recv_buf.len());
            buf.put_slice(&partial_recv_buf.split_to(len));

            if partial_recv_buf.is_empty() {
                self.partial_recv_buf = None;
            }

            if buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
        }

        loop {
            let recv_ret = self.receiver.poll_recv(cx);
            match recv_ret {
                Poll::Ready(Some(mut read_buf)) => {
                    partial_recved = true;

                    let len = std::cmp::min(buf.remaining(), read_buf.len());
                    buf.put_slice(&read_buf[..len]);

                    if len < read_buf.len() {
                        self.partial_recv_buf = Some(read_buf.split_off(len));
                    }

                    if buf.remaining() == 0 {
                        return Poll::Ready(Ok(()));
                    }
                }
                Poll::Ready(None) => {
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => {
                    if partial_recved {
                        return Poll::Ready(Ok(()));
                    } else {
                        return Poll::Pending;
                    }
                }
            }
        }
    }
}

impl AsyncWrite for KcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut ret = ready!(self.sender.poll_reserve(cx));
        if ret.is_ok() {
            ret = self.sender.send_item(BytesMut::from(buf));
        }
        match ret {
            Ok(_) => Poll::Ready(Ok(buf.len())),
            Err(_) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stream closed",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context) -> Poll<std::io::Result<()>> {
        self.sender.close();
        Poll::Ready(Ok(()))
    }
}
