//! AArch64 kernel translation-table construction and address-space switching.
//!
//! The direct-kernel boot path starts with the MMU disabled.  This backend
//! builds a four-level, 4 KiB-granule identity map, assigns device and normal
//! memory attributes, validates the descriptors, enables the MMU, and then
//! applies page-granular W^X permissions to the kernel image.

use crate::arch::serial;
use crate::mm::{frame_alloc, reserved};

const PAGE_SIZE: u64 = 4096;
const BLOCK_2M: u64 = 2 * 1024 * 1024;
const BLOCK_1G: u64 = 1024 * 1024 * 1024;
const MAX_PT_FRAMES: usize = 128;
const ADDRESS_MASK: u64 = 0x0000_FFFF_FFFF_F000;

const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE_OR_PAGE: u64 = 1 << 1;
const ATTR_INDEX_NORMAL: u64 = 0 << 2;
const ATTR_INDEX_DEVICE: u64 = 1 << 2;
const AP_KERNEL_RO: u64 = 2 << 6;
const SH_INNER: u64 = 3 << 8;
const ACCESS_FLAG: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

const TABLE_DESC: u64 = DESC_VALID | DESC_TABLE_OR_PAGE;
const NORMAL_BLOCK: u64 = DESC_VALID | ATTR_INDEX_NORMAL | SH_INNER | ACCESS_FLAG;
const DEVICE_BLOCK: u64 = DESC_VALID | ATTR_INDEX_DEVICE | ACCESS_FLAG | PXN | UXN;
const NORMAL_PAGE: u64 =
    DESC_VALID | DESC_TABLE_OR_PAGE | ATTR_INDEX_NORMAL | SH_INNER | ACCESS_FLAG;

static mut PT_FRAMES: [u64; MAX_PT_FRAMES] = [0; MAX_PT_FRAMES];
static mut PT_FRAME_COUNT: usize = 0;
static mut ACTIVE_ROOT: u64 = 0;
static mut CONFIGURED_TCR: u64 = 0;

pub struct BootstrapResult {
    /// Kept as `pml4_phys` for the architecture-neutral scheduler ABI.  On
    /// AArch64 this is the level-zero translation table installed in TTBR0_EL1.
    pub pml4_phys: u64,
    pub frames_used: usize,
    pub mapped_bytes: u64,
}

#[inline]
const fn l0_index(addr: u64) -> usize {
    ((addr >> 39) & 0x1ff) as usize
}
#[inline]
const fn l1_index(addr: u64) -> usize {
    ((addr >> 30) & 0x1ff) as usize
}
#[inline]
const fn l2_index(addr: u64) -> usize {
    ((addr >> 21) & 0x1ff) as usize
}
#[inline]
const fn l3_index(addr: u64) -> usize {
    ((addr >> 12) & 0x1ff) as usize
}

fn alloc_pt_frame() -> Option<u64> {
    let frame = frame_alloc::alloc_frame()?;
    unsafe {
        core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE as usize);
        if PT_FRAME_COUNT >= MAX_PT_FRAMES {
            return None;
        }
        PT_FRAMES[PT_FRAME_COUNT] = frame;
        PT_FRAME_COUNT += 1;
    }
    Some(frame)
}

/// Program MAIR_EL1 and TCR_EL1 for 48-bit, 4 KiB-granule translation.
pub unsafe fn enable_nxe() {
    let mmfr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0, options(nomem, nostack));
    }
    let ips = mmfr0 & 0xf;
    // Attr0: normal WB/WA cacheable. Attr1: device-nGnRE.
    let mair = 0x04ffu64;
    // T0SZ=16 (48-bit VA), WB/WA walks, inner-shareable, 4 KiB TG0,
    // EPD1 disables TTBR1 walks until a higher-half map is introduced.
    let tcr = 16u64
        | (1 << 8)
        | (1 << 10)
        | (3 << 12)
        | (1 << 23)
        | (ips << 32);
    unsafe {
        core::arch::asm!(
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "isb",
            mair = in(reg) mair,
            tcr = in(reg) tcr,
            options(nostack)
        );
        CONFIGURED_TCR = tcr;
    }
    serial::write_line(b"[page_table] AArch64 MAIR/TCR configured");
}

/// Construct an identity map using level-one 1 GiB blocks.  The QEMU `virt`
/// RAM aperture begins at 0x4000_0000, so lower blocks are device memory and
/// RAM blocks receive normal cacheable attributes.
pub unsafe fn build_identity_map(limit_bytes: u64) -> Option<BootstrapResult> {
    if limit_bytes == 0 || limit_bytes > (1u64 << 48) {
        return None;
    }
    unsafe {
        PT_FRAME_COUNT = 0;
    }
    let root = alloc_pt_frame()?;
    let l1 = alloc_pt_frame()?;
    unsafe {
        (root as *mut u64).add(l0_index(0)).write(l1 | TABLE_DESC);
    }

    let mapped = limit_bytes.saturating_add(BLOCK_1G - 1) & !(BLOCK_1G - 1);
    let blocks = (mapped / BLOCK_1G) as usize;
    if blocks > 512 {
        return None;
    }
    for index in 0..blocks {
        let phys = index as u64 * BLOCK_1G;
        let attrs = if phys < 0x4000_0000 {
            DEVICE_BLOCK
        } else {
            NORMAL_BLOCK
        };
        unsafe {
            (l1 as *mut u64).add(index).write(phys | attrs);
        }
    }

    serial::write_bytes(b"[page_table] AArch64 identity root=");
    serial::write_hex(root);
    Some(BootstrapResult {
        pml4_phys: root,
        frames_used: unsafe { PT_FRAME_COUNT },
        mapped_bytes: mapped,
    })
}

pub unsafe fn validate(result: &BootstrapResult) -> usize {
    if result.pml4_phys & (PAGE_SIZE - 1) != 0 || result.mapped_bytes == 0 {
        return 1;
    }
    let root_entry = unsafe { (result.pml4_phys as *const u64).add(l0_index(0)).read() };
    if root_entry & TABLE_DESC != TABLE_DESC {
        return 1;
    }
    let l1 = (root_entry & ADDRESS_MASK) as *const u64;
    let blocks = (result.mapped_bytes / BLOCK_1G) as usize;
    let mut errors = 0;
    for index in 0..blocks {
        let entry = unsafe { l1.add(index).read() };
        if entry & DESC_VALID == 0 || entry & 0x0000_FFFF_C000_0000 != index as u64 * BLOCK_1G {
            errors += 1;
        }
    }
    errors
}

pub unsafe fn activate(result: &BootstrapResult) {
    assert_ne!(unsafe { CONFIGURED_TCR }, 0, "TCR must be configured before MMU enable");
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "msr ttbr0_el1, {root}",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            "mrs x9, sctlr_el1",
            // M, C, SA and I. WXN is enabled only after the coarse kernel
            // block has been split into page-granular RX/R/RW mappings.
            "orr x9, x9, #(1 << 0)",
            "orr x9, x9, #(1 << 2)",
            "orr x9, x9, #(1 << 3)",
            "orr x9, x9, #(1 << 12)",
            "msr sctlr_el1, x9",
            "isb",
            root = in(reg) result.pml4_phys,
            out("x9") _,
            options(nostack)
        );
        ACTIVE_ROOT = result.pml4_phys;
    }
    serial::write_line(b"[page_table] AArch64 MMU enabled");
}

pub unsafe fn load_address_space(root: u64) {
    debug_assert_eq!(root & (PAGE_SIZE - 1), 0);
    unsafe {
        core::arch::asm!(
            // Every address space currently uses ASID 0.  Unlike x86 CR3,
            // writing TTBR0_EL1 does not implicitly invalidate translations;
            // retaining them would let the next process execute or access the
            // previous process's pages at an identical virtual address.
            "dsb ishst",
            "msr ttbr0_el1, {}",
            "isb",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            in(reg) root,
            options(nostack)
        );
    }
}

pub fn current_pml4() -> u64 {
    let root: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) root, options(nomem, nostack));
    }
    root & ADDRESS_MASK
}

pub unsafe fn flush_page(vaddr: u64) {
    let operand = vaddr >> 12;
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vae1is, {}",
            "dsb ish",
            "isb",
            in(reg) operand,
            options(nostack)
        );
    }
}

pub fn active_pml4() -> u64 {
    unsafe { ACTIVE_ROOT }
}

pub fn with_kernel_address_space<R>(f: impl FnOnce() -> R) -> R {
    let kernel = active_pml4();
    let current = current_pml4();
    if kernel == 0 || kernel == current {
        return f();
    }
    unsafe { load_address_space(kernel) };
    let result = f();
    unsafe { load_address_space(current) };
    result
}

fn ensure_l2_for(addr: u64) -> Option<u64> {
    let root = active_pml4();
    if root == 0 {
        return None;
    }
    let l0e = unsafe { (root as *mut u64).add(l0_index(addr)).read() };
    if l0e & TABLE_DESC != TABLE_DESC {
        return None;
    }
    let l1 = (l0e & ADDRESS_MASK) as *mut u64;
    let slot = unsafe { l1.add(l1_index(addr)) };
    let current = unsafe { slot.read() };
    if current & DESC_VALID != 0 && current & DESC_TABLE_OR_PAGE != 0 {
        return Some(current & ADDRESS_MASK);
    }

    let l2 = alloc_pt_frame()?;
    // Preserve the complete old 1 GiB block as 512 level-two blocks before
    // replacing it, so dynamically mapping one MMIO window cannot punch holes.
    let block_base = addr & !(BLOCK_1G - 1);
    let old_attrs = current & !0x0000_FFFF_C000_0000;
    for index in 0..512usize {
        unsafe {
            (l2 as *mut u64)
                .add(index)
                .write(block_base + index as u64 * BLOCK_2M | old_attrs);
        }
    }
    unsafe { slot.write(l2 | TABLE_DESC) };
    Some(l2)
}

pub fn ensure_identity_mapped_2m(phys_addr: u64) -> bool {
    let base = phys_addr & !(BLOCK_2M - 1);
    let Some(l2) = ensure_l2_for(base) else {
        return false;
    };
    let slot = unsafe { (l2 as *mut u64).add(l2_index(base)) };
    let current = unsafe { slot.read() };
    if current & DESC_VALID != 0 {
        return current & 0x0000_FFFF_FFE0_0000 == base;
    }
    unsafe {
        slot.write(base | DEVICE_BLOCK);
        flush_page(base);
    }
    true
}

pub unsafe fn register_pt_frames_reserved() {
    for index in 0..unsafe { PT_FRAME_COUNT } {
        let frame = unsafe { PT_FRAMES[index] };
        unsafe { reserved::add_post_init(frame, frame + PAGE_SIZE, b"aarch64 page-table frame") };
    }
}

pub fn pt_frame_count() -> usize {
    unsafe { PT_FRAME_COUNT }
}

unsafe extern "C" {
    static __kernel_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __kernel_end: u8;
}

fn ensure_l3_for(addr: u64) -> Option<u64> {
    let l2 = ensure_l2_for(addr)?;
    let slot = unsafe { (l2 as *mut u64).add(l2_index(addr)) };
    let current = unsafe { slot.read() };
    if current & DESC_VALID != 0 && current & DESC_TABLE_OR_PAGE != 0 {
        return Some(current & ADDRESS_MASK);
    }
    let l3 = alloc_pt_frame()?;
    let block_base = addr & !(BLOCK_2M - 1);
    let old_attrs = current & !0x0000_FFFF_FFE0_0000;
    for index in 0..512usize {
        unsafe {
            (l3 as *mut u64)
                .add(index)
                .write(block_base + index as u64 * PAGE_SIZE | old_attrs | DESC_TABLE_OR_PAGE);
        }
    }
    unsafe { slot.write(l3 | TABLE_DESC) };
    Some(l3)
}

/// Apply RX/R/RW permissions to `.text`, `.rodata`, and writable kernel data.
pub unsafe fn remap_kernel_sections() {
    let start = core::ptr::addr_of!(__kernel_start) as u64;
    let text_end = core::ptr::addr_of!(__text_end) as u64;
    let ro_start = core::ptr::addr_of!(__rodata_start) as u64;
    let ro_end = core::ptr::addr_of!(__rodata_end) as u64;
    let end = core::ptr::addr_of!(__kernel_end) as u64;
    let mut page = start & !(PAGE_SIZE - 1);
    while page < (end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1) {
        let Some(l3) = ensure_l3_for(page) else {
            panic!("failed to allocate AArch64 kernel permission table");
        };
        let attrs = if page < text_end {
            NORMAL_PAGE | AP_KERNEL_RO | UXN
        } else if page >= ro_start && page < ro_end {
            NORMAL_PAGE | AP_KERNEL_RO | PXN | UXN
        } else {
            NORMAL_PAGE | PXN | UXN
        };
        unsafe {
            (l3 as *mut u64).add(l3_index(page)).write(page | attrs);
            flush_page(page);
        }
        page += PAGE_SIZE;
    }
    unsafe {
        core::arch::asm!(
            "dsb ish",
            "isb",
            "mrs x9, sctlr_el1",
            "orr x9, x9, #(1 << 19)",
            "msr sctlr_el1, x9",
            "isb",
            out("x9") _,
            options(nostack)
        );
    }
    serial::write_line(b"[page_table] AArch64 kernel W^X applied");
}
