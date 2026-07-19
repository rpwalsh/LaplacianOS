//! Spectral graph snapshots and signed change detection.

use crate::arch::serial;
use crate::graph::types::*;
use spin::Mutex;

pub const SPECTRAL_K: usize = 8;
pub const SNAPSHOT_RING_SIZE: usize = 64;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpectralSnapshot {
    pub generation: u64,
    pub eigenvalues: [Weight; SPECTRAL_K],
    pub gaps: [Weight; SPECTRAL_K],
    pub total_weight: u64,
    pub node_count: u32,
    pub edge_count: u32,
}

impl SpectralSnapshot {
    pub const EMPTY: Self = Self {
        generation: 0,
        eigenvalues: [0; SPECTRAL_K],
        gaps: [0; SPECTRAL_K],
        total_weight: 0,
        node_count: 0,
        edge_count: 0,
    };

    pub const fn fiedler(&self) -> Weight {
        self.eigenvalues[1]
    }

    pub const fn primary_gap(&self) -> Weight {
        self.gaps[0]
    }

    pub const fn is_valid(&self) -> bool {
        self.generation > 0
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ReviewSignalDetector {
    pub mu_0: Weight,
    pub slack: Weight,
    pub threshold: Weight,
    pub s_pos: Weight,
    pub s_neg: Weight,
    pub alarm_count: u32,
    baseline_sum: u64,
    baseline_count: u32,
    baseline_target: u32,
}

impl ReviewSignalDetector {
    pub const DEFAULT: Self = Self {
        mu_0: 0,
        slack: 655,       // 0.01 Q16
        threshold: 6_554, // 0.10 Q16
        s_pos: 0,
        s_neg: 0,
        alarm_count: 0,
        baseline_sum: 0,
        baseline_count: 0,
        baseline_target: 16,
    };

    pub fn observe(&mut self, algebraic_connectivity: Weight) -> bool {
        if self.baseline_count < self.baseline_target {
            self.baseline_sum = self
                .baseline_sum
                .saturating_add(algebraic_connectivity as u64);
            self.baseline_count += 1;
            if self.baseline_count == self.baseline_target {
                self.mu_0 = (self.baseline_sum / self.baseline_count as u64) as Weight;
            }
            return false;
        }

        let positive = self.s_pos as i64 + algebraic_connectivity as i64
            - self.mu_0 as i64
            - self.slack as i64;
        let negative = self.s_neg as i64 + self.mu_0 as i64
            - algebraic_connectivity as i64
            - self.slack as i64;
        self.s_pos = positive.max(0).min(Weight::MAX as i64) as Weight;
        self.s_neg = negative.max(0).min(Weight::MAX as i64) as Weight;
        let alarm = self.s_neg > self.threshold;
        if alarm {
            self.alarm_count = self.alarm_count.saturating_add(1);
        }
        alarm
    }

    pub fn reset(&mut self, new_mu_0: Weight) {
        self.mu_0 = new_mu_0;
        self.s_pos = 0;
        self.s_neg = 0;
        self.alarm_count = 0;
        self.baseline_sum = new_mu_0 as u64;
        self.baseline_count = self.baseline_target;
    }

    pub fn begin_baseline(&mut self, observations: u32) {
        self.mu_0 = 0;
        self.s_pos = 0;
        self.s_neg = 0;
        self.alarm_count = 0;
        self.baseline_sum = 0;
        self.baseline_count = 0;
        self.baseline_target = observations.max(1);
    }
}

struct SnapshotState {
    snapshots: [SpectralSnapshot; SNAPSHOT_RING_SIZE],
    next: usize,
    count: usize,
    total: u64,
    detector: ReviewSignalDetector,
    alarm: bool,
}

impl SnapshotState {
    const NEW: Self = Self {
        snapshots: [SpectralSnapshot::EMPTY; SNAPSHOT_RING_SIZE],
        next: 0,
        count: 0,
        total: 0,
        detector: ReviewSignalDetector::DEFAULT,
        alarm: false,
    };
}

static STATE: Mutex<SnapshotState> = Mutex::new(SnapshotState::NEW);

pub fn init() {
    *STATE.lock() = SnapshotState::NEW;
    serial::write_line(b"[graph] spectral snapshot ring initialized");
}

pub fn record_snapshot(snapshot: SpectralSnapshot) -> bool {
    if snapshot.node_count == 0
        || snapshot.eigenvalues[0] > 512
        || snapshot
            .eigenvalues
            .iter()
            .any(|value| *value > 2 * WEIGHT_ONE + 512)
    {
        return false;
    }
    let mut state = STATE.lock();
    let slot = state.next;
    state.snapshots[slot] = snapshot;
    state.next = (slot + 1) % SNAPSHOT_RING_SIZE;
    state.count = (state.count + 1).min(SNAPSHOT_RING_SIZE);
    state.total = state.total.saturating_add(1);
    state.alarm = state.detector.observe(snapshot.fiedler());
    true
}

pub fn latest_snapshot() -> Option<SpectralSnapshot> {
    snapshot_at_offset(0)
}

pub fn snapshot_at_offset(offset: usize) -> Option<SpectralSnapshot> {
    let state = STATE.lock();
    if offset >= state.count {
        return None;
    }
    let index = (state.next + SNAPSHOT_RING_SIZE - 1 - offset) % SNAPSHOT_RING_SIZE;
    Some(state.snapshots[index])
}

pub fn review_alarm() -> bool {
    STATE.lock().alarm
}

pub fn review_alarm_count() -> u32 {
    STATE.lock().detector.alarm_count
}

pub fn review_alarm_reset(new_mu_0: Weight) {
    let mut state = STATE.lock();
    state.detector.reset(new_mu_0);
    state.alarm = false;
}

pub fn total_snapshots() -> u64 {
    STATE.lock().total
}

pub fn fiedler_drift(window: usize) -> Option<(i64, bool)> {
    if window < 2 {
        return None;
    }
    let newest = snapshot_at_offset(0)?;
    let oldest = snapshot_at_offset(window - 1)?;
    let drift = newest.fiedler() as i64 - oldest.fiedler() as i64;
    Some((drift, drift < 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stationary_data_does_not_accumulate_and_shift_triggers() {
        let mut detector = ReviewSignalDetector::DEFAULT;
        detector.begin_baseline(4);
        for _ in 0..4 {
            assert!(!detector.observe(WEIGHT_ONE));
        }
        for offset in [200, 0, 100, 0, 200, 0] {
            assert!(!detector.observe(WEIGHT_ONE + offset));
        }
        let mut alarm = false;
        for _ in 0..16 {
            alarm |= detector.observe(WEIGHT_ONE - 4_000);
        }
        assert!(alarm);
        detector.reset(WEIGHT_ONE);
        assert_eq!(detector.s_neg, 0);
    }
}

pub fn dump() {
    serial::write_line(b"[graph] spectral snapshot ring reset");
}
