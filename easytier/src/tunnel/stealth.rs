//! Stealth outer-protection primitives (v1, UDP).
//!
//! Two-phase model (see plan `stealth-obfuscation`):
//!
//! * Phase 1 — a `network_secret`-derived, time-windowed **pre-auth gate** used
//!   to decide whether to respond to a peer at all. It authenticates the UDP
//!   transport `Syn`/`Sack` so an unauthenticated active prober receives no
//!   distinguishable response.
//! * Phase 2 — once the Noise handshake completes, both sides derive a
//!   connection-level `outer_key` from the handshake hash and use it (instead of
//!   the rolling time-window key) for the lifetime of the connection.
//!
//! This module only contains the feature-independent primitives: key
//! derivation, gate-token build/verify, a bounded per-window replay set, and the
//! shared [`OuterSessionState`] used to hand the phase-2 key from `PeerConn` down
//! to the running tunnel without a dedicated control API. The datagram AEAD body
//! (which depends on the feature-gated cipher backend) is layered on top of this
//! when wiring the UDP path.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hmac::{Hmac, Mac as _};
use rand::RngCore as _;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Default rolling window (seconds) for the phase-1 gate key. The receiver
/// accepts the current and previous window to tolerate clock skew.
pub const DEFAULT_GATE_WINDOW_SECS: u64 = 60;

/// Length of the random nonce carried in a gate token.
pub const GATE_NONCE_LEN: usize = 16;
/// Length of the truncated HMAC tag carried in a gate token.
pub const GATE_TAG_LEN: usize = 16;
/// Total wire size of a gate token (replaces the legacy 8-byte SYN magic body).
pub const GATE_TOKEN_LEN: usize = GATE_NONCE_LEN + GATE_TAG_LEN;
pub const STREAM_GATE_PREFACE_LEN: usize = 4 + GATE_TOKEN_LEN;

/// HKDF-Extract+Expand (RFC 5869) restricted to a single 32-byte output block,
/// implemented with HMAC-SHA256 only (no extra crate dependency). Mirrors the
/// derivation style already used by `secure_datagram.rs`.
fn hkdf_sha256(secret: &[u8], info: &[u8]) -> [u8; 32] {
    // Extract with an all-zero salt.
    let salt = [0u8; 32];
    let mut extract = HmacSha256::new_from_slice(&salt).expect("hmac accepts any key length");
    extract.update(secret);
    let prk = extract.finalize().into_bytes();

    // Expand a single block (T(1)).
    let mut expand = HmacSha256::new_from_slice(&prk).expect("hmac accepts any key length");
    expand.update(info);
    expand.update(&[1u8]);
    let okm = expand.finalize().into_bytes();

    let mut out = [0u8; 32];
    out.copy_from_slice(&okm[..32]);
    out
}

/// Current unix time in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The rolling gate-key window index for a given timestamp.
pub fn window_for(now_secs: u64, window_secs: u64) -> u64 {
    now_secs / window_secs.max(1)
}

/// Derive the phase-1 gate key for a specific window from the raw
/// `network_secret`. The window index is mixed into the derivation `info` so the
/// key rotates automatically without any persisted state.
pub fn derive_gate_key(network_secret: &[u8], window: u64) -> [u8; 32] {
    let mut info = Vec::with_capacity(b"et-obfs-gate".len() + 8);
    info.extend_from_slice(b"et-obfs-gate");
    info.extend_from_slice(&window.to_be_bytes());
    hkdf_sha256(network_secret, &info)
}

/// Derive the phase-2 connection-level outer key from the completed Noise
/// handshake hash. Independent of the rolling time window, so the live
/// connection is unaffected by clock drift or window rotation.
pub fn derive_outer_key(handshake_hash: &[u8]) -> [u8; 32] {
    hkdf_sha256(handshake_hash, b"et-outer")
}

/// Compute the truncated authentication tag binding a nonce to a connection id
/// under a given gate key.
fn gate_tag(gate_key: &[u8; 32], nonce: &[u8; GATE_NONCE_LEN], conn_id: u32) -> [u8; GATE_TAG_LEN] {
    let mut mac = HmacSha256::new_from_slice(gate_key).expect("hmac accepts any key length");
    mac.update(b"et-gate-token");
    mac.update(nonce);
    mac.update(&conn_id.to_be_bytes());
    let full = mac.finalize().into_bytes();
    let mut tag = [0u8; GATE_TAG_LEN];
    tag.copy_from_slice(&full[..GATE_TAG_LEN]);
    tag
}

/// Constant-time comparison of two equal-length byte slices.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// A phase-1 gate token: a random nonce plus an HMAC tag over `nonce || conn_id`.
/// Carried in place of the legacy 8-byte SYN/SACK magic body.
#[derive(Clone, Copy, Debug)]
pub struct GateToken {
    pub nonce: [u8; GATE_NONCE_LEN],
    pub tag: [u8; GATE_TAG_LEN],
}

impl GateToken {
    /// Build a fresh token for the current window.
    pub fn new(network_secret: &[u8], conn_id: u32, window_secs: u64) -> Self {
        let mut nonce = [0u8; GATE_NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let window = window_for(now_secs(), window_secs);
        let key = derive_gate_key(network_secret, window);
        let tag = gate_tag(&key, &nonce, conn_id);
        Self { nonce, tag }
    }

    /// Serialize to the on-wire byte layout (`nonce || tag`).
    pub fn to_bytes(&self) -> [u8; GATE_TOKEN_LEN] {
        let mut out = [0u8; GATE_TOKEN_LEN];
        out[..GATE_NONCE_LEN].copy_from_slice(&self.nonce);
        out[GATE_NONCE_LEN..].copy_from_slice(&self.tag);
        out
    }

    /// Parse from the on-wire byte layout. Returns `None` on length mismatch.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() != GATE_TOKEN_LEN {
            return None;
        }
        let mut nonce = [0u8; GATE_NONCE_LEN];
        let mut tag = [0u8; GATE_TAG_LEN];
        nonce.copy_from_slice(&buf[..GATE_NONCE_LEN]);
        tag.copy_from_slice(&buf[GATE_NONCE_LEN..]);
        Some(Self { nonce, tag })
    }

    /// Verify the token against the current and previous windows (skew
    /// tolerance). Returns the window index that matched, or `None`.
    pub fn verify(&self, network_secret: &[u8], conn_id: u32, window_secs: u64) -> Option<u64> {
        let cur = window_for(now_secs(), window_secs);
        for window in [cur, cur.wrapping_sub(1)] {
            let key = derive_gate_key(network_secret, window);
            let expected = gate_tag(&key, &self.nonce, conn_id);
            if ct_eq(&expected, &self.tag) {
                return Some(window);
            }
        }
        None
    }
}

/// Bounded, time-windowed replay guard for gate-token nonces. It keeps only the
/// current and immediately previous windows so replay protection still works
/// across the accepted clock-skew window, while memory stays constant-sized and
/// auto-expiring.
#[derive(Debug)]
pub struct GateReplayGuard {
    inner: Mutex<GateReplayInner>,
    max_entries: usize,
}

#[derive(Debug, Default)]
struct GateReplaySlot {
    window: Option<u64>,
    seen: HashSet<[u8; GATE_NONCE_LEN]>,
}

#[derive(Debug, Default)]
struct GateReplayInner {
    current: GateReplaySlot,
    previous: GateReplaySlot,
}

impl GateReplayGuard {
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(GateReplayInner::default()),
            max_entries: max_entries.max(1),
        }
    }

    /// Record a `(window, nonce)` pair. Returns `true` if it is fresh (accept),
    /// `false` if it is a replay or falls outside the retained skew window.
    pub fn accept(&self, window: u64, nonce: &[u8; GATE_NONCE_LEN]) -> bool {
        let mut g = self.inner.lock().unwrap();
        let slot = if g.current.window == Some(window) {
            &mut g.current
        } else if g.previous.window == Some(window) {
            &mut g.previous
        } else {
            match g.current.window {
                None => {
                    g.current.window = Some(window);
                    &mut g.current
                }
                Some(current_window) if window == current_window.wrapping_sub(1) => {
                    g.previous.window = Some(window);
                    g.previous.seen.clear();
                    &mut g.previous
                }
                Some(current_window) if window > current_window => {
                    if window == current_window.saturating_add(1) {
                        g.previous = std::mem::take(&mut g.current);
                    } else {
                        g.previous = GateReplaySlot::default();
                    }
                    g.current.window = Some(window);
                    g.current.seen.clear();
                    &mut g.current
                }
                _ => return false,
            }
        };
        if slot.seen.contains(nonce) {
            return false;
        }
        // If the set is full for this window, refuse rather than grow unbounded.
        if slot.seen.len() >= self.max_entries {
            return false;
        }
        slot.seen.insert(*nonce);
        true
    }
}

impl Default for GateReplayGuard {
    fn default() -> Self {
        Self::new(4096)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VerifiedStreamGatePreface {
    conn_id: u32,
    nonce: [u8; GATE_NONCE_LEN],
    window: u64,
}

#[derive(Clone, Copy)]
enum StreamGateRole {
    Initiator,
    Responder,
}

fn stream_gate_tag(
    gate_key: &[u8; 32],
    nonce: &[u8; GATE_NONCE_LEN],
    conn_id: u32,
    role: StreamGateRole,
) -> [u8; GATE_TAG_LEN] {
    let mut mac = HmacSha256::new_from_slice(gate_key).expect("hmac accepts any key length");
    mac.update(b"et-stream-gate-v1");
    mac.update(match role {
        StreamGateRole::Initiator => b"initiator",
        StreamGateRole::Responder => b"responder",
    });
    mac.update(nonce);
    mac.update(&conn_id.to_be_bytes());
    let full = mac.finalize().into_bytes();
    let mut tag = [0u8; GATE_TAG_LEN];
    tag.copy_from_slice(&full[..GATE_TAG_LEN]);
    tag
}

fn encode_stream_gate_preface(
    state: &OuterSessionState,
    conn_id: u32,
    nonce: [u8; GATE_NONCE_LEN],
    window: u64,
    role: StreamGateRole,
) -> [u8; STREAM_GATE_PREFACE_LEN] {
    let key = derive_gate_key(state.network_secret(), window);
    let token = GateToken {
        nonce,
        tag: stream_gate_tag(&key, &nonce, conn_id, role),
    };
    let mut preface = [0u8; STREAM_GATE_PREFACE_LEN];
    preface[..4].copy_from_slice(&conn_id.to_be_bytes());
    preface[4..].copy_from_slice(&token.to_bytes());
    preface
}

pub fn build_stream_gate_preface(state: &OuterSessionState) -> [u8; STREAM_GATE_PREFACE_LEN] {
    let conn_id = rand::random::<u32>();
    let mut nonce = [0u8; GATE_NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let window = window_for(now_secs(), state.window_secs());
    encode_stream_gate_preface(state, conn_id, nonce, window, StreamGateRole::Initiator)
}

pub fn verify_stream_gate_preface(
    state: &OuterSessionState,
    replay_guard: &GateReplayGuard,
    preface: &[u8],
) -> Option<VerifiedStreamGatePreface> {
    if preface.len() != STREAM_GATE_PREFACE_LEN {
        return None;
    }
    let conn_id = u32::from_be_bytes(preface[..4].try_into().unwrap());
    let token = GateToken::from_bytes(&preface[4..])?;
    let current = window_for(now_secs(), state.window_secs());
    for window in [current, current.wrapping_sub(1)] {
        let key = derive_gate_key(state.network_secret(), window);
        let expected = stream_gate_tag(&key, &token.nonce, conn_id, StreamGateRole::Initiator);
        if ct_eq(&expected, &token.tag) && replay_guard.accept(window, &token.nonce) {
            return Some(VerifiedStreamGatePreface {
                conn_id,
                nonce: token.nonce,
                window,
            });
        }
    }
    None
}

pub fn build_stream_gate_ack(
    state: &OuterSessionState,
    request: &VerifiedStreamGatePreface,
) -> [u8; STREAM_GATE_PREFACE_LEN] {
    encode_stream_gate_preface(
        state,
        request.conn_id,
        request.nonce,
        request.window,
        StreamGateRole::Responder,
    )
}

pub fn verify_stream_gate_ack(state: &OuterSessionState, request: &[u8], ack: &[u8]) -> bool {
    if request.len() != STREAM_GATE_PREFACE_LEN || ack.len() != STREAM_GATE_PREFACE_LEN {
        return false;
    }
    let request_conn_id = u32::from_be_bytes(request[..4].try_into().unwrap());
    let ack_conn_id = u32::from_be_bytes(ack[..4].try_into().unwrap());
    let (Some(request_token), Some(ack_token)) = (
        GateToken::from_bytes(&request[4..]),
        GateToken::from_bytes(&ack[4..]),
    ) else {
        return false;
    };
    if request_conn_id != ack_conn_id || request_token.nonce != ack_token.nonce {
        return false;
    }
    let current = window_for(now_secs(), state.window_secs());
    [current, current.wrapping_sub(1)]
        .into_iter()
        .any(|window| {
            let key = derive_gate_key(state.network_secret(), window);
            let expected = stream_gate_tag(
                &key,
                &ack_token.nonce,
                ack_conn_id,
                StreamGateRole::Responder,
            );
            ct_eq(&expected, &ack_token.tag)
        })
}

/// Shared state handed to BOTH the running tunnel and `PeerConn`, used to switch
/// from the phase-1 gate key to the phase-2 connection-level outer key without a
/// dedicated downward control API. `PeerConn` writes the outer key once Noise
/// completes; the tunnel send/recv paths read it on each datagram.
#[derive(Debug)]
pub struct OuterSessionState {
    enabled: bool,
    window_secs: u64,
    network_secret: Vec<u8>,
    transition_mode: OuterTransitionMode,
    key_phase: RwLock<OuterKeyPhase>,
}

pub struct OuterSessionAssociation {
    state: Arc<OuterSessionState>,
    _transport_keepalive: Option<Box<dyn std::any::Any + Send>>,
}

impl OuterSessionAssociation {
    pub fn new(
        state: Arc<OuterSessionState>,
        transport_keepalive: Option<Box<dyn std::any::Any + Send>>,
    ) -> Self {
        Self {
            state,
            _transport_keepalive: transport_keepalive,
        }
    }

    pub fn state(&self) -> Arc<OuterSessionState> {
        self.state.clone()
    }
}

#[derive(Debug, Clone, Copy)]
enum OuterTransitionMode {
    NextSeal,
    TransportDelayed,
}

#[derive(Debug, Clone, Copy)]
enum OuterKeyPhase {
    Gate,
    PromoteAfterNextSeal([u8; 32]),
    Outer([u8; 32], Instant),
}

impl OuterSessionState {
    /// Create an enabled state seeded with the raw `network_secret`.
    pub fn new(network_secret: Vec<u8>, window_secs: u64) -> Self {
        Self {
            enabled: true,
            window_secs: window_secs.max(1),
            network_secret,
            transition_mode: OuterTransitionMode::NextSeal,
            key_phase: RwLock::new(OuterKeyPhase::Gate),
        }
    }

    /// A disabled, no-op state (stealth off). Used so the tunnel paths can hold a
    /// single non-optional handle and cheaply check `is_enabled()`.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            window_secs: DEFAULT_GATE_WINDOW_SECS,
            network_secret: Vec::new(),
            transition_mode: OuterTransitionMode::NextSeal,
            key_phase: RwLock::new(OuterKeyPhase::Gate),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn window_secs(&self) -> u64 {
        self.window_secs
    }

    pub fn network_secret(&self) -> &[u8] {
        &self.network_secret
    }

    /// Build a gate token for a new outbound connection.
    pub fn build_gate_token(&self, conn_id: u32) -> GateToken {
        GateToken::new(&self.network_secret, conn_id, self.window_secs)
    }

    /// Verify an inbound gate token. Returns the matched window on success.
    pub fn verify_gate_token(&self, token: &GateToken, conn_id: u32) -> Option<u64> {
        token.verify(&self.network_secret, conn_id, self.window_secs)
    }

    /// Phase-2: install the connection-level outer key derived from the Noise
    /// handshake hash. Idempotent and safe to call from `PeerConn`.
    pub fn set_outer_key_from_handshake_hash(&self, handshake_hash: &[u8]) {
        let key = derive_outer_key(handshake_hash);
        let mut phase = self.key_phase.write().unwrap();
        if matches!(*phase, OuterKeyPhase::Outer(current, _) if current == key) {
            return;
        }
        *phase = OuterKeyPhase::Outer(key, Instant::now());
    }

    /// Keep the next outbound datagram on the gate key, then atomically promote
    /// subsequent traffic to the phase-2 key. The Noise initiator uses this for
    /// msg3 because queueing the packet does not mean UDP has sealed it yet.
    pub fn promote_outer_key_after_next_seal(&self, handshake_hash: &[u8]) {
        if !self.enabled {
            return;
        }
        let key = derive_outer_key(handshake_hash);
        let mut phase = self.key_phase.write().unwrap();
        if matches!(
            *phase,
            OuterKeyPhase::PromoteAfterNextSeal(current) | OuterKeyPhase::Outer(current, _)
                if current == key
        ) {
            return;
        }
        *phase = match self.transition_mode {
            OuterTransitionMode::NextSeal => OuterKeyPhase::PromoteAfterNextSeal(key),
            OuterTransitionMode::TransportDelayed => OuterKeyPhase::Outer(key, Instant::now()),
        };
    }

    /// The current phase-2 outer key, if Noise has completed.
    pub fn outer_key(&self) -> Option<[u8; 32]> {
        match *self.key_phase.read().unwrap() {
            OuterKeyPhase::Outer(key, _) => Some(key),
            OuterKeyPhase::Gate | OuterKeyPhase::PromoteAfterNextSeal(_) => None,
        }
    }

    pub(crate) fn outer_key_elapsed(&self) -> Option<Duration> {
        match *self.key_phase.read().unwrap() {
            OuterKeyPhase::Outer(_, installed_at) => Some(installed_at.elapsed()),
            OuterKeyPhase::Gate | OuterKeyPhase::PromoteAfterNextSeal(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_outer_key_age_for_test(&self, age: Duration) {
        let mut phase = self.key_phase.write().unwrap();
        if let OuterKeyPhase::Outer(key, _) = *phase {
            *phase = OuterKeyPhase::Outer(key, Instant::now() - age);
        }
    }

    /// Create a fresh per-connection state from a listener/connector template.
    /// The new state keeps the same network secret + window, but starts with no
    /// phase-2 outer key so different live connections cannot share handshake state.
    pub fn fork_for_connection(&self) -> Arc<Self> {
        if !self.enabled {
            Arc::new(Self::disabled())
        } else {
            Arc::new(Self::new(self.network_secret.clone(), self.window_secs))
        }
    }

    /// Create state for a datagram transport whose scheduler can emit control
    /// packets independently of the peer packet sink. The transport provides
    /// its own bounded gate-to-outer delay.
    pub fn fork_for_transport_delayed_transition(&self) -> Arc<Self> {
        if !self.enabled {
            Arc::new(Self::disabled())
        } else {
            Arc::new(Self {
                enabled: true,
                window_secs: self.window_secs,
                network_secret: self.network_secret.clone(),
                transition_mode: OuterTransitionMode::TransportDelayed,
                key_phase: RwLock::new(OuterKeyPhase::Gate),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_key_is_deterministic_per_window() {
        let secret = b"network-secret";
        let k1 = derive_gate_key(secret, 100);
        let k2 = derive_gate_key(secret, 100);
        let k3 = derive_gate_key(secret, 101);
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn token_roundtrips_through_bytes() {
        let secret = b"network-secret";
        let token = GateToken::new(secret, 42, DEFAULT_GATE_WINDOW_SECS);
        let bytes = token.to_bytes();
        assert_eq!(bytes.len(), GATE_TOKEN_LEN);
        let parsed = GateToken::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.nonce, token.nonce);
        assert_eq!(parsed.tag, token.tag);
    }

    #[test]
    fn valid_token_verifies_wrong_secret_or_conn_rejected() {
        let secret = b"network-secret";
        let token = GateToken::new(secret, 7, DEFAULT_GATE_WINDOW_SECS);

        assert!(token.verify(secret, 7, DEFAULT_GATE_WINDOW_SECS).is_some());
        // Wrong conn id.
        assert!(token.verify(secret, 8, DEFAULT_GATE_WINDOW_SECS).is_none());
        // Wrong secret (a prober without network_secret).
        assert!(
            token
                .verify(b"other-secret", 7, DEFAULT_GATE_WINDOW_SECS)
                .is_none()
        );
    }

    #[test]
    fn whitespace_secret_does_not_enable_stealth() {
        assert!(!is_stealth_effectively_enabled(Some("   "), true, true));
        assert!(!build_outer_session(Some("   "), true, true, 0).is_enabled());
    }

    #[test]
    fn stream_gate_ack_is_direction_and_challenge_bound() {
        let state = OuterSessionState::new(b"stream-secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        let request = build_stream_gate_preface(&state);
        let verified =
            verify_stream_gate_preface(&state, &GateReplayGuard::default(), &request).unwrap();
        let ack = build_stream_gate_ack(&state, &verified);

        assert!(verify_stream_gate_ack(&state, &request, &ack));
        assert!(!verify_stream_gate_ack(&state, &request, &request));

        let other_request = build_stream_gate_preface(&state);
        assert!(!verify_stream_gate_ack(&state, &other_request, &ack));

        let wrong_state =
            OuterSessionState::new(b"other-secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        assert!(!verify_stream_gate_ack(&wrong_state, &request, &ack));
    }

    #[test]
    fn stream_gate_request_replay_is_rejected() {
        let state = OuterSessionState::new(b"stream-secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        let guard = GateReplayGuard::default();
        let request = build_stream_gate_preface(&state);

        assert!(verify_stream_gate_preface(&state, &guard, &request).is_some());
        assert!(verify_stream_gate_preface(&state, &guard, &request).is_none());
    }

    #[test]
    fn previous_window_is_accepted_for_skew() {
        let secret = b"network-secret";
        let window_secs = DEFAULT_GATE_WINDOW_SECS;
        let prev_window = window_for(now_secs(), window_secs).wrapping_sub(1);

        // Forge a token as if it had been built in the previous window.
        let mut nonce = [0u8; GATE_NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let key = derive_gate_key(secret, prev_window);
        let tag = gate_tag(&key, &nonce, 99);
        let token = GateToken { nonce, tag };

        assert_eq!(token.verify(secret, 99, window_secs), Some(prev_window));
    }

    #[test]
    fn replay_guard_rejects_duplicate_in_window() {
        let guard = GateReplayGuard::new(16);
        let nonce = [1u8; GATE_NONCE_LEN];
        assert!(guard.accept(10, &nonce));
        assert!(!guard.accept(10, &nonce));
        // New window still retains the previous window replay view.
        assert!(guard.accept(11, &nonce));
        assert!(!guard.accept(10, &nonce));
    }

    #[test]
    fn replay_guard_is_bounded() {
        let guard = GateReplayGuard::new(2);
        assert!(guard.accept(1, &[1u8; GATE_NONCE_LEN]));
        assert!(guard.accept(1, &[2u8; GATE_NONCE_LEN]));
        // Third distinct nonce in the same window exceeds the bound.
        assert!(!guard.accept(1, &[3u8; GATE_NONCE_LEN]));
    }

    #[test]
    fn replay_guard_retains_previous_window() {
        let guard = GateReplayGuard::new(4);
        let prev_nonce = [1u8; GATE_NONCE_LEN];
        let prev_nonce_fresh = [2u8; GATE_NONCE_LEN];
        let cur_nonce = [3u8; GATE_NONCE_LEN];

        assert!(guard.accept(20, &prev_nonce));
        assert!(guard.accept(21, &cur_nonce));
        assert!(!guard.accept(20, &prev_nonce));
        assert!(guard.accept(20, &prev_nonce_fresh));
        assert!(!guard.accept(19, &[4u8; GATE_NONCE_LEN]));
    }

    #[test]
    fn replay_guard_initializes_previous_window_after_current_seen() {
        let guard = GateReplayGuard::new(4);
        let cur_nonce = [7u8; GATE_NONCE_LEN];
        let prev_nonce = [8u8; GATE_NONCE_LEN];

        assert!(guard.accept(100, &cur_nonce));
        assert!(guard.accept(99, &prev_nonce));
        assert!(!guard.accept(99, &prev_nonce));
        assert!(!guard.accept(98, &[9u8; GATE_NONCE_LEN]));
    }

    #[test]
    fn outer_session_state_handoff() {
        let state = OuterSessionState::new(b"secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        assert!(state.is_enabled());
        assert!(state.outer_key().is_none());

        let token = state.build_gate_token(5);
        assert!(state.verify_gate_token(&token, 5).is_some());

        state.set_outer_key_from_handshake_hash(b"handshake-hash-material");
        assert!(state.outer_key().is_some());
        assert_eq!(
            state.outer_key().unwrap(),
            derive_outer_key(b"handshake-hash-material")
        );
    }

    #[test]
    fn forked_connection_state_is_independent() {
        let template = OuterSessionState::new(b"secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        let child_a = template.fork_for_connection();
        let child_b = template.fork_for_connection();

        child_a.set_outer_key_from_handshake_hash(b"handshake-a");

        assert!(template.outer_key().is_none());
        assert!(child_a.outer_key().is_some());
        assert!(child_b.outer_key().is_none());
    }

    #[test]
    fn transport_delayed_transition_records_outer_key_without_sealing() {
        let template = OuterSessionState::new(b"secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        let state = template.fork_for_transport_delayed_transition();

        state.promote_outer_key_after_next_seal(b"handshake");

        assert!(state.outer_key().is_some());
        state.set_outer_key_age_for_test(Duration::from_secs(10));
        state.set_outer_key_from_handshake_hash(b"handshake");
        assert!(state.outer_key_elapsed().unwrap() >= Duration::from_secs(10));
        assert!(template.outer_key().is_none());
    }

    #[test]
    fn disabled_state_is_noop() {
        let state = OuterSessionState::disabled();
        assert!(!state.is_enabled());
        assert!(state.outer_key().is_none());
    }
}

pub(crate) const OUTER_NONCE_LEN: usize = 12;
const OUTER_TAG_LEN: usize = 16;
/// Per-datagram overhead added by [`seal`].
pub const OUTER_OVERHEAD: usize = OUTER_NONCE_LEN + OUTER_TAG_LEN;

fn outer_subkeys(key: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    (
        hkdf_sha256(key, b"et-outer-enc"),
        hkdf_sha256(key, b"et-outer-mac"),
    )
}

fn apply_keystream(enc_key: &[u8; 32], nonce: &[u8; OUTER_NONCE_LEN], data: &mut [u8]) {
    let mut counter: u32 = 0;
    let mut offset = 0;
    while offset < data.len() {
        let mut mac = HmacSha256::new_from_slice(enc_key).expect("hmac key");
        mac.update(b"et-outer-strm");
        mac.update(nonce);
        mac.update(&counter.to_be_bytes());
        let block = mac.finalize().into_bytes();
        let n = (data.len() - offset).min(block.len());
        for i in 0..n {
            data[offset + i] ^= block[i];
        }
        offset += n;
        counter = counter.wrapping_add(1);
    }
}

fn outer_mac(
    mac_key: &[u8; 32],
    nonce: &[u8; OUTER_NONCE_LEN],
    ciphertext: &[u8],
) -> [u8; OUTER_TAG_LEN] {
    let mut mac = HmacSha256::new_from_slice(mac_key).expect("hmac key");
    mac.update(b"et-outer-tag");
    mac.update(nonce);
    mac.update(ciphertext);
    let full = mac.finalize().into_bytes();
    let mut tag = [0u8; OUTER_TAG_LEN];
    tag.copy_from_slice(&full[..OUTER_TAG_LEN]);
    tag
}

/// Seal `plaintext` under `key`, producing `nonce || ciphertext || tag`.
/// Encrypt-then-MAC built only on HMAC-SHA256, so it works in every cipher
/// feature configuration. The inner payload is already AEAD-protected by
/// `SecureDatagramSession`; this layer authenticates/obfuscates the outer
/// metadata (tunnel + peer-manager headers) and provides the anti-probe property.
pub fn seal(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let (enc_key, mac_key) = outer_subkeys(key);
    let mut nonce = [0u8; OUTER_NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);

    let mut out = Vec::with_capacity(OUTER_NONCE_LEN + plaintext.len() + OUTER_TAG_LEN);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(plaintext);
    apply_keystream(&enc_key, &nonce, &mut out[OUTER_NONCE_LEN..]);
    let tag = outer_mac(&mac_key, &nonce, &out[OUTER_NONCE_LEN..]);
    out.extend_from_slice(&tag);
    out
}

/// Open a buffer produced by [`seal`]. Returns the plaintext, or `None` if the
/// buffer is malformed or the tag does not verify (constant-time check).
pub fn open(key: &[u8; 32], buf: &[u8]) -> Option<Vec<u8>> {
    if buf.len() < OUTER_OVERHEAD {
        return None;
    }
    let (enc_key, mac_key) = outer_subkeys(key);
    let nonce: [u8; OUTER_NONCE_LEN] = buf[..OUTER_NONCE_LEN].try_into().ok()?;
    let ct_end = buf.len() - OUTER_TAG_LEN;
    let ciphertext = &buf[OUTER_NONCE_LEN..ct_end];
    let tag = &buf[ct_end..];

    let expected = outer_mac(&mac_key, &nonce, ciphertext);
    if !ct_eq(&expected, tag) {
        return None;
    }
    let mut plaintext = ciphertext.to_vec();
    apply_keystream(&enc_key, &nonce, &mut plaintext);
    Some(plaintext)
}

impl OuterSessionState {
    /// Seal with the current phase-1 gate key without changing the phase
    /// transition state. Datagram transports whose wire scheduler is outside
    /// the peer packet sink (notably QUIC) use this during their bounded
    /// phase-2 transition window.
    pub fn seal_gate_datagram(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
        if !self.enabled {
            return None;
        }
        let key = derive_gate_key(
            &self.network_secret,
            window_for(now_secs(), self.window_secs),
        );
        Some(seal(&key, plaintext))
    }

    /// Seal an outbound datagram with the phase-2 outer key if available, else
    /// the current phase-1 gate key (used while the Noise handshake is still in
    /// flight). Returns `None` when stealth is disabled.
    pub fn seal_datagram(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
        if !self.enabled {
            return None;
        }
        let mut phase = self.key_phase.write().unwrap();
        let key = match *phase {
            OuterKeyPhase::Gate => derive_gate_key(
                &self.network_secret,
                window_for(now_secs(), self.window_secs),
            ),
            OuterKeyPhase::PromoteAfterNextSeal(outer_key) => {
                *phase = OuterKeyPhase::Outer(outer_key, Instant::now());
                derive_gate_key(
                    &self.network_secret,
                    window_for(now_secs(), self.window_secs),
                )
            }
            OuterKeyPhase::Outer(key, _) => key,
        };
        drop(phase);
        Some(seal(&key, plaintext))
    }

    /// Open an inbound datagram. Tries the phase-2 outer key first, then the
    /// current and previous gate-key windows (handshake-phase only).
    pub fn open_datagram(&self, buf: &[u8]) -> Option<Vec<u8>> {
        if !self.enabled {
            return None;
        }
        match *self.key_phase.read().unwrap() {
            OuterKeyPhase::Outer(key, _) => return open(&key, buf),
            OuterKeyPhase::Gate | OuterKeyPhase::PromoteAfterNextSeal(_) => {}
        }
        let cur = window_for(now_secs(), self.window_secs);
        for window in [cur, cur.wrapping_sub(1)] {
            let key = derive_gate_key(&self.network_secret, window);
            if let Some(pt) = open(&key, buf) {
                return Some(pt);
            }
        }
        None
    }

    /// Narrow listener-side fallback for a new transport SYN that arrives from
    /// an address with an existing phase-2 session. Callers must validate that
    /// the opened packet is a SYN; this helper must never reopen gate-key data.
    pub fn open_gate_datagram(&self, buf: &[u8]) -> Option<Vec<u8>> {
        if !self.enabled {
            return None;
        }
        let cur = window_for(now_secs(), self.window_secs);
        for window in [cur, cur.wrapping_sub(1)] {
            let key = derive_gate_key(&self.network_secret, window);
            if let Some(plaintext) = open(&key, buf) {
                return Some(plaintext);
            }
        }
        None
    }
}

#[cfg(test)]
mod aead_tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip() {
        let key = [9u8; 32];
        let msg = b"the quick brown fox";
        let sealed = seal(&key, msg);
        assert_eq!(sealed.len(), msg.len() + OUTER_OVERHEAD);
        assert_ne!(
            &sealed[OUTER_NONCE_LEN..OUTER_NONCE_LEN + msg.len()],
            &msg[..]
        );
        assert_eq!(open(&key, &sealed).unwrap(), msg);
    }

    #[test]
    fn open_rejects_tamper_and_wrong_key() {
        let key = [9u8; 32];
        let msg = b"payload";
        let mut sealed = seal(&key, msg);
        assert!(open(&[8u8; 32], &sealed).is_none());
        sealed[OUTER_NONCE_LEN] ^= 0xff;
        assert!(open(&key, &sealed).is_none());
    }

    #[test]
    fn empty_plaintext_roundtrips() {
        let key = [3u8; 32];
        let sealed = seal(&key, b"");
        assert_eq!(sealed.len(), OUTER_OVERHEAD);
        assert_eq!(open(&key, &sealed).unwrap(), b"");
    }

    #[test]
    fn datagram_uses_gate_key_then_outer_key() {
        let state = OuterSessionState::new(b"secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        let sealed = state.seal_datagram(b"hello").unwrap();
        assert_eq!(state.open_datagram(&sealed).unwrap(), b"hello");

        state.set_outer_key_from_handshake_hash(b"hh");
        let sealed2 = state.seal_datagram(b"world").unwrap();
        assert_eq!(state.open_datagram(&sealed2).unwrap(), b"world");
    }

    #[test]
    fn datagram_rejects_gate_key_after_phase2_install() {
        let state = OuterSessionState::new(b"secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        let gate_sealed = state.seal_datagram(b"hello").unwrap();

        state.set_outer_key_from_handshake_hash(b"hh");
        assert!(state.open_datagram(&gate_sealed).is_none());

        let outer_sealed = state.seal_datagram(b"world").unwrap();
        assert_eq!(state.open_datagram(&outer_sealed).unwrap(), b"world");
    }

    #[test]
    fn explicit_gate_open_remains_available_after_phase2_install() {
        let state = OuterSessionState::new(b"secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        let gate_sealed = state.seal_datagram(b"syn").unwrap();
        state.set_outer_key_from_handshake_hash(b"hh");

        assert!(state.open_datagram(&gate_sealed).is_none());
        assert_eq!(state.open_gate_datagram(&gate_sealed).unwrap(), b"syn");
    }

    #[test]
    fn pending_outer_key_keeps_one_datagram_on_gate_key() {
        let state = OuterSessionState::new(b"secret".to_vec(), DEFAULT_GATE_WINDOW_SECS);
        let handshake_hash = b"noise-msg3-handshake-hash";
        state.promote_outer_key_after_next_seal(handshake_hash);

        let msg3 = state.seal_datagram(b"msg3").unwrap();
        let gate_key = derive_gate_key(b"secret", window_for(now_secs(), DEFAULT_GATE_WINDOW_SECS));
        assert_eq!(open(&gate_key, &msg3).unwrap(), b"msg3");

        let outer_key = derive_outer_key(handshake_hash);
        assert_eq!(state.outer_key(), Some(outer_key));
        let data = state.seal_datagram(b"data").unwrap();
        assert_eq!(open(&outer_key, &data).unwrap(), b"data");
        assert!(open(&gate_key, &data).is_none());
    }
}

/// Whether stealth is actually usable for this config and runtime mode.
pub fn is_stealth_effectively_enabled(
    network_secret: Option<&str>,
    stealth_mode: bool,
    secure_mode: bool,
) -> bool {
    stealth_mode && secure_mode && network_secret.is_some_and(|secret| !secret.trim().is_empty())
}

/// Build a shared [`OuterSessionState`] from config. Returns a disabled state
/// (no-op) unless stealth is effectively enabled for this config.
pub fn build_outer_session(
    network_secret: Option<&str>,
    stealth_mode: bool,
    secure_mode: bool,
    window_secs: u32,
) -> Arc<OuterSessionState> {
    match (
        is_stealth_effectively_enabled(network_secret, stealth_mode, secure_mode),
        network_secret,
    ) {
        (true, Some(secret)) => {
            let window = if window_secs == 0 {
                DEFAULT_GATE_WINDOW_SECS
            } else {
                window_secs as u64
            };
            Arc::new(OuterSessionState::new(secret.as_bytes().to_vec(), window))
        }
        _ => Arc::new(OuterSessionState::disabled()),
    }
}
