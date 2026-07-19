//! EL0 process entry for AArch64.
//!
//! AArch64 exceptions taken from EL0 automatically use SP_EL1, so the
//! scheduler's current kernel stack remains the syscall/exception stack.

/// The x86 fast-syscall gate needs per-CPU MSR setup.  AArch64 SVC dispatches
/// through VBAR_EL1, installed by the exception subsystem, and therefore has
/// no separate per-CPU fast-gate programming step.
pub fn init_fast_syscalls(_cpu: usize) {
    super::exceptions::install_vectors();
}

/// SP_EL1 is already the scheduler's active kernel stack.  This function is
/// retained in the portable scheduler contract and verifies the supplied
/// stack is aligned instead of silently accepting an unusable exception stack.
pub fn set_syscall_kernel_stack(stack_top: u64) {
    assert!(stack_top != 0 && stack_top & 0xf == 0);
}

#[inline(never)]
pub unsafe fn enter_user_mode(entry: u64, stack: u64) -> ! {
    unsafe { enter_user_mode_with_arg(entry, stack, 0) }
}

/// Enter EL0t with interrupts unmasked and x0 carrying the thread argument.
///
/// # Safety
/// `entry` and `stack` must be mapped in the current TTBR0_EL1 address space;
/// `stack` must be 16-byte aligned as required by AAPCS64.
#[inline(never)]
pub unsafe fn enter_user_mode_with_arg(entry: u64, stack: u64, arg: u64) -> ! {
    assert!(entry != 0 && stack != 0 && stack & 0xf == 0);
    unsafe {
        core::arch::asm!(
            "msr sp_el0, {stack}",
            "msr elr_el1, {entry}",
            // EL0t (M[3:0]=0), all DAIF interrupt masks cleared.
            "msr spsr_el1, xzr",
            "mov x0, {arg}",
            "isb",
            "eret",
            stack = in(reg) stack,
            entry = in(reg) entry,
            arg = in(reg) arg,
            options(noreturn)
        );
    }
}
