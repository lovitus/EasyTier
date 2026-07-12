use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::PolicyRevision;

const MAX_RESTARTS: u8 = 3;

pub trait PolicyRuntime: Send + Sync {
    fn revision_id(&self) -> &str;
    fn shutdown(self: Arc<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub trait PolicyRuntimeFactory: Send + Sync {
    fn build(
        &self,
        revision: Arc<PolicyRevision>,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<dyn PolicyRuntime>, String>> + Send>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyStatus {
    Disabled,
    Applying { revision: String },
    Ready { revision: String },
    Outage { generation: u64 },
    Dormant { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthEvent {
    NetworkChanged,
    NetworkAvailable,
    RuntimeFailed,
    ManualRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    Wait(Duration),
    Probe,
    Dormant,
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    generation: u64,
    attempts: u8,
    started_at: Instant,
    next_allowed: Instant,
    probe_in_flight: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            generation: 0,
            attempts: 0,
            started_at: now,
            next_allowed: now,
            probe_in_flight: false,
        }
    }
}

impl RetryPolicy {
    pub fn network_changed(&mut self, now: Instant) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.attempts = 0;
        self.started_at = now;
        self.next_allowed = now + Duration::from_secs(3);
        self.probe_in_flight = false;
        self.generation
    }

    pub fn decide(&mut self, now: Instant) -> RetryDecision {
        if self.attempts >= 12 || now.duration_since(self.started_at) >= Duration::from_secs(600) {
            return RetryDecision::Dormant;
        }
        if self.probe_in_flight || now < self.next_allowed {
            return RetryDecision::Wait(self.next_allowed.saturating_duration_since(now));
        }
        self.probe_in_flight = true;
        RetryDecision::Probe
    }

    pub fn finish_probe(&mut self, now: Instant, success: bool) {
        self.probe_in_flight = false;
        if success {
            self.attempts = 0;
            self.started_at = now;
            self.next_allowed = now;
            return;
        }
        const BACKOFF: [u64; 6] = [1, 2, 5, 10, 30, 60];
        let index = usize::from(self.attempts).min(BACKOFF.len() - 1);
        self.attempts = self.attempts.saturating_add(1);
        self.next_allowed = now + Duration::from_secs(BACKOFF[index]);
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("policy runtime rejected revision {revision}: {reason}")]
    Runtime { revision: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyResult {
    Unchanged,
    Applied { revision: String },
}

struct SupervisorState {
    status: PolicyStatus,
    previous: Option<Arc<PolicyRevision>>,
    current: Option<Arc<PolicyRevision>>,
    retry: RetryPolicy,
    restart_count: u8,
}

pub struct PolicySupervisor<F> {
    factory: F,
    runtime: Mutex<Option<Arc<dyn PolicyRuntime>>>,
    state: Mutex<SupervisorState>,
}

impl<F: PolicyRuntimeFactory> PolicySupervisor<F> {
    pub fn new(factory: F) -> Self {
        Self {
            factory,
            runtime: Mutex::new(None),
            state: Mutex::new(SupervisorState {
                status: PolicyStatus::Disabled,
                previous: None,
                current: None,
                retry: RetryPolicy::default(),
                restart_count: 0,
            }),
        }
    }

    pub fn status(&self) -> PolicyStatus {
        self.state.lock().unwrap().status.clone()
    }

    pub async fn apply(&self, revision: Arc<PolicyRevision>) -> Result<ApplyResult, ApplyError> {
        {
            let mut state = self.state.lock().unwrap();
            if state
                .current
                .as_ref()
                .is_some_and(|current| current.digest == revision.digest)
            {
                return Ok(ApplyResult::Unchanged);
            }
            state.status = PolicyStatus::Applying {
                revision: revision.id.clone(),
            };
        }

        let candidate = match self.factory.build(revision.clone()).await {
            Ok(candidate) => candidate,
            Err(reason) => {
                let mut state = self.state.lock().unwrap();
                state.status = match &state.current {
                    Some(current) => PolicyStatus::Ready {
                        revision: current.id.clone(),
                    },
                    None => PolicyStatus::Dormant {
                        reason: reason.clone(),
                    },
                };
                return Err(ApplyError::Runtime {
                    revision: revision.id.clone(),
                    reason,
                });
            }
        };

        let old_runtime = self.runtime.lock().unwrap().replace(candidate);
        {
            let mut state = self.state.lock().unwrap();
            state.previous = state.current.replace(revision.clone());
            state.restart_count = 0;
            state.status = PolicyStatus::Ready {
                revision: revision.id.clone(),
            };
        }
        if let Some(old_runtime) = old_runtime {
            old_runtime.shutdown().await;
        }
        Ok(ApplyResult::Applied {
            revision: revision.id.clone(),
        })
    }

    pub fn on_health_event(&self, event: HealthEvent, now: Instant) -> RetryDecision {
        let mut state = self.state.lock().unwrap();
        match event {
            HealthEvent::NetworkChanged => {
                let generation = state.retry.network_changed(now);
                state.status = PolicyStatus::Outage { generation };
                RetryDecision::Wait(Duration::from_secs(3))
            }
            HealthEvent::NetworkAvailable | HealthEvent::ManualRetry => state.retry.decide(now),
            HealthEvent::RuntimeFailed => {
                state.restart_count = state.restart_count.saturating_add(1);
                if state.restart_count > MAX_RESTARTS {
                    state.status = PolicyStatus::Dormant {
                        reason: "policy runtime restart budget exhausted".to_owned(),
                    };
                    RetryDecision::Dormant
                } else {
                    state.retry.finish_probe(now, false);
                    state.retry.decide(now)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const POLICY: &str = "version: 1\nrules: [\"MATCH,DIRECT\"]\n";

    struct Runtime(String);

    impl PolicyRuntime for Runtime {
        fn revision_id(&self) -> &str {
            &self.0
        }

        fn shutdown(self: Arc<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(async {})
        }
    }

    struct Factory(bool);

    impl PolicyRuntimeFactory for Factory {
        fn build(
            &self,
            revision: Arc<PolicyRevision>,
        ) -> Pin<Box<dyn Future<Output = Result<Arc<dyn PolicyRuntime>, String>> + Send>> {
            let succeeds = self.0;
            Box::pin(async move {
                if succeeds {
                    Ok(Arc::new(Runtime(revision.id.clone())) as Arc<dyn PolicyRuntime>)
                } else {
                    Err("not ready".to_owned())
                }
            })
        }
    }

    #[tokio::test]
    async fn applies_transactionally_and_skips_same_digest() {
        let revision = Arc::new(PolicyRevision::parse(POLICY, Path::new(".")).unwrap());
        let supervisor = PolicySupervisor::new(Factory(true));
        assert!(matches!(
            supervisor.apply(revision.clone()).await.unwrap(),
            ApplyResult::Applied { .. }
        ));
        assert_eq!(
            supervisor.apply(revision).await.unwrap(),
            ApplyResult::Unchanged
        );
    }

    #[test]
    fn network_generation_has_grace_and_bounded_backoff() {
        let mut retry = RetryPolicy::default();
        let now = Instant::now();
        assert_eq!(retry.network_changed(now), 1);
        assert_eq!(
            retry.decide(now),
            RetryDecision::Wait(Duration::from_secs(3))
        );
        assert_eq!(
            retry.decide(now + Duration::from_secs(3)),
            RetryDecision::Probe
        );
        retry.finish_probe(now + Duration::from_secs(3), false);
        assert_eq!(
            retry.decide(now + Duration::from_secs(3)),
            RetryDecision::Wait(Duration::from_secs(1))
        );
    }

    #[tokio::test]
    async fn failed_candidate_keeps_previous_runtime() {
        let revision = Arc::new(PolicyRevision::parse(POLICY, Path::new(".")).unwrap());
        let supervisor = PolicySupervisor::new(Factory(false));
        assert!(supervisor.apply(revision).await.is_err());
        assert!(matches!(supervisor.status(), PolicyStatus::Dormant { .. }));
    }
}
