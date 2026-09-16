//! Process-wide, per-endpoint timing aggregates: at most one info record per
//! five seconds, emitted by the next request (no idle timer/task). Every request
//! contributes maxima, including unsampled slow requests. Concurrent updates
//! can straddle reporting windows; the fields are not a per-request tuple.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

const LOG_INTERVAL_MS: u64 = 5_000;
pub(super) static SNAPSHOT: ReadTiming = ReadTiming::new();
pub(super) static SSE_FIRST: ReadTiming = ReadTiming::new();

pub(super) struct Summary {
    pub requests: u64,
    pub serve_ms: f64,
    pub buffer_age_ms: f64,
}

pub(super) struct ReadTiming {
    started: OnceLock<Instant>,
    next_ms: AtomicU64,
    requests: AtomicU64,
    serve_us: AtomicU64,
    age_us: AtomicU64,
}

impl ReadTiming {
    const fn new() -> Self {
        Self {
            started: OnceLock::new(),
            next_ms: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            serve_us: AtomicU64::new(0),
            age_us: AtomicU64::new(0),
        }
    }

    pub(super) fn record(&self, serve: Duration, age: Duration) -> Option<Summary> {
        let now = self.started.get_or_init(Instant::now).elapsed();
        self.record_at(now.as_millis() as u64, serve, age)
    }

    fn record_at(&self, now_ms: u64, serve: Duration, age: Duration) -> Option<Summary> {
        self.requests.fetch_add(1, Relaxed);
        self.serve_us.fetch_max(serve.as_micros() as u64, Relaxed);
        self.age_us.fetch_max(age.as_micros() as u64, Relaxed);
        let next = self.next_ms.load(Relaxed);
        if now_ms < next
            || self
                .next_ms
                .compare_exchange(
                    next,
                    now_ms.saturating_add(LOG_INTERVAL_MS),
                    Relaxed,
                    Relaxed,
                )
                .is_err()
        {
            return None;
        }
        Some(Summary {
            requests: self.requests.swap(0, Relaxed),
            serve_ms: self.serve_us.swap(0, Relaxed) as f64 / 1000.0,
            buffer_age_ms: self.age_us.swap(0, Relaxed) as f64 / 1000.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn tight_polling_is_bounded_and_keeps_unsampled_peaks() {
        let timing = ReadTiming::new();
        let ms = Duration::from_millis;
        assert_eq!(timing.record_at(0, ms(1), ms(2)).unwrap().requests, 1);
        for now in 1..5_000 {
            assert!(timing.record_at(now, ms(37), ms(2011)).is_none());
        }
        let next = timing.record_at(5_000, ms(1), ms(2)).unwrap();
        assert_eq!(next.requests, 5_000);
        assert_eq!(next.serve_ms, 37.0);
        assert_eq!(next.buffer_age_ms, 2011.0);
        // A late caller carrying an older clock reading cannot reopen a window.
        assert!(timing.record_at(4_999, ms(1), ms(2)).is_none());
        assert!(timing.record_at(9_999, ms(1), ms(2)).is_none());
        let next = timing.record_at(10_000, ms(1), ms(2)).unwrap();
        assert_eq!(next.requests, 3);
        assert_eq!(next.serve_ms, 1.0);
        assert_eq!(next.buffer_age_ms, 2.0);
    }

    #[test]
    fn concurrent_callers_cannot_multiply_the_log_budget() {
        let timing = Arc::new(ReadTiming::new());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let timing = timing.clone();
                std::thread::spawn(move || {
                    (0..1_000)
                        .filter(|_| {
                            timing
                                .record_at(0, Duration::ZERO, Duration::ZERO)
                                .is_some()
                        })
                        .count()
                })
            })
            .collect();
        let emitted: usize = threads.into_iter().map(|t| t.join().unwrap()).sum();
        assert_eq!(emitted, 1);
        assert!(
            timing
                .record_at(5_000, Duration::ZERO, Duration::ZERO)
                .is_some()
        );
        println!(
            "G555_INFO_BOUND 8000 concurrent observations: 1 emission; next window emits again"
        );
    }
}
