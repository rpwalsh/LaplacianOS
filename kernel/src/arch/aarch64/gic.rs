//! GICv2 distributor and CPU-interface driver for the QEMU `virt` platform.

const GICD_BASE: u64 = 0x0800_0000;
const GICC_BASE: u64 = 0x0801_0000;
const GICD_CTLR: u64 = 0x000;
const GICD_TYPER: u64 = 0x004;
const GICD_ISENABLER: u64 = 0x100;
const GICD_ICENABLER: u64 = 0x180;
const GICD_ICPENDR: u64 = 0x280;
const GICD_IPRIORITYR: u64 = 0x400;
const GICD_ITARGETSR: u64 = 0x800;

pub fn init() {
    unsafe {
        core::ptr::write_volatile((GICD_BASE + GICD_CTLR) as *mut u32, 0);
        let lines = (((core::ptr::read_volatile((GICD_BASE + GICD_TYPER) as *const u32) & 0x1f)
            + 1)
            * 32)
            .min(1020);
        for irq in (32..lines).step_by(32) {
            let offset = (irq / 32) as u64 * 4;
            core::ptr::write_volatile((GICD_BASE + GICD_ICENABLER + offset) as *mut u32, u32::MAX);
            core::ptr::write_volatile((GICD_BASE + GICD_ICPENDR + offset) as *mut u32, u32::MAX);
        }
        for irq in (0..lines).step_by(4) {
            core::ptr::write_volatile(
                (GICD_BASE + GICD_IPRIORITYR + irq as u64) as *mut u32,
                0xa0a0_a0a0,
            );
            if irq >= 32 {
                core::ptr::write_volatile(
                    (GICD_BASE + GICD_ITARGETSR + irq as u64) as *mut u32,
                    0x0101_0101,
                );
            }
        }
        core::ptr::write_volatile((GICD_BASE + GICD_CTLR) as *mut u32, 1);
        core::ptr::write_volatile(GICC_BASE as *mut u32, 1);
        core::ptr::write_volatile((GICC_BASE + 0x04) as *mut u32, 0xff);
        core::arch::asm!("dsb sy", "isb", options(nomem, nostack));
    }
}

pub fn enable_irq(irq: u32) {
    let register = (GICD_BASE + GICD_ISENABLER + (irq / 32) as u64 * 4) as *mut u32;
    unsafe {
        core::ptr::write_volatile(register, 1 << (irq % 32));
        core::arch::asm!("dsb sy", options(nomem, nostack));
    }
}

pub fn ack_irq() -> u32 {
    unsafe { core::ptr::read_volatile((GICC_BASE + 0x0c) as *const u32) & 0x3ff }
}

pub fn eoi_irq(irq: u32) {
    unsafe { core::ptr::write_volatile((GICC_BASE + 0x10) as *mut u32, irq) }
}
