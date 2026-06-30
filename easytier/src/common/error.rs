use std::{io, result};
use thiserror::Error;

use crate::tunnel;

use super::PeerId;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error")]
    IOError(#[from] io::Error),

    #[cfg(feature = "tun")]
    #[error("rust tun error {0}")]
    TunError(#[from] tun::Error),

    #[error("tunnel error {0}")]
    TunnelError(#[from] tunnel::TunnelError),
    #[error("Peer has no conn, PeerId: {0}")]
    PeerNoConnectionError(PeerId),
    #[error("RouteError: {0:?}")]
    RouteError(Option<String>),
    #[error("Not found")]
    NotFound,
    #[error("Invalid Url: {0}")]
    InvalidUrl(String),
    #[error("Shell Command error: {0}")]
    ShellCommandError(String),
    // #[error("Rpc listen error: {0}")]
    // RpcListenError(String),
    #[error("Rpc connect error: {0}")]
    RpcConnectError(String),
    #[error("Timeout error: {0}")]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error("url in blacklist")]
    UrlInBlacklist,
    #[error("protocol loop suppressed for this runtime: {0}")]
    ProtocolLoopSuppressed(String),
    #[error("unknown data store error")]
    Unknown,
    #[error("anyhow error: {0}")]
    AnyhowError(#[from] anyhow::Error),

    #[error("wait resp error: {0}")]
    WaitRespError(String),

    #[error("message decode error: {0}")]
    MessageDecodeError(String),

    #[error("secret key error: {0}")]
    SecretKeyError(String),

    #[error("noise protocol error: {0}")]
    NoiseError(#[from] snow::Error),
}

pub type Result<T> = result::Result<T, Error>;

pub type ErrorCollection = crate::utils::error::ErrorCollection<Error>;

impl Error {
    /// High-confidence loop signal: we completed the underlay handshake only to
    /// discover we connected back to ourselves.
    pub fn is_self_loop_signal(&self) -> bool {
        match self {
            Error::WaitRespError(msg) => msg.contains("peer id conflict"),
            Error::AnyhowError(err) => err.chain().any(|cause| {
                let msg = cause.to_string();
                msg.contains("peer id conflict")
            }),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn self_loop_signal_matches_only_self_connect_errors() {
        assert!(Error::WaitRespError("peer id conflict".to_owned()).is_self_loop_signal());
        assert!(
            Error::WaitRespError("peer id conflict, are you connecting to yourself?".to_owned())
                .is_self_loop_signal()
        );
        assert!(!Error::WaitRespError("wait handshake timeout".to_owned()).is_self_loop_signal());
        assert!(!Error::UrlInBlacklist.is_self_loop_signal());
    }
}
