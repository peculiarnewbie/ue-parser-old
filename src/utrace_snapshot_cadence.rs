//! Limit full live-frame snapshots without delaying the first or final update.

#[derive(Default)]
pub(crate) struct SnapshotCadence {
    last_frame_ms: Option<f64>,
    received_bytes: u64,
}

impl SnapshotCadence {
    pub(crate) fn pushed(&mut self, bytes: usize) {
        self.received_bytes = self.received_bytes.saturating_add(bytes as u64);
    }

    pub(crate) fn frames_due(&mut self, now_ms: f64, total_bytes: u64) -> bool {
        const MIN_FRAME_SNAPSHOT_MS: f64 = 50.0;
        if self.received_bytes >= total_bytes
            || self
                .last_frame_ms
                .is_none_or(|last| now_ms - last >= MIN_FRAME_SNAPSHOT_MS)
        {
            self.last_frame_ms = Some(now_ms);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_periodic_and_final_snapshots_are_delivered() {
        let mut cadence = SnapshotCadence::default();
        cadence.pushed(1);
        assert!(cadence.frames_due(10.0, 100));
        cadence.pushed(1);
        assert!(!cadence.frames_due(59.0, 100));
        assert!(cadence.frames_due(60.0, 100));
        cadence.pushed(98);
        assert!(cadence.frames_due(61.0, 100));
    }

    #[test]
    fn independent_sessions_publish_their_own_first_snapshot() {
        let mut first = SnapshotCadence::default();
        let mut second = SnapshotCadence::default();
        assert!(first.frames_due(5.0, 100));
        assert!(second.frames_due(6.0, 100));
        assert!(!first.frames_due(6.0, 100));
    }
}
