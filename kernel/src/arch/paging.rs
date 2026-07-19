//! Portable page-table vocabulary for per-process address spaces.
//!
//! VMAs store architecture-neutral permission bits.  `encode_leaf` converts
//! those permissions into the actual hardware descriptor, while preserving
//! software permission bits for fast syscall pointer validation.

pub const PAGE_SIZE_4K: u64 = 4096;
pub const PAGE_SIZE_2M: u64 = 2 * 1024 * 1024;
pub const FRAME_MASK: u64 = 0x0000_FFFF_FFFF_F000;

#[inline]
pub const fn pml4_index(vaddr: u64) -> usize {
    ((vaddr >> 39) & 0x1ff) as usize
}
#[inline]
pub const fn pdpt_index(vaddr: u64) -> usize {
    ((vaddr >> 30) & 0x1ff) as usize
}
#[inline]
pub const fn pd_index(vaddr: u64) -> usize {
    ((vaddr >> 21) & 0x1ff) as usize
}
#[inline]
pub const fn pt_index(vaddr: u64) -> usize {
    ((vaddr >> 12) & 0x1ff) as usize
}

#[cfg(target_arch = "x86_64")]
pub mod flags {
    pub use crate::arch::x86_64::paging::flags::*;
}

#[cfg(target_arch = "aarch64")]
pub mod flags {
    pub const PRESENT: u64 = 1 << 0;
    // Ignored-by-hardware bits retained in every process descriptor so generic
    // VM policy and syscall validation can inspect the original permissions.
    pub const WRITABLE: u64 = 1 << 55;
    pub const USER: u64 = 1 << 56;
    pub const HUGE_PAGE: u64 = 1 << 58;
    // UXN is also the portable no-execute marker.
    pub const NO_EXECUTE: u64 = 1 << 54;
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub const fn encode_table(phys: u64, permissions: u64) -> u64 {
    phys | permissions
}

#[cfg(target_arch = "aarch64")]
#[inline]
pub const fn encode_table(phys: u64, permissions: u64) -> u64 {
    // Valid table descriptor; APTable defaults permit lower-level EL0 access.
    phys | 0b11 | (permissions & (flags::USER | flags::WRITABLE))
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub const fn encode_leaf(phys: u64, permissions: u64) -> u64 {
    phys | permissions
}

#[cfg(target_arch = "aarch64")]
#[inline]
pub const fn encode_leaf(phys: u64, permissions: u64) -> u64 {
    const PAGE: u64 = 0b11;
    const SH_INNER: u64 = 3 << 8;
    const AF: u64 = 1 << 10;
    const AP_EL0_RW: u64 = 1 << 6;
    const AP_EL0_RO: u64 = 3 << 6;
    const AP_EL1_RO: u64 = 2 << 6;
    const PXN: u64 = 1 << 53;

    let user = permissions & flags::USER != 0;
    let writable = permissions & flags::WRITABLE != 0;
    let ap = match (user, writable) {
        (true, true) => AP_EL0_RW,
        (true, false) => AP_EL0_RO,
        (false, false) => AP_EL1_RO,
        (false, true) => 0,
    };
    let execute = if user {
        PXN | (permissions & flags::NO_EXECUTE)
    } else if permissions & flags::NO_EXECUTE != 0 {
        PXN | flags::NO_EXECUTE
    } else {
        flags::NO_EXECUTE
    };
    phys
        | PAGE
        | SH_INNER
        | AF
        | ap
        | execute
        | (permissions & (flags::USER | flags::WRITABLE))
}

#[inline]
pub const fn entry_address(entry: u64) -> u64 {
    entry & FRAME_MASK
}
