//! AArch64 EL1 boot entry.
//!
//! This module provides the bare-metal `_start` routine that is the
//! entry point when the kernel is loaded on an AArch64 platform.
//!
//! # Boot sequence
//! 1. Determine current EL.  If running at EL2 drop to EL1 (EL3 is not
//!    supported in this initial revision — firmware must have already
//!    handled EL3 → EL2 transition).
//! 2. Set up `SP_EL1` to point at the pre-linked boot stack symbol
//!    `__boot_stack_top`.
//! 3. Zero the BSS section.
//! 4. Install the exception vector table (`VBAR_EL1`).
//! 5. Enable the virtual timers and MMU identity map for the first 2 GiB.
//! 6. Call `crate::arch::aarch64::init()` and then `crate::kmain()`.

use core::arch::global_asm;

/// Captured before changing exception level or installing a stack.  Keeping
/// this word in the file-backed data segment lets us distinguish a firmware
/// handoff failure from corruption introduced by the Rust trampoline.
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.boot")]
static mut AARCH64_FIRMWARE_DTB: u64 = u64::MAX;

unsafe extern "C" {
    /// Top of the boot stack (defined in `linker.ld` or by the build script).
    static __boot_stack_top: u8;
    /// Start of the BSS section.
    static mut __bss_start: u8;
    /// End of the BSS section (exclusive).
    static __bss_end: u8;
}

// ---------------------------------------------------------------------------
// Rust boot trampoline — called from assembly _start after stack is ready.
// ---------------------------------------------------------------------------

/// Called by the assembly `_start` stub once EL1 stack is live.
///
/// # Safety
/// May only be called once, from the assembly entry stub, with a valid SP.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aarch64_boot_rust() -> ! {
    // Zero BSS.
    let bss_start = &raw mut __bss_start;
    let bss_end = &raw const __bss_end as usize;
    let bss_len = bss_end.saturating_sub(bss_start as usize);
    unsafe {
        core::ptr::write_bytes(bss_start, 0u8, bss_len);
    }

    // Install exception vector table.
    unsafe {
        core::arch::asm!(
            "adrp {t}, exception_vector_table",
            "add  {t}, {t}, :lo12:exception_vector_table",
            "msr  vbar_el1, {t}",
            "isb",
            t = out(reg) _,
            options(nomem, nostack),
        );
    }

    // Hand off to the architecture-common init path.
    let dtb = unsafe { core::ptr::read_volatile(&raw const AARCH64_FIRMWARE_DTB) };
    crate::arch::aarch64::init(dtb);
    crate::kmain_aarch64()
}

// ---------------------------------------------------------------------------
// Assembly entry point
// ---------------------------------------------------------------------------

global_asm!(
    ".section .text.boot",
    ".global _start",
    "_start:",
    // Linux-compatible arm64 Image header.  Direct-boot firmware and QEMU use
    // this header to load the raw image at RAM + 512 KiB and place the DTB
    // address in x0.  Both words at the front are executable so the same
    // artifact remains valid when entered directly by a simple loader.
    "nop",
    "b .Lfirmware_entry",
    ".quad 0x0000000000080000", // text_offset
    ".quad 0",                  // image_size: legacy-compatible, infer from file
    ".quad 0",                  // little-endian, 4 KiB pages, low placement
    ".quad 0",
    ".quad 0",
    ".quad 0",
    ".long 0x644d5241",         // ARM64 Image magic
    ".long 0",
    ".Lfirmware_entry:",
    // QEMU/firmware passes the flattened device-tree address in x0.
    "adrp x1, AARCH64_FIRMWARE_DTB",
    "add  x1, x1, :lo12:AARCH64_FIRMWARE_DTB",
    "str  x0, [x1]",
    // Determine current EL.  We must be at EL2 or EL1.
    "mrs  x0, currentel",
    "lsr  x0, x0, #2",
    "cmp  x0, #2",
    "b.eq .Ldrop_to_el1",
    // Already in EL1 — just set up SP and continue.
    ".Lat_el1:",
    // Rust/LLVM may use NEON for fixed-size copies even when source code does
    // not explicitly request floating point. Enable FP/SIMD at EL1 and EL0.
    "mrs  x0, cpacr_el1",
    "orr  x0, x0, #(3 << 20)",
    "msr  cpacr_el1, x0",
    "isb",
    "adrp x30, __boot_stack_top",
    "add  sp,  x30, :lo12:__boot_stack_top",
    "b    .Lstart_rust",
    // At EL2 — configure minimal HCR_EL2 and drop to EL1.
    ".Ldrop_to_el1:",
    // HCR_EL2: RW=1 (EL1 is AArch64), no virtualisation features.
    "mov  x0, #(1 << 31)",
    "msr  hcr_el2, x0",
    // Do not trap FP/SIMD accesses from EL1 into EL2.
    "msr  cptr_el2, xzr",
    // SCTLR_EL1: default (MMU off, caches off).
    "msr  sctlr_el1, xzr",
    // SPSR_EL2: EL1h (SPSel=1), all interrupts masked.
    "mov  x0, #0x3c5",
    "msr  spsr_el2, x0",
    // ELR_EL2: return to .Lat_el1.
    "adr  x0, .Lat_el1",
    "msr  elr_el2, x0",
    "eret",
    // Set up SP and call Rust boot trampoline.
    ".Lstart_rust:",
    "bl   aarch64_boot_rust",
    // Should never return; spin if it does.
    "b .",
);
