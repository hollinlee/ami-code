use std::collections::VecDeque;
use std::time::{Duration, Instant};

const STABLE_SESSION: Duration = Duration::from_secs(10);
const FAILURE_WINDOW: Duration = Duration::from_secs(30);
const MAX_QUICK_FAILURES: usize = 6;
const RESTART_DELAYS_MS: [u64; 6] = [250, 500, 1_000, 2_000, 4_000, 8_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct SessionId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SessionIdentity {
    pub id: SessionId,
    pub generation: u64,
}

impl SessionIdentity {
    pub fn matches(self, event: Self) -> bool {
        self == event
    }
}

#[derive(Default)]
pub(super) struct SessionIds {
    next: u64,
    shutting_down: bool,
}

impl SessionIds {
    pub fn new() -> Self {
        Self {
            next: 1,
            shutting_down: false,
        }
    }

    pub fn allocate(&mut self, generation: u64) -> Option<SessionIdentity> {
        if self.shutting_down {
            return None;
        }
        let id = SessionId(self.next);
        self.next = self.next.checked_add(1).expect("session id exhausted");
        Some(SessionIdentity { id, generation })
    }

    pub fn shutdown(&mut self) {
        self.shutting_down = true;
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }
}

#[derive(Default)]
pub(super) struct RestartPolicy {
    consecutive: usize,
    quick_failures: VecDeque<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestartDecision {
    Backoff(Duration),
    Paused,
}

impl RestartPolicy {
    pub fn failed(&mut self, now: Instant, runtime: Duration) -> RestartDecision {
        if runtime >= STABLE_SESSION {
            self.reset();
        }
        while self
            .quick_failures
            .front()
            .is_some_and(|failure| now.saturating_duration_since(*failure) > FAILURE_WINDOW)
        {
            self.quick_failures.pop_front();
        }
        self.quick_failures.push_back(now);
        self.consecutive = self.consecutive.saturating_add(1);

        if self.quick_failures.len() >= MAX_QUICK_FAILURES {
            RestartDecision::Paused
        } else {
            let index = self
                .consecutive
                .saturating_sub(1)
                .min(RESTART_DELAYS_MS.len() - 1);
            RestartDecision::Backoff(Duration::from_millis(RESTART_DELAYS_MS[index]))
        }
    }

    pub fn reset(&mut self) {
        self.consecutive = 0;
        self.quick_failures.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delay(decision: RestartDecision) -> Duration {
        match decision {
            RestartDecision::Backoff(delay) => delay,
            RestartDecision::Paused => panic!("unexpected pause"),
        }
    }

    #[test]
    fn delay_sequence_reaches_and_stays_at_cap() {
        let start = Instant::now();
        let mut policy = RestartPolicy::default();
        let actual: Vec<_> = (0..8)
            .map(|failure| {
                // Keep failures outside the rolling pause window while retaining
                // the consecutive backoff count.
                delay(policy.failed(start + Duration::from_secs(failure * 31), Duration::ZERO))
            })
            .collect();
        assert_eq!(
            actual,
            [250, 500, 1_000, 2_000, 4_000, 8_000, 8_000, 8_000].map(Duration::from_millis)
        );
    }

    #[test]
    fn rolling_window_discards_old_quick_failures() {
        let start = Instant::now();
        let mut policy = RestartPolicy::default();
        for second in 0..5 {
            assert!(matches!(
                policy.failed(start + Duration::from_secs(second), Duration::ZERO),
                RestartDecision::Backoff(_)
            ));
        }
        assert_eq!(
            policy.failed(start + Duration::from_secs(35), Duration::ZERO),
            RestartDecision::Backoff(Duration::from_secs(8))
        );
    }

    #[test]
    fn stable_runtime_resets_failure_history_and_delay() {
        let start = Instant::now();
        let mut policy = RestartPolicy::default();
        assert_eq!(
            delay(policy.failed(start, Duration::ZERO)),
            Duration::from_millis(250)
        );
        assert_eq!(
            delay(policy.failed(start + Duration::from_secs(1), Duration::ZERO)),
            Duration::from_millis(500)
        );
        assert_eq!(
            delay(policy.failed(start + Duration::from_secs(2), STABLE_SESSION)),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn sixth_quick_failure_pauses() {
        let start = Instant::now();
        let mut policy = RestartPolicy::default();
        for second in 0..5 {
            assert!(matches!(
                policy.failed(start + Duration::from_secs(second), Duration::ZERO),
                RestartDecision::Backoff(_)
            ));
        }
        assert_eq!(
            policy.failed(start + Duration::from_secs(5), Duration::ZERO),
            RestartDecision::Paused
        );
    }

    #[test]
    fn manual_reset_restores_initial_delay_and_clears_pause() {
        let start = Instant::now();
        let mut policy = RestartPolicy::default();
        for second in 0..6 {
            let _ = policy.failed(start + Duration::from_secs(second), Duration::ZERO);
        }
        policy.reset();
        assert_eq!(
            policy.failed(start + Duration::from_secs(6), Duration::ZERO),
            RestartDecision::Backoff(Duration::from_millis(250))
        );
    }

    #[test]
    fn ids_are_monotonic_and_identity_includes_generation() {
        let mut ids = SessionIds::new();
        let first = ids.allocate(4).unwrap();
        let second = ids.allocate(5).unwrap();
        assert!(first.id < second.id);
        assert_eq!(first.generation, 4);
        assert_eq!(second.generation, 5);
    }

    #[test]
    fn stale_id_or_generation_does_not_match_current_identity() {
        let mut ids = SessionIds::new();
        let old = ids.allocate(1).unwrap();
        let current = ids.allocate(2).unwrap();
        assert!(!current.matches(old));
        assert!(!current.matches(SessionIdentity {
            id: current.id,
            generation: 1,
        }));
        assert!(current.matches(current));
    }

    #[test]
    fn shutdown_suppresses_future_allocations() {
        let mut ids = SessionIds::new();
        assert!(ids.allocate(1).is_some());
        ids.shutdown();
        assert!(ids.is_shutting_down());
        assert!(ids.allocate(2).is_none());
    }
}
