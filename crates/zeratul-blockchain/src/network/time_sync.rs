//! Time Synchronization
//!
//! Tracks current timeslot for Safrole consensus.
//! JAM uses 6-second timeslots starting from the Common Era (Unix epoch 0).

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Time synchronization configuration
#[derive(Clone, Debug)]
pub struct TimeSyncConfig {
    /// Timeslot duration in milliseconds
    pub timeslot_duration_ms: u64,

    /// Maximum clock drift allowed (in timeslots)
    pub max_drift_slots: u64,

    /// JAM Common Era offset (Unix timestamp in ms)
    /// Default: 0 (Unix epoch)
    pub common_era_offset_ms: u64,
}

impl Default for TimeSyncConfig {
    fn default() -> Self {
        Self {
            timeslot_duration_ms: 6000, // 6 seconds (JAM spec)
            max_drift_slots: 3,          // Allow 18 seconds drift
            common_era_offset_ms: 0,     // Unix epoch
        }
    }
}

/// Time synchronization tracker
pub struct TimeSync {
    config: TimeSyncConfig,

    /// System time at startup (for drift detection)
    startup_time: SystemTime,

    /// Estimated clock offset from network (milliseconds)
    /// Positive: our clock is ahead, Negative: our clock is behind
    estimated_offset_ms: i64,
}

impl TimeSync {
    /// Create new time sync
    pub fn new(config: TimeSyncConfig) -> Self {
        Self {
            config,
            startup_time: SystemTime::now(),
            estimated_offset_ms: 0,
        }
    }

    /// Get current timeslot based on system time
    pub fn current_timeslot(&self) -> u64 {
        let now_ms = self.current_time_ms();
        self.timeslot_from_ms(now_ms)
    }

    /// Get current time in milliseconds since JAM Common Era
    pub fn current_time_ms(&self) -> u64 {
        let now = SystemTime::now();
        let duration = now
            .duration_since(UNIX_EPOCH)
            .expect("System time before Unix epoch");

        let unix_ms = duration.as_millis() as u64;

        // Adjust for Common Era offset and estimated clock offset
        let adjusted_ms = (unix_ms as i64 - self.config.common_era_offset_ms as i64
            + self.estimated_offset_ms) as u64;

        adjusted_ms
    }

    /// Convert milliseconds to timeslot
    pub fn timeslot_from_ms(&self, ms: u64) -> u64 {
        ms / self.config.timeslot_duration_ms
    }

    /// Convert timeslot to milliseconds (start of slot)
    pub fn timeslot_to_ms(&self, timeslot: u64) -> u64 {
        timeslot * self.config.timeslot_duration_ms
    }

    /// Get time until next timeslot (in milliseconds)
    pub fn ms_until_next_slot(&self) -> u64 {
        let now_ms = self.current_time_ms();
        let current_slot = self.timeslot_from_ms(now_ms);
        let next_slot_start = self.timeslot_to_ms(current_slot + 1);

        next_slot_start.saturating_sub(now_ms)
    }

    /// Update clock offset based on observed block timestamp
    ///
    /// This implements a simple clock synchronization:
    /// - If we see blocks from the future, our clock is behind
    /// - If we see blocks from the past, our clock is ahead
    ///
    /// Uses exponential moving average for smooth adjustment.
    pub fn update_from_block(&mut self, block_timeslot: u64) {
        let our_timeslot = self.current_timeslot();

        // Calculate time difference
        let diff = (block_timeslot as i64 - our_timeslot as i64) * self.config.timeslot_duration_ms as i64;

        // Check if within acceptable drift
        if diff.abs() > (self.config.max_drift_slots * self.config.timeslot_duration_ms) as i64 {
            warn!(
                our_slot = our_timeslot,
                block_slot = block_timeslot,
                diff_ms = diff,
                "Large clock drift detected"
            );
        }

        // Update offset with exponential moving average (alpha = 0.1)
        // This smooths out transient network delays
        let alpha = 0.1;
        self.estimated_offset_ms =
            ((1.0 - alpha) * self.estimated_offset_ms as f64 + alpha * diff as f64) as i64;

        debug!(
            our_slot = our_timeslot,
            block_slot = block_timeslot,
            offset_ms = self.estimated_offset_ms,
            "Updated clock offset from block"
        );
    }

    /// Check if a block timeslot is valid (not too far in future)
    pub fn is_timeslot_valid(&self, block_timeslot: u64) -> bool {
        let our_timeslot = self.current_timeslot();

        // Allow some drift into the future
        block_timeslot <= our_timeslot + self.config.max_drift_slots
    }

    /// Get estimated clock offset in milliseconds
    pub fn estimated_offset_ms(&self) -> i64 {
        self.estimated_offset_ms
    }

    /// Get duration since startup
    pub fn uptime(&self) -> Duration {
        SystemTime::now()
            .duration_since(self.startup_time)
            .expect("Time went backwards")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeslot_conversion() {
        let config = TimeSyncConfig::default();
        let time_sync = TimeSync::new(config);

        // 6000ms = timeslot 1
        assert_eq!(time_sync.timeslot_from_ms(6000), 1);

        // 12000ms = timeslot 2
        assert_eq!(time_sync.timeslot_from_ms(12000), 2);

        // 5999ms = timeslot 0 (not reached timeslot 1 yet)
        assert_eq!(time_sync.timeslot_from_ms(5999), 0);

        // Reverse conversion
        assert_eq!(time_sync.timeslot_to_ms(1), 6000);
        assert_eq!(time_sync.timeslot_to_ms(2), 12000);
    }

    #[test]
    fn test_timeslot_validation() {
        let config = TimeSyncConfig {
            timeslot_duration_ms: 6000,
            max_drift_slots: 3,
            common_era_offset_ms: 0,
        };

        let time_sync = TimeSync::new(config);
        let current = time_sync.current_timeslot();

        // Current slot is valid
        assert!(time_sync.is_timeslot_valid(current));

        // Future within drift is valid
        assert!(time_sync.is_timeslot_valid(current + 2));

        // Far future is invalid
        assert!(!time_sync.is_timeslot_valid(current + 10));
    }

    #[test]
    fn test_clock_offset_update() {
        let config = TimeSyncConfig::default();
        let mut time_sync = TimeSync::new(config);

        let initial_offset = time_sync.estimated_offset_ms();

        // Simulate seeing a block from the future
        let our_slot = time_sync.current_timeslot();
        time_sync.update_from_block(our_slot + 1);

        // Offset should adjust (we're behind)
        let new_offset = time_sync.estimated_offset_ms();
        assert!(new_offset != initial_offset);
    }
}
