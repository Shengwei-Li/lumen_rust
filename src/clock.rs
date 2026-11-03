//! Recording time: starts at the first frame sent and stands still while paused.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    start: Option<Instant>,
    paused_at: Option<Instant>,
    paused_for: Duration,
}

#[derive(Default)]
pub struct Clock(Mutex<State>);

impl Clock {
    /// Starts the clock if it is not running yet.
    pub fn start(&self) {
        let mut s = self.0.lock().unwrap();
        s.start.get_or_insert_with(Instant::now);
    }

    /// Recorded time so far; `None` before the first frame.
    pub fn active(&self) -> Option<Duration> {
        let s = self.0.lock().unwrap();
        let start = s.start?;
        let now = s.paused_at.unwrap_or_else(Instant::now);
        Some(now.duration_since(start).saturating_sub(s.paused_for))
    }

    pub fn paused(&self) -> bool {
        self.0.lock().unwrap().paused_at.is_some()
    }

    pub fn set_paused(&self, paused: bool) {
        let mut s = self.0.lock().unwrap();
        match (paused, s.paused_at) {
            (true, None) => s.paused_at = Some(Instant::now()),
            (false, Some(at)) => {
                s.paused_for += at.elapsed();
                s.paused_at = None;
            }
            _ => {}
        }
    }
}
