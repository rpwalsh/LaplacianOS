//! AArch64 architecture module.

pub mod boot;
pub mod exceptions;
pub mod fdt;
pub mod gic;
pub mod mm;
pub mod pci;
pub mod serial;
pub mod timer;
pub mod user;
pub mod virtio_mmio;

/// Initialise AArch64 hardware.
pub fn init(dtb: u64) {
    serial::init();
    fdt::init(dtb);
    gic::init();
}

/// Halt the current CPU core.
pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack));
        }
    }
}

/// Spin for `us` microseconds.
pub fn udelay(us: u64) {
    timer::udelay(us);
}
