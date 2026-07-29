use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    net::SocketAddr,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{common::PeerId, tunnel::IpScheme};

const P2P_ENDPOINT_RETRY_SHARDS: usize = 64;
const P2P_ENDPOINT_RETRY_SHARD_CAPACITY: usize = 1024;
const P2P_ENDPOINT_RETRY_BACKOFF_MILLIS: [u64; 5] = [60_000, 120_000, 240_000, 480_000, 600_000];

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct P2pEndpointKey {
    peer_id: PeerId,
    scheme: IpScheme,
    remote_addr: SocketAddr,
}

impl P2pEndpointKey {
    pub(crate) fn new(peer_id: PeerId, scheme: IpScheme, remote_addr: SocketAddr) -> Self {
        Self {
            peer_id,
            scheme,
            remote_addr,
        }
    }

    pub(crate) fn peer_id(self) -> PeerId {
        self.peer_id
    }

    pub(crate) fn scheme(self) -> IpScheme {
        self.scheme
    }

    pub(crate) fn remote_addr(self) -> SocketAddr {
        self.remote_addr
    }
}

#[derive(Clone, Copy, Debug)]
struct P2pEndpointRetryEntry {
    failure_stage: Option<u8>,
    blocked_until_millis: u64,
    revision: u64,
    last_touched_millis: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct P2pEndpointAttempt {
    key: P2pEndpointKey,
    observed_revision: u64,
}

impl P2pEndpointAttempt {
    pub(crate) fn key(&self) -> P2pEndpointKey {
        self.key
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct P2pEndpointCooldown {
    stage: u8,
    remaining: Duration,
}

impl P2pEndpointCooldown {
    pub(crate) fn stage(self) -> u8 {
        self.stage
    }

    pub(crate) fn remaining(self) -> Duration {
        self.remaining
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct P2pEndpointFailureUpdate {
    stage: u8,
    cooldown: Duration,
}

impl P2pEndpointFailureUpdate {
    pub(crate) fn stage(self) -> u8 {
        self.stage
    }

    pub(crate) fn cooldown(self) -> Duration {
        self.cooldown
    }
}

/// Bounded endpoint failure memory for automatic P2P work.
///
/// This deliberately remains separate from `UnderlayBreaker`: that breaker is
/// a configurable loop-safety mechanism, while this table only suppresses
/// repeated automatic P2P failures. It is consulted at logical attempt
/// boundaries, never from packet send/receive loops.
pub(crate) struct P2pEndpointRetryTable {
    shards: Vec<Mutex<HashMap<P2pEndpointKey, P2pEndpointRetryEntry>>>,
    shard_capacity: usize,
    disabled: AtomicBool,
    next_revision: AtomicU64,
    started_at: Instant,
}

impl std::fmt::Debug for P2pEndpointRetryTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P2pEndpointRetryTable")
            .field("shards", &self.shards.len())
            .field("shard_capacity", &self.shard_capacity)
            .field("disabled", &self.disabled.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Default for P2pEndpointRetryTable {
    fn default() -> Self {
        Self::new(false)
    }
}

impl P2pEndpointRetryTable {
    pub(crate) fn new(disabled: bool) -> Self {
        Self::with_limits_and_disabled(
            P2P_ENDPOINT_RETRY_SHARDS,
            P2P_ENDPOINT_RETRY_SHARD_CAPACITY,
            disabled,
        )
    }

    fn with_limits(shard_count: usize, shard_capacity: usize) -> Self {
        Self::with_limits_and_disabled(shard_count, shard_capacity, false)
    }

    fn with_limits_and_disabled(shard_count: usize, shard_capacity: usize, disabled: bool) -> Self {
        assert!(shard_count.is_power_of_two());
        assert!(shard_capacity > 0);
        Self {
            shards: (0..shard_count)
                .map(|_| Mutex::new(HashMap::new()))
                .collect(),
            shard_capacity,
            disabled: AtomicBool::new(disabled),
            next_revision: AtomicU64::new(0),
            started_at: Instant::now(),
        }
    }

    /// Toggle the compatibility escape hatch at the single shared decision
    /// point. Enabling it also removes old cooldowns so disabling it again
    /// starts from a clean table instead of resurrecting stale failures.
    pub(crate) fn set_disabled(&self, disabled: bool) {
        self.disabled.store(disabled, Ordering::Release);
        if disabled {
            for shard in &self.shards {
                shard.lock().unwrap().clear();
            }
        }
    }

    fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    fn bypassed_attempt(key: P2pEndpointKey) -> P2pEndpointAttempt {
        P2pEndpointAttempt {
            key,
            // Revisions created by next_revision are never zero.
            observed_revision: 0,
        }
    }

    fn now_millis(&self) -> u64 {
        self.started_at
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn next_revision(&self) -> u64 {
        loop {
            let revision = self
                .next_revision
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            if revision != 0 {
                return revision;
            }
        }
    }

    fn shard_index(&self, key: &P2pEndpointKey) -> usize {
        let mut hasher = std::hash::DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish() as usize & (self.shards.len() - 1)
    }

    fn evict_oldest_if_full(
        shard: &mut HashMap<P2pEndpointKey, P2pEndpointRetryEntry>,
        shard_capacity: usize,
    ) {
        if shard.len() < shard_capacity {
            return;
        }
        let Some(oldest_key) = shard
            .iter()
            .min_by_key(|(_, entry)| entry.last_touched_millis)
            .map(|(key, _)| *key)
        else {
            return;
        };
        shard.remove(&oldest_key);
    }

    pub(crate) fn begin(
        &self,
        key: P2pEndpointKey,
    ) -> Result<P2pEndpointAttempt, P2pEndpointCooldown> {
        self.begin_at(key, self.now_millis())
    }

    fn begin_at(
        &self,
        key: P2pEndpointKey,
        now_millis: u64,
    ) -> Result<P2pEndpointAttempt, P2pEndpointCooldown> {
        if self.is_disabled() {
            return Ok(Self::bypassed_attempt(key));
        }

        let shard_index = self.shard_index(&key);
        let mut shard = self.shards[shard_index].lock().unwrap();
        // Serialize against set_disabled(true) clearing this shard. Whichever
        // side wins, a disabled table cannot publish a new tracked attempt.
        if self.is_disabled() {
            return Ok(Self::bypassed_attempt(key));
        }
        if let Some(entry) = shard.get_mut(&key) {
            entry.last_touched_millis = now_millis;
            if now_millis < entry.blocked_until_millis {
                let stage = entry
                    .failure_stage
                    .expect("a blocked P2P endpoint must have a failure stage")
                    + 1;
                return Err(P2pEndpointCooldown {
                    stage,
                    remaining: Duration::from_millis(
                        entry.blocked_until_millis.saturating_sub(now_millis),
                    ),
                });
            }
            return Ok(P2pEndpointAttempt {
                key,
                observed_revision: entry.revision,
            });
        }

        Self::evict_oldest_if_full(&mut shard, self.shard_capacity);
        let revision = self.next_revision();
        shard.insert(
            key,
            P2pEndpointRetryEntry {
                failure_stage: None,
                blocked_until_millis: 0,
                revision,
                last_touched_millis: now_millis,
            },
        );
        Ok(P2pEndpointAttempt {
            key,
            observed_revision: revision,
        })
    }

    pub(crate) fn failed(&self, attempt: P2pEndpointAttempt) -> Option<P2pEndpointFailureUpdate> {
        self.failed_at(attempt, self.now_millis())
    }

    fn failed_at(
        &self,
        attempt: P2pEndpointAttempt,
        now_millis: u64,
    ) -> Option<P2pEndpointFailureUpdate> {
        if attempt.observed_revision == 0 || self.is_disabled() {
            return None;
        }

        let shard_index = self.shard_index(&attempt.key);
        let mut shard = self.shards[shard_index].lock().unwrap();
        if self.is_disabled() {
            return None;
        }
        let entry = shard.get_mut(&attempt.key)?;
        if entry.revision != attempt.observed_revision {
            return None;
        }

        let stage_index = entry
            .failure_stage
            .map_or(0, |stage| stage.saturating_add(1) as usize)
            .min(P2P_ENDPOINT_RETRY_BACKOFF_MILLIS.len() - 1);
        let cooldown_millis = P2P_ENDPOINT_RETRY_BACKOFF_MILLIS[stage_index];
        entry.failure_stage = Some(stage_index as u8);
        entry.blocked_until_millis = now_millis.saturating_add(cooldown_millis);
        entry.revision = self.next_revision();
        entry.last_touched_millis = now_millis;

        Some(P2pEndpointFailureUpdate {
            stage: stage_index as u8 + 1,
            cooldown: Duration::from_millis(cooldown_millis),
        })
    }

    pub(crate) fn succeeded(&self, attempt: P2pEndpointAttempt) {
        if attempt.observed_revision == 0 {
            return;
        }
        self.shards[self.shard_index(&attempt.key)]
            .lock()
            .unwrap()
            .remove(&attempt.key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.lock().unwrap().len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn key(peer_id: PeerId, scheme: IpScheme, octet: u8, port: u16) -> P2pEndpointKey {
        P2pEndpointKey::new(
            peer_id,
            scheme,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, octet)), port),
        )
    }

    #[test]
    fn endpoint_backoff_is_capped_at_ten_minutes() {
        let table = P2pEndpointRetryTable::with_limits(1, 8);
        let key = key(7, IpScheme::Udp, 1, 11010);
        let expected = [60_000, 120_000, 240_000, 480_000, 600_000, 600_000];
        let mut now = 1_000;

        for (index, expected_cooldown) in expected.into_iter().enumerate() {
            let attempt = table.begin_at(key, now).unwrap();
            let update = table.failed_at(attempt, now).unwrap();
            assert_eq!(update.stage(), (index + 1).min(5) as u8);
            assert_eq!(update.cooldown(), Duration::from_millis(expected_cooldown));
            assert_eq!(
                table.begin_at(key, now + expected_cooldown - 1),
                Err(P2pEndpointCooldown {
                    stage: (index + 1).min(5) as u8,
                    remaining: Duration::from_millis(1),
                })
            );
            now += expected_cooldown;
        }
    }

    #[test]
    fn peer_scheme_ip_and_port_are_independent_keys() {
        let table = P2pEndpointRetryTable::with_limits(4, 8);
        let blocked = key(7, IpScheme::Udp, 1, 11010);
        let attempt = table.begin_at(blocked, 0).unwrap();
        table.failed_at(attempt, 0).unwrap();

        assert!(table.begin_at(blocked, 1).is_err());
        assert!(table.begin_at(key(8, IpScheme::Udp, 1, 11010), 1).is_ok());
        assert!(table.begin_at(key(7, IpScheme::Tcp, 1, 11010), 1).is_ok());
        assert!(table.begin_at(key(7, IpScheme::Udp, 2, 11010), 1).is_ok());
        assert!(table.begin_at(key(7, IpScheme::Udp, 1, 11011), 1).is_ok());
    }

    #[test]
    fn success_clears_cooldown_and_old_failure_cannot_restore_it() {
        let table = P2pEndpointRetryTable::with_limits(1, 8);
        let key = key(7, IpScheme::Udp, 1, 11010);
        let first = table.begin_at(key, 0).unwrap();
        let concurrent = table.begin_at(key, 0).unwrap();
        let stale = table.begin_at(key, 0).unwrap();

        table.failed_at(first, 0).unwrap();
        table.succeeded(concurrent);
        assert!(table.begin_at(key, 1).is_ok());

        assert_eq!(table.failed_at(stale, 2), None);
        assert!(table.begin_at(key, 2).is_ok());
    }

    #[test]
    fn concurrent_failures_with_one_revision_only_advance_once() {
        let table = P2pEndpointRetryTable::with_limits(1, 8);
        let key = key(7, IpScheme::Udp, 1, 11010);
        let first = table.begin_at(key, 0).unwrap();
        let second = table.begin_at(key, 0).unwrap();

        assert_eq!(table.failed_at(first, 0).unwrap().stage(), 1);
        assert_eq!(table.failed_at(second, 0), None);

        let next = table.begin_at(key, 60_000).unwrap();
        assert_eq!(table.failed_at(next, 60_000).unwrap().stage(), 2);
    }

    #[test]
    fn protocol_subattempts_share_one_revision_and_settle_once() {
        let table = P2pEndpointRetryTable::with_limits(1, 8);
        let key = key(7, IpScheme::Udp, 1, 11010);

        // Models a cone pre-attempt followed by the symmetric burst for the
        // same logical outer round. The pre-attempt does not settle; the final
        // burst advances exactly one stage.
        let cone_subattempt = table.begin_at(key, 0).unwrap();
        let symmetric_subattempt = table.begin_at(key, 1).unwrap();
        assert_eq!(
            cone_subattempt.observed_revision,
            symmetric_subattempt.observed_revision
        );
        assert_eq!(table.failed_at(symmetric_subattempt, 2).unwrap().stage(), 1);
        assert_eq!(table.failed_at(cone_subattempt, 3), None);
    }

    #[test]
    fn abandoned_attempt_does_not_create_a_cooldown() {
        let table = P2pEndpointRetryTable::with_limits(1, 8);
        let key = key(7, IpScheme::Udp, 1, 11010);

        let _cancelled_or_no_work = table.begin_at(key, 0).unwrap();
        assert!(table.begin_at(key, 1).is_ok());
        assert!(table.begin_at(key, 60_000).is_ok());
    }

    #[test]
    fn table_capacity_is_hard_and_oldest_entry_is_evicted() {
        let table = P2pEndpointRetryTable::with_limits(1, 2);
        let first_key = key(1, IpScheme::Udp, 1, 11010);
        let second_key = key(2, IpScheme::Udp, 2, 11010);
        let third_key = key(3, IpScheme::Udp, 3, 11010);

        table.begin_at(first_key, 1).unwrap();
        table.begin_at(second_key, 2).unwrap();
        table.begin_at(third_key, 3).unwrap();

        assert_eq!(table.len(), 2);
        assert!(!table.shards[0].lock().unwrap().contains_key(&first_key));
        assert!(table.shards[0].lock().unwrap().contains_key(&second_key));
        assert!(table.shards[0].lock().unwrap().contains_key(&third_key));
    }

    #[test]
    fn disabled_table_is_stateless_and_reenable_starts_clean() {
        let table = P2pEndpointRetryTable::with_limits(1, 8);
        let key = key(7, IpScheme::Udp, 1, 11010);

        let attempt = table.begin_at(key, 0).unwrap();
        table.failed_at(attempt, 0).unwrap();
        assert!(table.begin_at(key, 1).is_err());

        table.set_disabled(true);
        assert_eq!(table.len(), 0);
        let bypassed = table.begin_at(key, 1).unwrap();
        assert_eq!(table.failed_at(bypassed, 1), None);
        assert!(table.begin_at(key, 1).is_ok());
        assert_eq!(table.len(), 0);

        table.set_disabled(false);
        let fresh = table.begin_at(key, 1).unwrap();
        assert_eq!(table.failed_at(fresh, 1).unwrap().stage(), 1);
        assert!(table.begin_at(key, 2).is_err());
    }
}
