//! AArch64 EL1 exception vectors, syscall entry, and interrupt dispatch.

use core::arch::global_asm;

pub const EC_SVC64: u32 = 0x15;
pub const EC_DABT_EL0: u32 = 0x24;
pub const EC_DABT_EL1: u32 = 0x25;

unsafe extern "C" {
    static exception_vector_table: u8;
}

/// Install the 2 KiB-aligned EL1 vector table used for EL0 SVC and IRQ entry.
pub fn install_vectors() {
    let vectors = core::ptr::addr_of!(exception_vector_table) as u64;
    debug_assert_eq!(vectors & 0x7ff, 0);
    unsafe {
        core::arch::asm!(
            "msr vbar_el1, {}",
            "isb",
            in(reg) vectors,
            options(nostack)
        );
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn aarch64_syscall_handler(
    nr: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> u64 {
    crate::syscall::dispatch(nr, &[a0, a1, a2, a3, a4, a5])
}

/// Resolve an EL0 data abort through the architecture-neutral process VMA
/// fault handler.  The address-space layer uses the x86-compatible low two
/// bits as its portable fault contract: bit 0 means protection/present and
/// bit 1 means write.  AArch64 supplies those facts as FSC and WnR fields.
#[unsafe(no_mangle)]
pub extern "C" fn aarch64_el0_data_abort(esr: u64, far: u64) -> u64 {
    let iss = esr & 0x01ff_ffff;
    let fsc = iss & 0x3f;
    let is_write = iss & (1 << 6) != 0;
    let is_translation_fault = (0x04..=0x07).contains(&fsc);
    let is_access_or_permission_fault = (0x08..=0x0f).contains(&fsc);
    if !is_translation_fault && !is_access_or_permission_fault {
        return 0;
    }

    let mut portable_error = 0u64;
    if is_access_or_permission_fault {
        portable_error |= 1 << 0;
    }
    if is_write {
        portable_error |= 1 << 1;
    }
    let current = crate::sched::current_index();
    u64::from(
        current != 0
            && crate::task::table::is_user_task(current)
            && crate::task::table::handle_user_page_fault(current, far, portable_error),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn aarch64_irq_handler() {
    let irq = super::gic::ack_irq();
    if irq >= 1020 {
        return;
    }
    if irq == super::timer::VIRTUAL_TIMER_IRQ {
        super::timer::handle_irq();
        let quantum_expired = crate::arch::timer::tick();
        crate::sched::tick_advance();
        let now = crate::arch::timer::ticks();
        if now.is_multiple_of(500) {
            crate::svc::watchdog_check();
            crate::svc::drain_restart_queue();
        }
        crate::ui::desktop::pump_frame_clock_from_timer(now);
        crate::input::pointer::poll_input();
        // Complete the GIC transaction before switching away. The vector
        // frame contains ELR/SPSR and all GPRs, so this C activation can be
        // suspended on the outgoing task's stack and resumed safely later.
        super::gic::eoi_irq(irq);
        if quantum_expired {
            unsafe { crate::sched::preempt() };
        }
        return;
    } else if irq <= u8::MAX as u32 {
        let _ = crate::drivers::dispatch_irq(irq as u8);
    }
    super::gic::eoi_irq(irq);
}

#[unsafe(no_mangle)]
pub extern "C" fn aarch64_sync_exception(esr: u64, far: u64, elr: u64) -> ! {
    let current_el: u64;
    let spsr: u64;
    let sp_el0: u64;
    let sctlr: u64;
    let cpacr: u64;
    let ttbr0: u64;
    unsafe {
        core::arch::asm!(
            "mrs {current_el}, currentel",
            "mrs {spsr}, spsr_el1",
            "mrs {sp_el0}, sp_el0",
            "mrs {sctlr}, sctlr_el1",
            "mrs {cpacr}, cpacr_el1",
            "mrs {ttbr0}, ttbr0_el1",
            current_el = out(reg) current_el,
            spsr = out(reg) spsr,
            sp_el0 = out(reg) sp_el0,
            sctlr = out(reg) sctlr,
            cpacr = out(reg) cpacr,
            ttbr0 = out(reg) ttbr0,
            options(nomem, nostack),
        );
    }
    super::serial::write_bytes(b"[aarch64] sync exception ESR=");
    super::serial::write_hex_raw(esr);
    super::serial::write_bytes(b" FAR=");
    super::serial::write_hex_raw(far);
    super::serial::write_bytes(b" ELR=");
    super::serial::write_hex_raw(elr);
    super::serial::write_bytes_raw(b"\r\n");
    super::serial::write_bytes(b"[aarch64] EC=");
    super::serial::write_hex_raw(esr >> 26);
    super::serial::write_bytes(b" ISS=");
    super::serial::write_hex_raw(esr & 0x01ff_ffff);
    super::serial::write_bytes(b" CurrentEL=");
    super::serial::write_hex_raw(current_el);
    super::serial::write_bytes(b" SPSR_EL1=");
    super::serial::write_hex_raw(spsr);
    super::serial::write_bytes_raw(b"\r\n");
    super::serial::write_bytes(b"[aarch64] SP_EL0=");
    super::serial::write_hex_raw(sp_el0);
    super::serial::write_bytes(b" TTBR0_EL1=");
    super::serial::write_hex_raw(ttbr0);
    super::serial::write_bytes_raw(b"\r\n");
    super::serial::write_bytes(b"[aarch64] SCTLR_EL1=");
    super::serial::write_hex_raw(sctlr);
    super::serial::write_bytes(b" CPACR_EL1=");
    super::serial::write_hex_raw(cpacr);
    super::serial::write_bytes_raw(b"\r\n");
    if elr != 0 {
        let instruction = unsafe { core::ptr::read_volatile(elr as *const u32) };
        super::serial::write_bytes(b"[aarch64] opcode=");
        super::serial::write_hex_raw(instruction as u64);
        super::serial::write_bytes_raw(b"\r\n");
    }
    super::halt()
}

global_asm!(
    ".section .text.exceptions",
    ".balign 0x800",
    ".global exception_vector_table",
    "exception_vector_table:",
    ".balign 0x80", "b aarch64_current_sync",
    ".balign 0x80", "b aarch64_common_irq",
    ".balign 0x80", "b .",
    ".balign 0x80", "b .",
    ".balign 0x80", "b aarch64_current_sync",
    ".balign 0x80", "b aarch64_common_irq",
    ".balign 0x80", "b .",
    ".balign 0x80", "b .",
    ".balign 0x80", "b aarch64_el0_sync",
    ".balign 0x80", "b aarch64_common_irq",
    ".balign 0x80", "b .",
    ".balign 0x80", "b .",
    ".balign 0x80", "b aarch64_unhandled_sync",
    ".balign 0x80", "b aarch64_common_irq",
    ".balign 0x80", "b .",
    ".balign 0x80", "b .",

    // Preserve every general-purpose register plus the architectural return
    // state. ELR_EL1/SPSR_EL1 must live on the task's own stack because a
    // timer-driven context switch may suspend this handler while another task
    // receives exceptions and overwrites the system registers.
    ".macro SAVE_GPRS",
    "sub sp, sp, #272",
    "stp x0, x1, [sp, #0]", "stp x2, x3, [sp, #16]",
    "stp x4, x5, [sp, #32]", "stp x6, x7, [sp, #48]",
    "stp x8, x9, [sp, #64]", "stp x10, x11, [sp, #80]",
    "stp x12, x13, [sp, #96]", "stp x14, x15, [sp, #112]",
    "stp x16, x17, [sp, #128]", "stp x18, x19, [sp, #144]",
    "stp x20, x21, [sp, #160]", "stp x22, x23, [sp, #176]",
    "stp x24, x25, [sp, #192]", "stp x26, x27, [sp, #208]",
    "stp x28, x29, [sp, #224]", "str x30, [sp, #240]",
    "mrs x9, elr_el1", "str x9, [sp, #248]",
    "mrs x9, spsr_el1", "str x9, [sp, #256]",
    "mrs x9, sp_el0", "str x9, [sp, #264]",
    ".endm",
    ".macro RESTORE_GPRS",
    "ldr x9, [sp, #248]", "msr elr_el1, x9",
    "ldr x9, [sp, #256]", "msr spsr_el1, x9",
    "ldr x9, [sp, #264]", "msr sp_el0, x9",
    "ldp x0, x1, [sp, #0]", "ldp x2, x3, [sp, #16]",
    "ldp x4, x5, [sp, #32]", "ldp x6, x7, [sp, #48]",
    "ldp x8, x9, [sp, #64]", "ldp x10, x11, [sp, #80]",
    "ldp x12, x13, [sp, #96]", "ldp x14, x15, [sp, #112]",
    "ldp x16, x17, [sp, #128]", "ldp x18, x19, [sp, #144]",
    "ldp x20, x21, [sp, #160]", "ldp x22, x23, [sp, #176]",
    "ldp x24, x25, [sp, #192]", "ldp x26, x27, [sp, #208]",
    "ldp x28, x29, [sp, #224]", "ldr x30, [sp, #240]",
    "add sp, sp, #272",
    ".endm",

    "aarch64_el0_sync:",
    "SAVE_GPRS",
    "mrs x9, esr_el1",
    "lsr x10, x9, #26",
    "cmp x10, #0x15",
    "b.ne 1f",
    "ldr x0, [sp, #64]",
    "ldp x1, x2, [sp, #0]",
    "ldp x3, x4, [sp, #16]",
    "ldp x5, x6, [sp, #32]",
    "bl aarch64_syscall_handler",
    "str x0, [sp, #0]",
    "RESTORE_GPRS",
    "eret",
    "1:",
    "cmp x10, #0x24",
    "b.ne 2f",
    "mov x0, x9",
    "mrs x1, far_el1",
    "bl aarch64_el0_data_abort",
    "cbz x0, 2f",
    "RESTORE_GPRS",
    "eret",
    "2:",
    "mrs x0, esr_el1",
    "mrs x1, far_el1",
    "mrs x2, elr_el1",
    "bl aarch64_sync_exception",
    "b .",

    // Data faults can occur at EL1 while a syscall is copying a valid lazy
    // user VMA (for example stack growth). Resolve those against the current
    // process and retry the exact interrupted kernel instruction.
    "aarch64_current_sync:",
    "SAVE_GPRS",
    "mrs x9, esr_el1",
    "lsr x10, x9, #26",
    "cmp x10, #0x25",
    "b.ne 3f",
    "mov x0, x9",
    "mrs x1, far_el1",
    "bl aarch64_el0_data_abort",
    "cbz x0, 3f",
    "RESTORE_GPRS",
    "eret",
    "3:",
    "mrs x0, esr_el1",
    "mrs x1, far_el1",
    "mrs x2, elr_el1",
    "bl aarch64_sync_exception",
    "b .",

    "aarch64_unhandled_sync:",
    "mrs x0, esr_el1",
    "mrs x1, far_el1",
    "mrs x2, elr_el1",
    "bl aarch64_sync_exception",
    "b .",

    "aarch64_common_irq:",
    "SAVE_GPRS",
    "bl aarch64_irq_handler",
    "RESTORE_GPRS",
    "eret",
);
