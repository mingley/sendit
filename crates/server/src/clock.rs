use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / sendit_sim::TICK_RATE as u64);

pub(crate) struct TickClock {
    next: Instant,
}

impl TickClock {
    pub fn new() -> Self {
        Self {
            next: Instant::now(),
        }
    }

    pub fn delay(&self) -> Option<Duration> {
        self.next.checked_duration_since(Instant::now())
    }

    pub fn finish(&mut self, after: Instant) -> (bool, u64) {
        self.next += TICK;
        let late = after > self.next;
        let mut skipped = 0;
        // Limit catch-up after suspension. A simulation step still advances exactly one tick.
        if after.saturating_duration_since(self.next) > TICK * 8 {
            skipped = (after.duration_since(self.next).as_nanos() / TICK.as_nanos()) as u64;
            self.next += Duration::from_secs_f64(TICK.as_secs_f64() * skipped as f64);
        }
        (late, skipped)
    }
}
