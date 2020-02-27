use async_std::task;
use std::time::{Instant, Duration};

/// A simple primitive that sleeps for the specified amount of time.
///
/// This watchdog doesn't support cancelling from other task and relies on dropping of the future.
pub struct Watchdog {
    reset_at: Instant,
    deadline: Duration,
}

impl Watchdog {
    pub fn new(deadline: Duration) -> Self {
        Self {
            reset_at: Instant::now(),
            deadline,
        }
    }

    /// Replenish the amount of time until this watchdog fires.
    ///
    /// Next call to `wait` will resolve at `deadline` (specified to the constructor).
    pub fn reset(&mut self) {
        self.reset_at = Instant::now();
    }

    /// Wait until the deadline expires.
    pub async fn wait(&self) {
        let deadline = self.reset_at + self.deadline;
        match deadline.checked_duration_since(Instant::now()) {
            Some(dur) => task::sleep(dur).await,
            // `Instant::now` is later than the deadline meaning that we are done here.
            None => {},
        }
    }
}
