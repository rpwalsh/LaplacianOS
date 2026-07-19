//! Read-only flattened-device-tree discovery for AArch64 boot.
//!
//! Firmware passes the DTB physical address in `x0`.  LaplacianOS validates the
//! complete envelope before walking it and records the platform resources
//! required before the heap and general VFS exist.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;
const MAX_DTB_BYTES: usize = 16 * 1024 * 1024;
const MAX_VIRTIO_MMIO: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub base: u64,
    pub size: u64,
}

impl Region {
    const EMPTY: Self = Self { base: 0, size: 0 };
}

static VALID: AtomicBool = AtomicBool::new(false);
static DTB_ADDRESS: AtomicU64 = AtomicU64::new(0);
static DTB_SIZE: AtomicU64 = AtomicU64::new(0);
static MEMORY_BASE: AtomicU64 = AtomicU64::new(0);
static MEMORY_SIZE: AtomicU64 = AtomicU64::new(0);
static CPU_COUNT: AtomicU32 = AtomicU32::new(0);
static VIRTIO_COUNT: AtomicU32 = AtomicU32::new(0);
static PCIE_ECAM_BASE: AtomicU64 = AtomicU64::new(0);
static PCIE_ECAM_SIZE: AtomicU64 = AtomicU64::new(0);
static mut VIRTIO_REGIONS: [Region; MAX_VIRTIO_MMIO] = [Region::EMPTY; MAX_VIRTIO_MMIO];
static mut VIRTIO_IRQS: [u32; MAX_VIRTIO_MMIO] = [u32::MAX; MAX_VIRTIO_MMIO];

#[derive(Clone, Copy)]
struct Header {
    total_size: usize,
    struct_offset: usize,
    strings_offset: usize,
    strings_size: usize,
    struct_size: usize,
}

#[inline]
fn be32(bytes: &[u8], offset: usize) -> Option<u32> {
    let data = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes(data.try_into().ok()?))
}

#[inline]
fn be64(bytes: &[u8], offset: usize) -> Option<u64> {
    let data = bytes.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_be_bytes(data.try_into().ok()?))
}

#[inline]
fn align4(value: usize) -> Option<usize> {
    value.checked_add(3).map(|v| v & !3)
}

fn header(bytes: &[u8]) -> Option<Header> {
    if be32(bytes, 0)? != FDT_MAGIC {
        return None;
    }
    let total_size = be32(bytes, 4)? as usize;
    let struct_offset = be32(bytes, 8)? as usize;
    let strings_offset = be32(bytes, 12)? as usize;
    let version = be32(bytes, 20)?;
    let last_compatible = be32(bytes, 24)?;
    let strings_size = be32(bytes, 32)? as usize;
    let struct_size = be32(bytes, 36)? as usize;
    if total_size < 40
        || total_size > bytes.len()
        || version < 17
        || last_compatible > version
        || struct_offset.checked_add(struct_size)? > total_size
        || strings_offset.checked_add(strings_size)? > total_size
    {
        return None;
    }
    Some(Header {
        total_size,
        struct_offset,
        strings_offset,
        strings_size,
        struct_size,
    })
}

fn cstr_at(bytes: &[u8], start: usize, limit: usize) -> Option<&[u8]> {
    if start >= limit || limit > bytes.len() {
        return None;
    }
    let rest = &bytes[start..limit];
    let end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[..end])
}

fn compatible_contains(value: &[u8], needle: &[u8]) -> bool {
    value
        .split(|&byte| byte == 0)
        .any(|entry| entry == needle)
}

fn decode_first_reg(value: &[u8], address_cells: u32, size_cells: u32) -> Option<Region> {
    if !(1..=2).contains(&address_cells) || !(1..=2).contains(&size_cells) {
        return None;
    }
    let cells = address_cells.checked_add(size_cells)? as usize;
    if value.len() < cells.checked_mul(4)? {
        return None;
    }
    let mut cursor = 0usize;
    let mut base = 0u64;
    for _ in 0..address_cells {
        base = (base << 32) | be32(value, cursor)? as u64;
        cursor += 4;
    }
    let mut size = 0u64;
    for _ in 0..size_cells {
        size = (size << 32) | be32(value, cursor)? as u64;
        cursor += 4;
    }
    (size != 0).then_some(Region { base, size })
}

fn record_virtio(region: Region, irq: Option<u32>) {
    let index = VIRTIO_COUNT.load(Ordering::Relaxed) as usize;
    if index >= MAX_VIRTIO_MMIO {
        return;
    }
    unsafe {
        VIRTIO_REGIONS[index] = region;
        VIRTIO_IRQS[index] = irq.unwrap_or(u32::MAX);
    }
    VIRTIO_COUNT.store((index + 1) as u32, Ordering::Release);
}

#[derive(Clone, Copy)]
struct NodeState {
    address_cells: u32,
    size_cells: u32,
    reg: Option<Region>,
    irq: Option<u32>,
    is_virtio: bool,
    is_memory: bool,
    is_cpu: bool,
    is_pcie: bool,
}

impl NodeState {
    const EMPTY: Self = Self {
        address_cells: 2,
        size_cells: 1,
        reg: None,
        irq: None,
        is_virtio: false,
        is_memory: false,
        is_cpu: false,
        is_pcie: false,
    };
}

fn decode_gic_interrupt(value: &[u8]) -> Option<u32> {
    if value.len() < 12 {
        return None;
    }
    let kind = be32(value, 0)?;
    let number = be32(value, 4)?;
    match kind {
        0 => number.checked_add(32), // SPI
        1 => number.checked_add(16), // PPI
        _ => None,
    }
}

fn discover(bytes: &[u8], hdr: Header) -> bool {
    let struct_end = match hdr.struct_offset.checked_add(hdr.struct_size) {
        Some(end) => end,
        None => return false,
    };
    let strings_end = match hdr.strings_offset.checked_add(hdr.strings_size) {
        Some(end) => end,
        None => return false,
    };
    let mut cursor = hdr.struct_offset;
    const MAX_DEPTH: usize = 64;
    let mut stack = [NodeState::EMPTY; MAX_DEPTH];
    let mut depth = 0usize;
    let mut saw_root = false;

    while cursor < struct_end {
        let token = match be32(bytes, cursor) {
            Some(token) => token,
            None => return false,
        };
        cursor += 4;
        match token {
            FDT_BEGIN_NODE => {
                let node_name = match cstr_at(bytes, cursor, struct_end) {
                    Some(name) => name,
                    None => return false,
                };
                cursor = match align4(cursor + node_name.len() + 1) {
                    Some(next) if next <= struct_end => next,
                    _ => return false,
                };
                if depth >= MAX_DEPTH {
                    return false;
                }
                let (address_cells, size_cells) = if depth == 0 {
                    (2, 1)
                } else {
                    (stack[depth - 1].address_cells, stack[depth - 1].size_cells)
                };
                stack[depth] = NodeState {
                    address_cells,
                    size_cells,
                    reg: None,
                    irq: None,
                    is_virtio: false,
                    is_memory: node_name.starts_with(b"memory@"),
                    is_cpu: node_name.starts_with(b"cpu@"),
                    is_pcie: false,
                };
                depth += 1;
                saw_root = true;
            }
            FDT_END_NODE => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
                let node = stack[depth];
                if node.is_memory {
                    if let Some(region) = node.reg {
                        MEMORY_BASE.store(region.base, Ordering::Relaxed);
                        MEMORY_SIZE.store(region.size, Ordering::Release);
                    }
                }
                if node.is_virtio {
                    if let Some(region) = node.reg {
                        record_virtio(region, node.irq);
                    }
                }
                if node.is_cpu {
                    CPU_COUNT.fetch_add(1, Ordering::Relaxed);
                }
                if node.is_pcie {
                    if let Some(region) = node.reg {
                        PCIE_ECAM_BASE.store(region.base, Ordering::Relaxed);
                        PCIE_ECAM_SIZE.store(region.size, Ordering::Release);
                    }
                }
                stack[depth] = NodeState::EMPTY;
            }
            FDT_PROP => {
                let len = match be32(bytes, cursor) {
                    Some(value) => value as usize,
                    None => return false,
                };
                let name_offset = match be32(bytes, cursor + 4) {
                    Some(value) => value as usize,
                    None => return false,
                };
                cursor += 8;
                let value_end = match cursor.checked_add(len) {
                    Some(end) if end <= struct_end => end,
                    _ => return false,
                };
                let value = &bytes[cursor..value_end];
                cursor = match align4(value_end) {
                    Some(next) if next <= struct_end => next,
                    _ => return false,
                };
                let name_start = match hdr.strings_offset.checked_add(name_offset) {
                    Some(start) if start < strings_end => start,
                    _ => return false,
                };
                let name = match cstr_at(bytes, name_start, strings_end) {
                    Some(name) => name,
                    None => return false,
                };
                if depth == 0 {
                    return false;
                }
                let node = &mut stack[depth - 1];
                if name == b"#address-cells" {
                    node.address_cells = be32(value, 0).unwrap_or(node.address_cells);
                } else if name == b"#size-cells" {
                    node.size_cells = be32(value, 0).unwrap_or(node.size_cells);
                } else if name == b"device_type" && value.starts_with(b"memory\0") {
                    node.is_memory = true;
                } else if name == b"device_type" && value.starts_with(b"cpu\0") {
                    node.is_cpu = true;
                } else if name == b"compatible"
                    && compatible_contains(value, b"virtio,mmio")
                {
                    node.is_virtio = true;
                } else if name == b"compatible"
                    && compatible_contains(value, b"pci-host-ecam-generic")
                {
                    node.is_pcie = true;
                } else if name == b"reg" {
                    let (parent_address_cells, parent_size_cells) = if depth > 1 {
                        (
                            stack[depth - 2].address_cells,
                            stack[depth - 2].size_cells,
                        )
                    } else {
                        (node.address_cells, node.size_cells)
                    };
                    stack[depth - 1].reg =
                        decode_first_reg(value, parent_address_cells, parent_size_cells);
                } else if name == b"interrupts" {
                    node.irq = decode_gic_interrupt(value);
                }
            }
            FDT_NOP => {}
            FDT_END => return saw_root && depth == 0,
            _ => return false,
        }
    }
    false
}

pub fn init(dtb_address: u64) {
    VALID.store(false, Ordering::Relaxed);
    DTB_ADDRESS.store(dtb_address, Ordering::Relaxed);
    DTB_SIZE.store(0, Ordering::Relaxed);
    MEMORY_BASE.store(0, Ordering::Relaxed);
    MEMORY_SIZE.store(0, Ordering::Relaxed);
    CPU_COUNT.store(0, Ordering::Relaxed);
    VIRTIO_COUNT.store(0, Ordering::Relaxed);
    PCIE_ECAM_BASE.store(0, Ordering::Relaxed);
    PCIE_ECAM_SIZE.store(0, Ordering::Relaxed);
    unsafe {
        VIRTIO_REGIONS = [Region::EMPTY; MAX_VIRTIO_MMIO];
        VIRTIO_IRQS = [u32::MAX; MAX_VIRTIO_MMIO];
    }
    if dtb_address == 0 || dtb_address as usize % 8 != 0 {
        return;
    }
    let prefix = unsafe { core::slice::from_raw_parts(dtb_address as *const u8, 40) };
    let total_size = match be32(prefix, 4) {
        Some(size) if (40..=MAX_DTB_BYTES as u32).contains(&size) => size as usize,
        _ => return,
    };
    let bytes = unsafe { core::slice::from_raw_parts(dtb_address as *const u8, total_size) };
    let Some(hdr) = header(bytes) else {
        return;
    };
    if hdr.total_size == total_size && discover(bytes, hdr) {
        DTB_SIZE.store(total_size as u64, Ordering::Release);
        VALID.store(true, Ordering::Release);
    }
}

pub fn is_valid() -> bool {
    VALID.load(Ordering::Acquire)
}

pub fn dtb_address() -> u64 {
    DTB_ADDRESS.load(Ordering::Relaxed)
}

pub fn dtb_size() -> u64 {
    DTB_SIZE.load(Ordering::Acquire)
}

pub fn memory() -> Option<Region> {
    let size = MEMORY_SIZE.load(Ordering::Acquire);
    (size != 0).then_some(Region {
        base: MEMORY_BASE.load(Ordering::Relaxed),
        size,
    })
}

pub fn cpu_count() -> u32 {
    CPU_COUNT.load(Ordering::Relaxed)
}

pub fn virtio_mmio_count() -> usize {
    VIRTIO_COUNT.load(Ordering::Acquire) as usize
}

pub fn virtio_mmio(index: usize) -> Option<Region> {
    if index >= virtio_mmio_count() {
        return None;
    }
    Some(unsafe { VIRTIO_REGIONS[index] })
}

pub fn virtio_mmio_irq(index: usize) -> Option<u32> {
    if index >= virtio_mmio_count() {
        return None;
    }
    let irq = unsafe { VIRTIO_IRQS[index] };
    (irq != u32::MAX).then_some(irq)
}

pub fn pcie_ecam() -> Option<Region> {
    let size = PCIE_ECAM_SIZE.load(Ordering::Acquire);
    (size != 0).then_some(Region {
        base: PCIE_ECAM_BASE.load(Ordering::Relaxed),
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_64_bit_reg_cells() {
        let bytes = [
            0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0x20, 0, 0, 0,
        ];
        assert_eq!(
            decode_first_reg(&bytes, 2, 2),
            Some(Region {
                base: 0x4000_0000,
                size: 0x2000_0000,
            })
        );
    }

    #[test]
    fn rejects_unsupported_cell_widths() {
        assert_eq!(decode_first_reg(&[0; 24], 3, 2), None);
    }
}
