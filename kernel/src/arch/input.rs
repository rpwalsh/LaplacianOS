//! Native pointer transport selected for each platform.

#[cfg(target_arch = "x86_64")]
pub use crate::arch::x86_64::virtio_input::{
    handle_irq, has_pending_event, init, irq_line, poll_input, set_display_bounds,
    try_read_event,
};

#[cfg(target_arch = "aarch64")]
pub fn set_display_bounds(width: u32, height: u32) {
    crate::arch::aarch64::virtio_mmio::set_input_display_bounds(width, height);
}

#[cfg(target_arch = "aarch64")]
pub fn init() -> bool {
    crate::arch::aarch64::virtio_mmio::init_input()
}

#[cfg(target_arch = "aarch64")]
pub fn irq_line() -> Option<u8> {
    // The QEMU virt MMIO transport is polled until the FDT interrupt-map is
    // attached to the GIC domain; polling consumes the same hardware queue.
    None
}

#[cfg(target_arch = "aarch64")]
pub fn handle_irq(_irq: u8) -> bool {
    crate::arch::aarch64::virtio_mmio::poll_input();
    true
}

#[cfg(target_arch = "aarch64")]
pub fn poll_input() {
    crate::arch::aarch64::virtio_mmio::poll_input();
}

#[cfg(target_arch = "aarch64")]
pub fn has_pending_event() -> bool {
    crate::arch::aarch64::virtio_mmio::input_has_pending_event()
}

#[cfg(target_arch = "aarch64")]
pub fn try_read_event() -> Option<crate::input::pointer::PointerEvent> {
    crate::arch::aarch64::virtio_mmio::input_try_read_event()
}
