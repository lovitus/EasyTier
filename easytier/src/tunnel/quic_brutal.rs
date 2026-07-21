use std::{
    any::Any,
    sync::Arc,
    time::{Duration, Instant},
};

use quinn_proto::{
    RttEstimator,
    congestion::{Controller, ControllerFactory},
};

const INITIAL_RTT: Duration = Duration::from_millis(333);
const SAMPLE_WINDOW_SECS: u64 = 5;
const SAMPLE_SLOT_COUNT: usize = SAMPLE_WINDOW_SECS as usize;
const MIN_SAMPLE_BYTES: u64 = 64 * 1024;
const MIN_ACK_RATE_NUMERATOR: u64 = 4;
const MIN_ACK_RATE_DENOMINATOR: u64 = 5;

#[derive(Clone, Debug)]
pub struct BrutalConfig {
    bytes_per_second: u64,
}

impl BrutalConfig {
    pub fn new(bits_per_second: u64) -> Option<Self> {
        let bytes_per_second = bits_per_second.checked_div(8)?;
        (bytes_per_second != 0).then_some(Self { bytes_per_second })
    }

    pub fn bytes_per_second(&self) -> u64 {
        self.bytes_per_second
    }
}

impl ControllerFactory for BrutalConfig {
    fn build(self: Arc<Self>, now: Instant, current_mtu: u16) -> Box<dyn Controller> {
        Box::new(BrutalController::new(self, now, current_mtu))
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct DeliverySample {
    second: u64,
    acked_bytes: u64,
    lost_bytes: u64,
}

#[derive(Clone, Debug)]
struct BrutalController {
    config: Arc<BrutalConfig>,
    started_at: Instant,
    current_mtu: u16,
    latest_rtt: Duration,
    window: u64,
    samples: [DeliverySample; SAMPLE_SLOT_COUNT],
}

impl BrutalController {
    fn new(config: Arc<BrutalConfig>, now: Instant, current_mtu: u16) -> Self {
        let window = congestion_window(
            config.bytes_per_second,
            INITIAL_RTT,
            MIN_ACK_RATE_DENOMINATOR,
            MIN_ACK_RATE_DENOMINATOR,
            current_mtu,
        );
        Self {
            config,
            started_at: now,
            current_mtu,
            latest_rtt: INITIAL_RTT,
            window,
            samples: [DeliverySample::default(); SAMPLE_SLOT_COUNT],
        }
    }

    fn elapsed_second(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.started_at).as_secs()
    }

    fn record_delivery(&mut self, now: Instant, acked_bytes: u64, lost_bytes: u64) {
        let second = self.elapsed_second(now);
        let slot = &mut self.samples[second as usize % SAMPLE_SLOT_COUNT];
        if slot.second != second {
            *slot = DeliverySample {
                second,
                ..DeliverySample::default()
            };
        }
        slot.acked_bytes = slot.acked_bytes.saturating_add(acked_bytes);
        slot.lost_bytes = slot.lost_bytes.saturating_add(lost_bytes);
    }

    fn ack_rate(&self, now: Instant) -> (u64, u64) {
        let second = self.elapsed_second(now);
        let oldest = second.saturating_sub(SAMPLE_WINDOW_SECS - 1);
        let (acked, lost) = self
            .samples
            .iter()
            .filter(|sample| sample.second >= oldest && sample.second <= second)
            .fold((0u64, 0u64), |(acked, lost), sample| {
                (
                    acked.saturating_add(sample.acked_bytes),
                    lost.saturating_add(sample.lost_bytes),
                )
            });
        let total = acked.saturating_add(lost);
        if total < MIN_SAMPLE_BYTES {
            return (1, 1);
        }
        if acked.saturating_mul(MIN_ACK_RATE_DENOMINATOR)
            < total.saturating_mul(MIN_ACK_RATE_NUMERATOR)
        {
            return (MIN_ACK_RATE_NUMERATOR, MIN_ACK_RATE_DENOMINATOR);
        }
        (acked, total)
    }

    fn update_window(&mut self, now: Instant, rtt: Duration) {
        self.latest_rtt = rtt;
        let (ack_numerator, ack_denominator) = self.ack_rate(now);
        self.window = congestion_window(
            self.config.bytes_per_second,
            rtt,
            ack_numerator,
            ack_denominator,
            self.current_mtu,
        );
    }
}

impl Controller for BrutalController {
    fn on_ack(
        &mut self,
        now: Instant,
        _sent: Instant,
        bytes: u64,
        _app_limited: bool,
        rtt: &RttEstimator,
    ) {
        self.record_delivery(now, bytes, 0);
        self.update_window(now, rtt.conservative());
    }

    fn on_congestion_event(
        &mut self,
        now: Instant,
        _sent: Instant,
        _is_persistent_congestion: bool,
        lost_bytes: u64,
    ) {
        self.record_delivery(now, 0, lost_bytes);
        self.update_window(now, self.latest_rtt);
    }

    fn on_mtu_update(&mut self, new_mtu: u16) {
        self.current_mtu = new_mtu;
        self.window = self.window.max(u64::from(new_mtu).saturating_mul(10));
    }

    fn window(&self) -> u64 {
        self.window
    }

    fn clone_box(&self) -> Box<dyn Controller> {
        Box::new(self.clone())
    }

    fn initial_window(&self) -> u64 {
        self.window
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

fn congestion_window(
    bytes_per_second: u64,
    rtt: Duration,
    ack_numerator: u64,
    ack_denominator: u64,
    mtu: u16,
) -> u64 {
    // Quinn 0.11 exposes only a congestion window, while its internal pacer refills at
    // 1.25 * window / RTT. One loss-compensated BDP keeps the declared rate bounded by
    // the in-flight limit; H0 measures whether this approximation is useful in practice.
    let rtt_nanos = rtt.as_nanos().max(1);
    let ack_numerator = u128::from(ack_numerator.max(1));
    let value = u128::from(bytes_per_second)
        .saturating_mul(rtt_nanos)
        .saturating_mul(u128::from(ack_denominator))
        / 1_000_000_000u128
        / ack_numerator;
    let minimum = u64::from(mtu).saturating_mul(10);
    u64::try_from(value)
        .unwrap_or(u64::MAX)
        .clamp(minimum, u64::from(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brutal_config_converts_bits_to_bytes_without_overflow() {
        assert!(BrutalConfig::new(0).is_none());
        assert_eq!(
            BrutalConfig::new(1_000_000).unwrap().bytes_per_second(),
            125_000
        );
        assert_eq!(
            BrutalConfig::new(100_000_000_000)
                .unwrap()
                .bytes_per_second(),
            12_500_000_000
        );
    }

    #[test]
    fn brutal_window_tracks_bdp_and_ack_rate() {
        let bps = 125_000_000;
        let mtu = 1200;
        assert_eq!(
            congestion_window(bps, Duration::from_millis(100), 1, 1, mtu),
            12_500_000
        );
        assert_eq!(
            congestion_window(bps, Duration::from_millis(100), 4, 5, mtu),
            15_625_000
        );
    }

    #[test]
    fn brutal_window_is_bounded_for_extreme_inputs() {
        assert_eq!(
            congestion_window(1, Duration::from_nanos(1), 1, 1, 1500),
            15_000
        );
        assert_eq!(
            congestion_window(u64::MAX, Duration::MAX, 1, u64::MAX, 1200),
            u64::from(u32::MAX)
        );
    }

    #[test]
    fn brutal_mtu_update_preserves_ten_packet_minimum() {
        let now = Instant::now();
        let config = Arc::new(BrutalConfig::new(8).unwrap());
        let mut controller = BrutalController::new(config, now, 1200);
        assert_eq!(controller.window(), 12_000);

        controller.on_mtu_update(1500);
        assert_eq!(controller.window(), 15_000);
    }

    #[test]
    fn brutal_ack_rate_uses_five_second_window_and_floor() {
        let now = Instant::now();
        let config = Arc::new(BrutalConfig::new(1_000_000_000).unwrap());
        let mut controller = BrutalController::new(config, now, 1200);

        controller.record_delivery(now, MIN_SAMPLE_BYTES - 1, 0);
        assert_eq!(controller.ack_rate(now), (1, 1));

        controller.record_delivery(now, 0, MIN_SAMPLE_BYTES);
        assert_eq!(
            controller.ack_rate(now),
            (MIN_ACK_RATE_NUMERATOR, MIN_ACK_RATE_DENOMINATOR)
        );

        let later = now + Duration::from_secs(SAMPLE_WINDOW_SECS + 1);
        controller.record_delivery(later, MIN_SAMPLE_BYTES, 0);
        assert_eq!(
            controller.ack_rate(later),
            (MIN_SAMPLE_BYTES, MIN_SAMPLE_BYTES)
        );
    }
}
