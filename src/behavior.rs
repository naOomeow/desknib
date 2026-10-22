//! Picks the pet's animation from how long the desk has been idle.
//!
//! Pure logic, independent of X11, so it is tested without a display.

use std::time::Duration;

/// Idle this long: stop breathing around, sit down.
const SIT_AFTER: Duration = Duration::from_secs(2 * 60);
/// Idle this long: lie down and sleep.
const SLEEP_AFTER: Duration = Duration::from_secs(10 * 60);
/// Below this much idle time, treat the desk as freshly used even if a
/// single idle sample came back high (a slow poll right after a click).
const AWAKE_BELOW: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Awake,
    Sitting,
    Sleeping,
}

impl State {
    fn from_idle(idle: Duration) -> State {
        if idle >= SLEEP_AFTER {
            State::Sleeping
        } else if idle >= SIT_AFTER {
            State::Sitting
        } else {
            State::Awake
        }
    }

    fn animation(self) -> &'static str {
        match self {
            State::Awake => "idle",
            State::Sitting => "sit",
            State::Sleeping => "sleep",
        }
    }
}

/// A change the caller must apply: switch animation, and whether the pet
/// just woke up (worth a short "stretches awake" bubble).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub animation: &'static str,
    pub woke_up: bool,
}

#[derive(Debug, Default)]
pub struct BehaviorTracker {
    state: Option<State>,
}

impl BehaviorTracker {
    /// Feeds a fresh idle-time sample. Returns a change when the animation
    /// should switch, or `None` when the state is unchanged.
    pub fn sample(&mut self, idle: Duration) -> Option<Change> {
        // Treat a very small idle time as "definitely awake" even if the
        // previous state was asleep, so a single stray long reading right
        // after start-up cannot make the pet nod off for no reason while
        // an equally stray low reading right after sleeping always wakes it.
        let next = if idle < AWAKE_BELOW {
            State::Awake
        } else {
            State::from_idle(idle)
        };
        let was_asleep = self.state.is_some_and(|s| s != State::Awake);
        if self.state == Some(next) {
            return None;
        }
        self.state = Some(next);
        Some(Change {
            animation: next.animation(),
            woke_up: next == State::Awake && was_asleep,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn stays_awake_until_the_threshold() {
        let mut t = BehaviorTracker::default();
        assert_eq!(t.sample(secs(0)).unwrap().animation, "idle");
        assert!(t.sample(secs(30)).is_none());
        assert!(t.sample(secs(119)).is_none());
    }

    #[test]
    fn sits_then_sleeps_as_idle_time_grows() {
        let mut t = BehaviorTracker::default();
        t.sample(secs(0));
        let c = t.sample(secs(121)).unwrap();
        assert_eq!((c.animation, c.woke_up), ("sit", false));
        assert!(t.sample(secs(300)).is_none());
        let c = t.sample(secs(601)).unwrap();
        assert_eq!((c.animation, c.woke_up), ("sleep", false));
    }

    #[test]
    fn waking_from_sleep_is_reported() {
        let mut t = BehaviorTracker::default();
        t.sample(secs(0));
        t.sample(secs(700));
        let c = t.sample(secs(0)).unwrap();
        assert_eq!((c.animation, c.woke_up), ("idle", true));
    }

    #[test]
    fn waking_from_sitting_is_also_reported() {
        let mut t = BehaviorTracker::default();
        t.sample(secs(0));
        t.sample(secs(150));
        let c = t.sample(secs(1)).unwrap();
        assert_eq!((c.animation, c.woke_up), ("idle", true));
    }

    #[test]
    fn first_sample_sets_the_state_without_counting_as_waking() {
        let mut t = BehaviorTracker::default();
        let c = t.sample(secs(700)).unwrap();
        assert_eq!((c.animation, c.woke_up), ("sleep", false));
    }

    #[test]
    fn a_brief_dip_still_counts_as_awake() {
        let mut t = BehaviorTracker::default();
        t.sample(secs(300));
        let c = t.sample(secs(2)).unwrap();
        assert_eq!((c.animation, c.woke_up), ("idle", true));
    }
}
