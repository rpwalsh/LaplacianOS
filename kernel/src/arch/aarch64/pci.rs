//! PCI Express ECAM configuration access for firmware-described ARM systems.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciLocation {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciDeviceInfo {
    pub location: PciLocation,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub irq_line: u8,
}

const PCI_COMMAND_OFFSET: u8 = 0x04;
const PCI_CLASS_OFFSET: u8 = 0x08;
const PCI_HEADER_TYPE_OFFSET: u8 = 0x0e;
const PCI_INTERRUPT_LINE_OFFSET: u8 = 0x3c;
const PCI_COMMAND_IO_SPACE: u16 = 1 << 0;
const PCI_COMMAND_MEMORY_SPACE: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;

fn config_address(bus: u8, slot: u8, func: u8, offset: u8) -> Option<*mut u32> {
    if slot >= 32 || func >= 8 {
        return None;
    }
    let ecam = super::fdt::pcie_ecam()?;
    let relative = ((bus as u64) << 20)
        | ((slot as u64) << 15)
        | ((func as u64) << 12)
        | (offset as u64 & 0xfc);
    if relative.checked_add(4)? > ecam.size {
        return None;
    }
    let address = ecam.base.checked_add(relative)?;
    if !crate::mm::page_table::ensure_identity_mapped_2m(address) {
        return None;
    }
    Some(address as *mut u32)
}

fn probe_device(bus: u8, slot: u8, func: u8) -> Option<PciDeviceInfo> {
    let vendor_id = read_u16(bus, slot, func, 0);
    if vendor_id == 0xffff {
        return None;
    }
    let class = read_u32(bus, slot, func, PCI_CLASS_OFFSET);
    Some(PciDeviceInfo {
        location: PciLocation { bus, slot, func },
        vendor_id,
        device_id: read_u16(bus, slot, func, 2),
        class_code: (class >> 24) as u8,
        subclass: (class >> 16) as u8,
        prog_if: (class >> 8) as u8,
        irq_line: read_u8(bus, slot, func, PCI_INTERRUPT_LINE_OFFSET),
    })
}

pub fn for_each_device<F: FnMut(PciDeviceInfo)>(mut visit: F) {
    let Some(ecam) = super::fdt::pcie_ecam() else {
        return;
    };
    let buses = (ecam.size >> 20).min(256) as u16;
    for bus in 0..buses {
        for slot in 0..32u8 {
            if read_u16(bus as u8, slot, 0, 0) == 0xffff {
                continue;
            }
            let functions = if read_u8(bus as u8, slot, 0, PCI_HEADER_TYPE_OFFSET) & 0x80 != 0 {
                8
            } else {
                1
            };
            for func in 0..functions {
                if let Some(device) = probe_device(bus as u8, slot, func) {
                    visit(device);
                }
            }
        }
    }
}

pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDeviceInfo> {
    let mut result = None;
    for_each_device(|device| {
        if result.is_none() && device.vendor_id == vendor_id && device.device_id == device_id {
            result = Some(device);
        }
    });
    result
}

pub fn enable_bus_master(location: PciLocation) {
    let command = read_u16(location.bus, location.slot, location.func, PCI_COMMAND_OFFSET)
        | PCI_COMMAND_IO_SPACE
        | PCI_COMMAND_MEMORY_SPACE
        | PCI_COMMAND_BUS_MASTER;
    write_u16(
        location.bus,
        location.slot,
        location.func,
        PCI_COMMAND_OFFSET,
        command,
    );
}

pub fn read_u8(bus: u8, slot: u8, func: u8, offset: u8) -> u8 {
    let word = read_u32(bus, slot, func, offset);
    (word >> ((offset & 3) * 8)) as u8
}

pub fn read_u16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
    let word = read_u32(bus, slot, func, offset);
    (word >> ((offset & 2) * 8)) as u16
}

pub fn read_u32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    let Some(address) = config_address(bus, slot, func, offset) else {
        return u32::MAX;
    };
    unsafe { core::ptr::read_volatile(address) }
}

pub fn write_u16(bus: u8, slot: u8, func: u8, offset: u8, value: u16) {
    let Some(address) = config_address(bus, slot, func, offset) else {
        return;
    };
    unsafe {
        let shift = (offset & 2) * 8;
        let current = core::ptr::read_volatile(address);
        let updated = (current & !(0xffff << shift)) | ((value as u32) << shift);
        core::ptr::write_volatile(address, updated);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }
}

pub fn log_scan() {
    let mut count = 0u64;
    for_each_device(|device| {
        count += 1;
        crate::arch::serial::write_bytes(b"[pcie] bus=");
        crate::arch::serial::write_u64_dec_inline(device.location.bus as u64);
        crate::arch::serial::write_bytes(b" slot=");
        crate::arch::serial::write_u64_dec_inline(device.location.slot as u64);
        crate::arch::serial::write_bytes(b" vid=");
        crate::arch::serial::write_hex_inline(device.vendor_id as u64);
        crate::arch::serial::write_bytes(b" did=");
        crate::arch::serial::write_hex(device.device_id as u64);
    });
    crate::arch::serial::write_bytes(b"[pcie] scan complete devices=");
    crate::arch::serial::write_u64_dec(count);
}
