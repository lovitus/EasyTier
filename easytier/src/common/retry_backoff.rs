use rand::Rng as _;

#[derive(Debug)]
pub struct BackOff {
    backoffs_ms: Vec<u64>,
    current_idx: usize,
}

impl BackOff {
    pub fn new(backoffs_ms: Vec<u64>) -> Self {
        assert!(
            !backoffs_ms.is_empty(),
            "backoff schedule must not be empty"
        );
        Self {
            backoffs_ms,
            current_idx: 0,
        }
    }

    pub fn next_backoff(&mut self) -> u64 {
        let backoff = self.backoffs_ms[self.current_idx];
        self.current_idx = (self.current_idx + 1).min(self.backoffs_ms.len() - 1);
        backoff
    }

    pub fn next_backoff_with_jitter(&mut self) -> u64 {
        Self::with_half_jitter(self.next_backoff())
    }

    pub fn with_half_jitter(backoff_ms: u64) -> u64 {
        let delta = backoff_ms >> 1;
        if delta == 0 {
            return backoff_ms;
        }
        let mut rng = rand::rngs::OsRng;
        rng.gen_range(backoff_ms - delta..backoff_ms + delta)
    }

    pub fn reset(&mut self) {
        self.current_idx = 0;
    }

    pub fn rollback(&mut self) {
        self.current_idx = self.current_idx.saturating_sub(1);
    }

    pub fn is_saturated(&self) -> bool {
        self.current_idx == self.backoffs_ms.len() - 1
    }

    pub async fn sleep_for_next_backoff(&mut self) {
        let backoff = self.next_backoff();
        if backoff > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(backoff)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BackOff;

    #[test]
    fn sequence_saturates_and_reset_restarts_it() {
        let mut backoff = BackOff::new(vec![1_000, 10_000, 30_000]);
        assert_eq!(backoff.next_backoff(), 1_000);
        assert_eq!(backoff.next_backoff(), 10_000);
        assert!(backoff.is_saturated());
        assert_eq!(backoff.next_backoff(), 30_000);
        assert_eq!(backoff.next_backoff(), 30_000);
        backoff.reset();
        assert_eq!(backoff.next_backoff(), 1_000);
    }

    #[test]
    fn shared_jitter_stays_within_direct_connector_bounds() {
        for _ in 0..128 {
            let delay = BackOff::with_half_jitter(10_000);
            assert!((5_000..15_000).contains(&delay));
        }
        assert_eq!(BackOff::with_half_jitter(1), 1);
    }
}
