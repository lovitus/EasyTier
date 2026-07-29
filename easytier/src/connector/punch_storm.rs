use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use hotpath::instant::Instant;

pub(crate) const FAILURE_LIMIT: u8 = 10;
pub(crate) const FAILURE_WINDOW: Duration = Duration::from_secs(10);
pub(crate) const SILENCE_DURATION: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub(crate) struct PunchBurstTrace {
    target: Option<SocketAddr>,
    failed_attempts: u8,
    suppressed: bool,
}

impl PunchBurstTrace {
    pub(crate) fn observe_target(&mut self, target: SocketAddr) {
        if self.target.is_some_and(|current| current != target) {
            self.failed_attempts = 0;
        }
        self.target = Some(target);
        self.suppressed = false;
    }

    pub(crate) fn add_attempts(&mut self, attempts: usize) {
        let attempts = attempts.min(usize::from(FAILURE_LIMIT) + 1) as u8;
        self.failed_attempts = self
            .failed_attempts
            .saturating_add(attempts)
            .min(FAILURE_LIMIT + 1);
    }

    pub(crate) fn failed_attempts(&self) -> u8 {
        self.failed_attempts
    }

    pub(crate) fn target(&self) -> Option<SocketAddr> {
        self.target
    }

    pub(crate) fn begin_target(
        &mut self,
        guard: &mut PunchStormGuard,
        now: Instant,
        has_live_peer: bool,
        target: SocketAddr,
    ) -> bool {
        self.observe_target(target);
        self.suppressed = guard.is_target_silenced(now, has_live_peer, target);
        !self.suppressed
    }

    pub(crate) fn is_suppressed(&self) -> bool {
        self.suppressed
    }
}

#[derive(Debug)]
pub(crate) struct PunchStormGuard {
    window_started_at: Instant,
    block_until: Option<Instant>,
    // One exact, compact IP:port key keeps endpoint churn bounded without a
    // hash, allocation, endpoint collection, or collision semantics.
    last_target_key: Option<PunchTargetKey>,
    failed_attempts: u8,
}

impl PunchStormGuard {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            window_started_at: now,
            block_until: None,
            last_target_key: None,
            failed_attempts: 0,
        }
    }

    fn is_target_silenced(
        &mut self,
        now: Instant,
        has_live_peer: bool,
        target: SocketAddr,
    ) -> bool {
        if !has_live_peer {
            self.reset(now);
            return false;
        }

        let target_key = PunchTargetKey::from(target);
        if self.last_target_key != Some(target_key) {
            self.reset(now);
            return false;
        }

        let Some(block_until) = self.block_until else {
            return false;
        };
        if now < block_until {
            return true;
        }

        self.reset(now);
        false
    }

    pub(crate) fn record_success(&mut self, now: Instant) {
        self.reset(now);
    }

    pub(crate) fn failed_attempts_in_window(&self) -> u8 {
        self.failed_attempts
    }

    pub(crate) fn record_failed_burst(&mut self, now: Instant, trace: &PunchBurstTrace) -> bool {
        let Some(target) = trace.target() else {
            return false;
        };
        if trace.failed_attempts() == 0 {
            return false;
        }
        let target_key = PunchTargetKey::from(target);

        if self.last_target_key != Some(target_key) {
            self.reset(now);
        } else if now.saturating_duration_since(self.window_started_at) >= FAILURE_WINDOW {
            self.reset_window(now);
        }

        self.last_target_key = Some(target_key);
        self.failed_attempts = self
            .failed_attempts
            .saturating_add(trace.failed_attempts())
            .min(FAILURE_LIMIT + 1);
        if self.failed_attempts <= FAILURE_LIMIT {
            return false;
        }

        self.block_until = Some(now + SILENCE_DURATION);
        true
    }

    fn reset_window(&mut self, now: Instant) {
        self.window_started_at = now;
        self.failed_attempts = 0;
    }

    fn reset(&mut self, now: Instant) {
        self.reset_window(now);
        self.block_until = None;
        self.last_target_key = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PunchTargetKey {
    ip: [u8; 16],
    port: u16,
    is_ipv6: bool,
}

impl From<SocketAddr> for PunchTargetKey {
    fn from(target: SocketAddr) -> Self {
        let (ip, is_ipv6) = match target.ip() {
            IpAddr::V4(ip) => {
                let mut bytes = [0; 16];
                bytes[..4].copy_from_slice(&ip.octets());
                (bytes, false)
            }
            IpAddr::V6(ip) => (ip.octets(), true),
        };
        Self {
            ip,
            port: target.port(),
            is_ipv6,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use hotpath::instant::Instant;

    use super::{PunchBurstTrace, PunchStormGuard};

    fn failed_trace(target: &str, attempts: usize) -> PunchBurstTrace {
        let mut trace = PunchBurstTrace::default();
        trace.observe_target(target.parse::<SocketAddr>().unwrap());
        trace.add_attempts(attempts);
        trace
    }

    #[test]
    fn failed_attempt_threshold_silences_only_the_next_burst() {
        let now = Instant::now();
        let mut guard = PunchStormGuard::new(now);

        assert!(!guard.record_failed_burst(now, &failed_trace("192.0.2.1:11010", 10)));
        assert!(!guard.is_target_silenced(now, true, "192.0.2.1:11010".parse().unwrap()));

        let trigger = now + Duration::from_secs(1);
        assert!(guard.record_failed_burst(trigger, &failed_trace("192.0.2.1:11010", 1)));
        assert!(guard.is_target_silenced(trigger, true, "192.0.2.1:11010".parse().unwrap()));
        assert!(!guard.is_target_silenced(
            trigger + Duration::from_secs(60),
            true,
            "192.0.2.1:11010".parse().unwrap()
        ));
    }

    #[test]
    fn failed_attempt_count_saturates_and_endpoint_churn_keeps_fixed_state() {
        let now = Instant::now();
        let mut guard = PunchStormGuard::new(now);

        assert!(guard.record_failed_burst(now, &failed_trace("192.0.2.1:11010", usize::MAX)));
        assert!(!guard.record_failed_burst(now, &failed_trace("198.51.100.2:65000", 1)));
        assert!(!guard.is_target_silenced(now, true, "198.51.100.2:65000".parse().unwrap()));
        assert!(guard.record_failed_burst(now, &failed_trace("198.51.100.2:65000", usize::MAX)));
        assert!(size_of::<PunchStormGuard>() <= 64);
    }

    #[test]
    fn expired_window_discards_old_failures() {
        let now = Instant::now();
        let mut guard = PunchStormGuard::new(now);

        assert!(!guard.record_failed_burst(now, &failed_trace("192.0.2.1:11010", 10)));
        assert!(!guard.record_failed_burst(
            now + Duration::from_secs(10),
            &failed_trace("192.0.2.1:11010", 1)
        ));
        assert!(!guard.is_target_silenced(
            now + Duration::from_secs(10),
            true,
            "192.0.2.1:11010".parse().unwrap()
        ));
    }

    #[test]
    fn a_new_target_never_inherits_the_old_targets_failures() {
        let now = Instant::now();
        let mut guard = PunchStormGuard::new(now);

        assert!(guard.record_failed_burst(now, &failed_trace("192.0.2.1:11010", 11)));
        let mut trace = PunchBurstTrace::default();
        assert!(!trace.begin_target(
            &mut guard,
            now + Duration::from_millis(500),
            true,
            "192.0.2.1:11010".parse().unwrap(),
        ));
        assert!(trace.is_suppressed());
        assert!(trace.begin_target(
            &mut guard,
            now + Duration::from_millis(500),
            true,
            "198.51.100.2:11010".parse().unwrap(),
        ));
        assert!(!trace.is_suppressed());
        assert!(!guard.record_failed_burst(
            now + Duration::from_secs(1),
            &failed_trace("198.51.100.2:11010", 1)
        ));
        assert!(!guard.is_target_silenced(
            now + Duration::from_secs(1),
            true,
            "198.51.100.2:11010".parse().unwrap()
        ));

        let mut trace = PunchBurstTrace::default();
        trace.observe_target("192.0.2.1:11010".parse().unwrap());
        trace.add_attempts(10);
        trace.observe_target("198.51.100.2:11010".parse().unwrap());
        trace.add_attempts(1);
        assert_eq!(trace.failed_attempts(), 1);
    }

    #[test]
    fn success_and_loss_of_live_peer_reset_the_guard() {
        let now = Instant::now();
        let mut guard = PunchStormGuard::new(now);
        assert!(guard.record_failed_burst(now, &failed_trace("192.0.2.1:11010", 11)));

        guard.record_success(now + Duration::from_secs(1));
        assert!(!guard.is_target_silenced(
            now + Duration::from_secs(1),
            true,
            "192.0.2.1:11010".parse().unwrap()
        ));

        assert!(guard.record_failed_burst(
            now + Duration::from_secs(2),
            &failed_trace("192.0.2.1:11010", 11)
        ));
        assert!(!guard.is_target_silenced(
            now + Duration::from_secs(2),
            false,
            "192.0.2.1:11010".parse().unwrap()
        ));
        assert!(!guard.is_target_silenced(
            now + Duration::from_secs(2),
            true,
            "192.0.2.1:11010".parse().unwrap()
        ));
    }

    #[test]
    fn empty_or_cancelled_burst_does_not_change_the_guard() {
        let now = Instant::now();
        let mut guard = PunchStormGuard::new(now);
        let trace = PunchBurstTrace::default();

        assert!(!guard.record_failed_burst(now, &trace));
        assert!(!guard.is_target_silenced(now, true, "192.0.2.1:11010".parse().unwrap()));
    }
}
