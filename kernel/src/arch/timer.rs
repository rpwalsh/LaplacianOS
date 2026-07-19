#[cfg(target_arch = "x86_64")]
pub use crate::arch::x86_64::timer::{
    SCHED_TIME_SLICE_TICKS, init, quantum_remaining, reset_quantum, set_quantum, tick, ticks,
};

#[cfg(target_arch = "aarch64")]
mod aarch64_timer {
    use core::sync::atomic::{AtomicU64, Ordering};

    pub const SCHED_TIME_SLICE_TICKS: u64 = 10;

    static BOOT_COUNTER: AtomicU64 = AtomicU64::new(0);
    static QUANTUM: AtomicU64 = AtomicU64::new(SCHED_TIME_SLICE_TICKS);

    pub fn init() {
        BOOT_COUNTER.store(
            super::super::aarch64::timer::read_counter(),
            Ordering::Relaxed,
        );
        QUANTUM.store(SCHED_TIME_SLICE_TICKS, Ordering::Relaxed);
        super::super::aarch64::timer::init_periodic();
    }

    pub fn ticks() -> u64 {
        let now = super::super::aarch64::timer::read_counter();
        let start = BOOT_COUNTER.load(Ordering::Relaxed);
        let freq = super::super::aarch64::timer::frequency();
        if freq == 0 {
            return 0;
        }
        (now.saturating_sub(start)).saturating_mul(1000) / freq
    }

    pub fn set_quantum(q: u64) {
        QUANTUM.store(q.max(1), Ordering::Relaxed);
    }

    pub fn reset_quantum() {
        QUANTUM.store(SCHED_TIME_SLICE_TICKS, Ordering::Relaxed);
    }

    pub fn quantum_remaining() -> u64 {
        QUANTUM.load(Ordering::Relaxed)
    }

    /// Advance the architecture-neutral 1 kHz kernel clock from the ARM
    /// generic-timer IRQ and run the same maintenance work as the x86 PIT.
    pub fn tick() -> bool {
        let now = super::super::aarch64::timer::ticks();
        crate::graph::twin::ingest_irq_tick(now);
        crate::net::tcp::tick_retransmits(now);
        if now.is_multiple_of(1000) {
            crate::net::tcp::tick_timers(now, 1000);
        }
        if now.is_multiple_of(5000) {
            crate::security::audit::drain_to_vfs_log();
        }
        let remaining = QUANTUM.fetch_sub(1, Ordering::Relaxed);
        if remaining <= 1 {
            QUANTUM.store(SCHED_TIME_SLICE_TICKS, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub use aarch64_timer::{
    SCHED_TIME_SLICE_TICKS, init, quantum_remaining, reset_quantum, set_quantum, tick, ticks,
};
