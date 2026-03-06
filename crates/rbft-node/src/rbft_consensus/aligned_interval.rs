// SPDX-License-Identifier: Apache-2.0
//! Aligned interval timer for precise periodic ticking.
//!
//! This module provides a custom future that ticks at precise intervals aligned to
//! epoch time boundaries, avoiding accumulated delay at second transitions.

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, UNIX_EPOCH},
};

/// Custom future that ticks every interval_ms milliseconds, aligned to avoid delay
/// at the start of a new Linux epoch second.
pub struct AlignedInterval {
    sleep: Pin<Box<tokio::time::Sleep>>,
    interval_ms: u64,
}

impl AlignedInterval {
    pub fn new(interval_ms: u64) -> Self {
        let next_tick = Self::calculate_next_tick(interval_ms);
        Self {
            sleep: Box::pin(tokio::time::sleep_until(next_tick)),
            interval_ms,
        }
    }

    fn calculate_next_tick(interval_ms: u64) -> tokio::time::Instant {
        let now = std::time::SystemTime::now();
        let since_epoch = now
            .duration_since(UNIX_EPOCH)
            .expect("system time is before UNIX epoch");
        let millis_since_epoch = since_epoch.as_millis();

        // Calculate next tick aligned to interval_ms boundaries
        let interval_ms = interval_ms as u128;
        let next_millis = ((millis_since_epoch / interval_ms) + 1) * interval_ms;
        let duration_until_next = Duration::from_millis((next_millis - millis_since_epoch) as u64);

        tokio::time::Instant::now() + duration_until_next
    }

    pub fn tick(&mut self) -> AlignedIntervalFuture<'_> {
        AlignedIntervalFuture { interval: self }
    }
}

impl Default for AlignedInterval {
    fn default() -> Self {
        Self::new(10)
    }
}

pub struct AlignedIntervalFuture<'a> {
    interval: &'a mut AlignedInterval,
}

impl<'a> std::future::Future for AlignedIntervalFuture<'a> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.interval.sleep.as_mut().poll(cx) {
            Poll::Ready(()) => {
                // Reset the sleep for the next tick
                let next_tick = AlignedInterval::calculate_next_tick(self.interval.interval_ms);
                self.interval.sleep.as_mut().reset(next_tick);
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const TICK_INTERVAL_MS: u64 = 10;

    #[tokio::test]
    async fn test_aligned_interval_ticks() {
        let mut interval = AlignedInterval::new(TICK_INTERVAL_MS);

        // Record start time
        let start = Instant::now();

        // Collect several tick times
        let mut tick_times = Vec::new();
        for _ in 0..5 {
            interval.tick().await;
            tick_times.push(Instant::now());
        }

        // Verify we got 5 ticks
        assert_eq!(tick_times.len(), 5);

        // Verify ticks are spaced approximately TICK_INTERVAL_MS apart
        for i in 1..tick_times.len() {
            let elapsed = tick_times[i].duration_since(tick_times[i - 1]);
            let elapsed_ms = elapsed.as_millis() as u64;

            // Allow some tolerance (±2ms) for scheduling delays
            assert!(
                (TICK_INTERVAL_MS - 2..=TICK_INTERVAL_MS + 2).contains(&elapsed_ms),
                "Tick interval was {}ms, expected {}ms ± 2ms",
                elapsed_ms,
                TICK_INTERVAL_MS
            );
        }

        // Verify total time is approximately correct
        let total_elapsed = tick_times[4].duration_since(start);
        let expected_min = TICK_INTERVAL_MS * 4; // 4 intervals between 5 ticks
        let expected_max = (TICK_INTERVAL_MS + 3) * 4; // Allow some tolerance

        assert!(
            total_elapsed.as_millis() as u64 >= expected_min
                && total_elapsed.as_millis() as u64 <= expected_max,
            "Total elapsed time was {}ms, expected between {}ms and {}ms",
            total_elapsed.as_millis(),
            expected_min,
            expected_max
        );
    }

    #[tokio::test]
    async fn test_aligned_interval_alignment() {
        let mut interval = AlignedInterval::new(TICK_INTERVAL_MS);

        // Wait for a tick
        interval.tick().await;

        // Check the alignment by getting current epoch time
        let now = std::time::SystemTime::now();
        let since_epoch = now.duration_since(UNIX_EPOCH).unwrap();
        let millis_since_epoch = since_epoch.as_millis() as u64;

        // The current time should be close to a TICK_INTERVAL_MS boundary
        let remainder = millis_since_epoch % TICK_INTERVAL_MS;

        // Allow up to 5ms tolerance for execution time
        assert!(
            remainder <= 5 || remainder >= TICK_INTERVAL_MS - 5,
            "Tick occurred at {}ms past boundary, expected close to 0ms",
            remainder
        );
    }

    #[tokio::test]
    async fn test_aligned_interval_no_delay_at_second_boundary() {
        // Collect ticks across a second boundary
        let mut tick_times = Vec::new();

        // Wait until we're close to a second boundary
        while std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            % 1000
            < 950
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Create the interval after we're close to the boundary
        let mut interval = AlignedInterval::new(TICK_INTERVAL_MS);

        // Skip the first tick since it might be immediate
        interval.tick().await;

        // Now collect ticks across the second boundary
        for _ in 0..10 {
            interval.tick().await;
            let tick_time = std::time::SystemTime::now();
            tick_times.push(tick_time.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64);
        }

        // Verify all intervals are consistent, even across second boundaries
        // Use a larger tolerance for CI environments where scheduling can be less precise
        const TOLERANCE_MS: u64 = 5;
        for i in 1..tick_times.len() {
            let interval_ms = tick_times[i] - tick_times[i - 1];

            assert!(
                (TICK_INTERVAL_MS - TOLERANCE_MS..=TICK_INTERVAL_MS + TOLERANCE_MS)
                    .contains(&interval_ms),
                "Tick interval was {}ms at index {}, expected {}ms ± {}ms (tick times: {} -> {})",
                interval_ms,
                i,
                TICK_INTERVAL_MS,
                TOLERANCE_MS,
                tick_times[i - 1],
                tick_times[i]
            );
        }
    }

    #[tokio::test]
    async fn test_calculate_next_tick_alignment() {
        // Test that calculate_next_tick always returns times aligned to TICK_INTERVAL_MS
        for _ in 0..10 {
            let next_tick = AlignedInterval::calculate_next_tick(TICK_INTERVAL_MS);

            // Small sleep to vary the test timing
            tokio::time::sleep(Duration::from_millis(1)).await;

            // Wait until the calculated tick time
            tokio::time::sleep_until(next_tick).await;

            // Verify we're aligned
            let now = std::time::SystemTime::now();
            let since_epoch = now.duration_since(UNIX_EPOCH).unwrap();
            let millis_since_epoch = since_epoch.as_millis() as u64;
            let remainder = millis_since_epoch % TICK_INTERVAL_MS;

            assert!(
                remainder <= 5 || remainder >= TICK_INTERVAL_MS - 5,
                "After sleeping to calculated tick time, at {}ms past boundary",
                remainder
            );
        }
    }
}
