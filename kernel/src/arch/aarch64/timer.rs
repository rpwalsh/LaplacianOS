//! AArch64 generic virtual timer with a 1 kHz scheduler tick.

use core::sync::atomic::{AtomicU64, Ordering};

pub const VIRTUAL_TIMER_IRQ: u32 = 27;
const TICKS_PER_SECOND: u64 = 1_000;
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn read_counter() -> u64 {
    let value: u64;
    unsafe { core::arch::asm!("mrs {0}, cntpct_el0", out(reg) value, options(nomem, nostack)) };
    value
}

pub fn frequency() -> u64 {
    let value: u64;
    unsafe { core::arch::asm!("mrs {0}, cntfrq_el0", out(reg) value, options(nomem, nostack)) };
    value
}

pub fn udelay(microseconds: u64) {
    let frequency = frequency();
    let ticks = (frequency / 1_000_000).saturating_mul(microseconds);
    let start = read_counter();
    while read_counter().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

pub fn init_periodic() {
    TICK_COUNT.store(0, Ordering::Relaxed);
    rearm();
    super::gic::enable_irq(VIRTUAL_TIMER_IRQ);
}

fn rearm() {
    let interval = (frequency() / TICKS_PER_SECOND).max(1);
    unsafe {
        core::arch::asm!(
            "msr cntv_tval_el0, {interval}",
            "mov {control}, #1",
            "msr cntv_ctl_el0, {control}",
            "isb",
            interval = in(reg) interval,
            control = out(reg) _,
            options(nomem, nostack),
        );
    }
}

pub fn handle_irq() {
    TICK_COUNT.fetch_add(1, Ordering::Relaxed);
    rearm();
}

pub fn ticks() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}
