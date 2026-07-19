//! Low-level kernel task context switching for the supported ISAs.

use super::context::CpuContext;

#[cfg(target_arch = "x86_64")]
pub const fn kernel_task_start_addr(entry: u64) -> u64 {
    entry
}

#[cfg(target_arch = "aarch64")]
pub fn kernel_task_start_addr(_entry: u64) -> u64 {
    unsafe extern "C" {
        static aarch64_kernel_task_start: u8;
    }
    core::ptr::addr_of!(aarch64_kernel_task_start) as u64
}

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text",
    ".balign 16",
    ".global aarch64_kernel_task_start",
    "aarch64_kernel_task_start:",
    "msr daifclr, #2",
    "isb",
    // x19 is initialized with the Rust task entry and x30 with the return
    // trampoline. `br` preserves x30 so a normal Rust return terminates the
    // task through the scheduler.
    "br x19",
);

#[cfg(target_arch = "x86_64")]
#[inline(never)]
pub unsafe fn switch_context(old: *mut CpuContext, new: *const CpuContext) {
    unsafe {
        core::arch::asm!(
            "mov [rdi + 0],  rbx", "mov [rdi + 8],  rbp",
            "mov [rdi + 16], r12", "mov [rdi + 24], r13",
            "mov [rdi + 32], r14", "mov [rdi + 40], r15",
            "lea rax, [rip + 22f]", "mov [rdi + 48], rax",
            "mov [rdi + 56], rsp", "pushfq", "pop rax",
            "mov [rdi + 64], rax",
            "mov rbx, [rsi + 0]", "mov rbp, [rsi + 8]",
            "mov r12, [rsi + 16]", "mov r13, [rsi + 24]",
            "mov r14, [rsi + 32]", "mov r15, [rsi + 40]",
            "mov rsp, [rsi + 56]", "mov rax, [rsi + 64]",
            "push rax", "popfq", "mov rax, [rsi + 48]",
            "push rax", "ret", "22:",
            out("rax") _, in("rdi") old, in("rsi") new,
            clobber_abi("C"),
        );
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
pub unsafe fn switch_context(old: *mut CpuContext, new: *const CpuContext) {
    // CpuContext offsets: x19..x29 = 0..80, x30=88, pc=96, sp=104,
    // daif=112.
    unsafe {
        core::arch::asm!(
            "stp x19, x20, [x0, #0]", "stp x21, x22, [x0, #16]",
            "stp x23, x24, [x0, #32]", "stp x25, x26, [x0, #48]",
            "stp x27, x28, [x0, #64]", "str x29, [x0, #80]",
            "str x30, [x0, #88]",
            "adr x9, 22f", "str x9, [x0, #96]",
            "mov x9, sp", "str x9, [x0, #104]",
            "mrs x9, daif", "str x9, [x0, #112]",
            "ldp x19, x20, [x1, #0]", "ldp x21, x22, [x1, #16]",
            "ldp x23, x24, [x1, #32]", "ldp x25, x26, [x1, #48]",
            "ldp x27, x28, [x1, #64]", "ldr x29, [x1, #80]",
            "ldr x30, [x1, #88]",
            "ldr x9, [x1, #104]", "mov sp, x9",
            "ldr x9, [x1, #96]", "br x9",
            "22:",
            in("x0") old,
            in("x1") new,
            out("x9") _,
            clobber_abi("C"),
        );
    }
}
