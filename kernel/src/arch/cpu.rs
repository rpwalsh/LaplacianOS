//! Architecture-neutral CPU control primitives used by the scheduler and
//! monotonic telemetry.  Keeping these operations here prevents kernel-wide
//! call sites from embedding one ISA's privileged instructions.

#[inline]
pub fn disable_interrupts() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #2", options(nomem, nostack));
    }
}

#[inline]
pub fn enable_interrupts() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }
}

#[inline]
pub fn wait_for_interrupt() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack));
    }
}

/// Stable hardware execution-context identifier for per-CPU state.
#[inline]
pub fn hardware_thread_id() -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::lapic::lapic_id() as u32
    }
    #[cfg(target_arch = "aarch64")]
    {
        let mpidr: u64;
        unsafe {
            core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
        }
        // Preserve Aff2:Aff1:Aff0 while discarding the MT and reserved bits.
        ((mpidr & 0x00ff_ffff) | ((mpidr >> 8) & 0xff00_0000)) as u32
    }
}

/// Architecture counter used only as a high-resolution monotonic sample.
#[inline]
pub fn cycle_counter() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::aarch64::timer::read_counter()
    }
}

#[inline]
pub fn stack_pointer() -> u64 {
    let value: u64;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) value, options(nomem, nostack));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) value, options(nomem, nostack));
    }
    value
}
