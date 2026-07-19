//! VirtIO-MMIO transport discovery for AArch64.

const MAGIC: u32 = 0x7472_6976;
const DEVICE_ID_ENTROPY: u32 = 4;
const DEVICE_ID_BLOCK: u32 = 2;
const DEVICE_ID_NETWORK: u32 = 1;
const DEVICE_ID_INPUT: u32 = 18;
const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;
const STATUS_FAILED: u32 = 128;
const DESC_DEVICE_WRITES: u16 = 2;
const DESC_NEXT: u16 = 1;
const VIRTIO_F_VERSION_1_HIGH: u32 = 1;

#[repr(C, align(4096))]
struct EntropyQueue {
    descriptor: [Descriptor; 8],
    _descriptor_padding: [u8; 3968],
    avail_flags: u16,
    avail_index: u16,
    avail_ring: [u16; 8],
    used_event: u16,
    _avail_padding: [u8; 4074],
    used_flags: u16,
    used_index: u16,
    used_ring: [UsedElement; 8],
    avail_event: u16,
    _used_padding: [u8; 4026],
    bytes: [u8; 8],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Descriptor {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct UsedElement {
    id: u32,
    length: u32,
}

const EMPTY_DESCRIPTOR: Descriptor = Descriptor {
    address: 0,
    length: 0,
    flags: 0,
    next: 0,
};
const EMPTY_USED: UsedElement = UsedElement { id: 0, length: 0 };
static ENTROPY_LOCK: spin::Mutex<()> = spin::Mutex::new(());
static mut ENTROPY_QUEUE: EntropyQueue = EntropyQueue {
    descriptor: [EMPTY_DESCRIPTOR; 8],
    _descriptor_padding: [0; 3968],
    avail_flags: 0,
    avail_index: 0,
    avail_ring: [0; 8],
    used_event: 0,
    _avail_padding: [0; 4074],
    used_flags: 0,
    used_index: 0,
    used_ring: [EMPTY_USED; 8],
    avail_event: 0,
    _used_padding: [0; 4026],
    bytes: [0; 8],
};

#[inline]
unsafe fn read32(base: u64, offset: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

#[inline]
unsafe fn write32(base: u64, offset: u64, value: u32) {
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value) }
}

pub fn find_device(device_id: u32) -> Option<u64> {
    for index in 0..super::fdt::virtio_mmio_count() {
        let region = super::fdt::virtio_mmio(index)?;
        let magic = unsafe { read32(region.base, 0x000) };
        let discovered_id = unsafe { read32(region.base, 0x008) };
        if magic == MAGIC && discovered_id == device_id {
            return Some(region.base);
        }
    }
    None
}

pub fn log_discovery() {
    for index in 0..super::fdt::virtio_mmio_count() {
        let Some(region) = super::fdt::virtio_mmio(index) else {
            continue;
        };
        let magic = unsafe { read32(region.base, 0x000) };
        let device = unsafe { read32(region.base, 0x008) };
        if magic != MAGIC || device == 0 {
            continue;
        }
        crate::arch::serial::write_bytes(b"[virtio-mmio] base=");
        crate::arch::serial::write_hex_inline(region.base);
        crate::arch::serial::write_bytes(b" device=");
        crate::arch::serial::write_u64_dec_inline(device as u64);
        crate::arch::serial::write_bytes(b" version=");
        crate::arch::serial::write_u64_dec(unsafe { read32(region.base, 0x004) } as u64);
    }
}

pub fn random_u64() -> Option<u64> {
    let _guard = ENTROPY_LOCK.lock();
    let base = find_device(DEVICE_ID_ENTROPY)?;
    unsafe {
        if read32(base, 0x004) != 2 {
            return None;
        }
        let status = read32(base, 0x070);
        if status & STATUS_DRIVER_OK == 0 {
            if !initialise_entropy(base) {
                return None;
            }
        }

        let queue = &raw mut ENTROPY_QUEUE;
        let bytes = &raw mut (*queue).bytes;
        (*queue).descriptor[0] = Descriptor {
            address: bytes as u64,
            length: 8,
            flags: DESC_DEVICE_WRITES,
            next: 0,
        };
        let available = core::ptr::read_volatile(&raw const (*queue).avail_index);
        let used_before = core::ptr::read_volatile(&raw const (*queue).used_index);
        (*queue).avail_ring[(available as usize) & 7] = 0;
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        core::ptr::write_volatile(&raw mut (*queue).avail_index, available.wrapping_add(1));
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        write32(base, 0x050, 0);

        let start = super::timer::read_counter();
        let timeout = (super::timer::frequency() / 10).max(1);
        while core::ptr::read_volatile(&raw const (*queue).used_index) == used_before {
            if super::timer::read_counter().wrapping_sub(start) >= timeout {
                return None;
            }
            core::hint::spin_loop();
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        Some(u64::from_ne_bytes((*queue).bytes))
    }
}

pub fn probe_entropy_driver() -> crate::drivers::ProbeResult {
    if find_device(DEVICE_ID_ENTROPY).is_none() {
        return crate::drivers::ProbeResult::NoMatch;
    }
    if random_u64().is_some() {
        crate::drivers::ProbeResult::Bound
    } else {
        crate::drivers::ProbeResult::Failed
    }
}

unsafe fn initialise_entropy(base: u64) -> bool {
    unsafe {
        write32(base, 0x070, 0);
        write32(base, 0x070, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        write32(base, 0x014, 0);
        let _features_low = read32(base, 0x010);
        write32(base, 0x014, 1);
        let features_high = read32(base, 0x010);
        if features_high & VIRTIO_F_VERSION_1_HIGH == 0 {
            write32(base, 0x070, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FAILED);
            return false;
        }
        write32(base, 0x024, 0);
        write32(base, 0x020, 0);
        write32(base, 0x024, 1);
        write32(base, 0x020, VIRTIO_F_VERSION_1_HIGH);
        let features_ok = STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK;
        write32(base, 0x070, features_ok);
        if read32(base, 0x070) & STATUS_FEATURES_OK == 0 {
            write32(base, 0x070, features_ok | STATUS_FAILED);
            return false;
        }
        write32(base, 0x030, 0);
        if read32(base, 0x034) < 8 {
            write32(base, 0x070, features_ok | STATUS_FAILED);
            return false;
        }
        write32(base, 0x038, 8);
        let queue = &raw mut ENTROPY_QUEUE;
        let descriptor = &raw mut (*queue).descriptor as u64;
        let available = &raw mut (*queue).avail_flags as u64;
        let used = &raw mut (*queue).used_flags as u64;
        write32(base, 0x080, descriptor as u32);
        write32(base, 0x084, (descriptor >> 32) as u32);
        write32(base, 0x090, available as u32);
        write32(base, 0x094, (available >> 32) as u32);
        write32(base, 0x0a0, used as u32);
        write32(base, 0x0a4, (used >> 32) as u32);
        write32(base, 0x044, 1);
        write32(base, 0x070, features_ok | STATUS_DRIVER_OK);
        read32(base, 0x070) & STATUS_DRIVER_OK != 0
    }
}

// -------------------------------------------------------------------------
// Shared modern MMIO device setup
// -------------------------------------------------------------------------

pub(crate) unsafe fn begin_modern_device(base: u64, accepted_low: u32) -> bool {
    unsafe {
        if read32(base, 0x000) != MAGIC || read32(base, 0x004) != 2 {
            return false;
        }
        write32(base, 0x070, 0);
        let initial = STATUS_ACKNOWLEDGE | STATUS_DRIVER;
        write32(base, 0x070, initial);
        write32(base, 0x014, 0);
        let offered_low = read32(base, 0x010);
        write32(base, 0x014, 1);
        let offered_high = read32(base, 0x010);
        if offered_high & VIRTIO_F_VERSION_1_HIGH == 0 {
            write32(base, 0x070, initial | STATUS_FAILED);
            return false;
        }
        write32(base, 0x024, 0);
        write32(base, 0x020, offered_low & accepted_low);
        write32(base, 0x024, 1);
        write32(base, 0x020, VIRTIO_F_VERSION_1_HIGH);
        let negotiated = initial | STATUS_FEATURES_OK;
        write32(base, 0x070, negotiated);
        if read32(base, 0x070) & STATUS_FEATURES_OK == 0 {
            write32(base, 0x070, negotiated | STATUS_FAILED);
            return false;
        }
        true
    }
}

pub(crate) unsafe fn configure_queue(
    base: u64,
    queue_index: u32,
    requested_size: u32,
    descriptor: u64,
    available: u64,
    used: u64,
) -> Option<u16> {
    unsafe {
        write32(base, 0x030, queue_index);
        if read32(base, 0x044) != 0 {
            return None;
        }
        let size = read32(base, 0x034).min(requested_size);
        if size == 0 || !size.is_power_of_two() {
            return None;
        }
        write32(base, 0x038, size);
        write32(base, 0x080, descriptor as u32);
        write32(base, 0x084, (descriptor >> 32) as u32);
        write32(base, 0x090, available as u32);
        write32(base, 0x094, (available >> 32) as u32);
        write32(base, 0x0a0, used as u32);
        write32(base, 0x0a4, (used >> 32) as u32);
        write32(base, 0x044, 1);
        Some(size as u16)
    }
}

pub(crate) unsafe fn finish_device(base: u64) -> bool {
    unsafe {
        let status = read32(base, 0x070) | STATUS_DRIVER_OK;
        write32(base, 0x070, status);
        read32(base, 0x070) & STATUS_DRIVER_OK != 0
    }
}

// -------------------------------------------------------------------------
// VirtIO block device (device 2)
// -------------------------------------------------------------------------

const BLOCK_QUEUE_SIZE: usize = 8;

#[repr(C, align(4096))]
struct BlockQueue {
    descriptor: [Descriptor; BLOCK_QUEUE_SIZE],
    _descriptor_padding: [u8; 4096 - 16 * BLOCK_QUEUE_SIZE],
    avail_flags: u16,
    avail_index: u16,
    avail_ring: [u16; BLOCK_QUEUE_SIZE],
    used_event: u16,
    _avail_padding: [u8; 4096 - 6 - 2 * BLOCK_QUEUE_SIZE],
    used_flags: u16,
    used_index: u16,
    used_ring: [UsedElement; BLOCK_QUEUE_SIZE],
    avail_event: u16,
    _used_padding: [u8; 4096 - 6 - 8 * BLOCK_QUEUE_SIZE],
}

#[repr(C, align(16))]
struct BlockRequest {
    request_type: u32,
    reserved: u32,
    sector: u64,
    data: [u8; 512],
    status: u8,
}

static BLOCK_LOCK: spin::Mutex<()> = spin::Mutex::new(());
static mut BLOCK_QUEUE: BlockQueue = BlockQueue {
    descriptor: [EMPTY_DESCRIPTOR; BLOCK_QUEUE_SIZE],
    _descriptor_padding: [0; 4096 - 16 * BLOCK_QUEUE_SIZE],
    avail_flags: 0,
    avail_index: 0,
    avail_ring: [0; BLOCK_QUEUE_SIZE],
    used_event: 0,
    _avail_padding: [0; 4096 - 6 - 2 * BLOCK_QUEUE_SIZE],
    used_flags: 0,
    used_index: 0,
    used_ring: [EMPTY_USED; BLOCK_QUEUE_SIZE],
    avail_event: 0,
    _used_padding: [0; 4096 - 6 - 8 * BLOCK_QUEUE_SIZE],
};
static mut BLOCK_REQUEST: BlockRequest = BlockRequest {
    request_type: 0,
    reserved: 0,
    sector: 0,
    data: [0; 512],
    status: 0xff,
};
static mut BLOCK_BASE: u64 = 0;
static mut BLOCK_CAPACITY: u64 = 0;

pub fn init_block() -> bool {
    let Some(base) = find_device(DEVICE_ID_BLOCK) else {
        return false;
    };
    let _guard = BLOCK_LOCK.lock();
    unsafe {
        if BLOCK_BASE == base && read32(base, 0x070) & STATUS_DRIVER_OK != 0 {
            return true;
        }
        if !begin_modern_device(base, 0) {
            return false;
        }
        let queue = &raw mut BLOCK_QUEUE;
        let descriptor = &raw mut (*queue).descriptor as u64;
        let available = &raw mut (*queue).avail_flags as u64;
        let used = &raw mut (*queue).used_flags as u64;
        if configure_queue(
            base,
            0,
            BLOCK_QUEUE_SIZE as u32,
            descriptor,
            available,
            used,
        )
        .is_none()
        {
            write32(base, 0x070, read32(base, 0x070) | STATUS_FAILED);
            return false;
        }
        if !finish_device(base) {
            return false;
        }
        let low = read32(base, 0x100) as u64;
        let high = read32(base, 0x104) as u64;
        BLOCK_CAPACITY = low | (high << 32);
        BLOCK_BASE = base;
        true
    }
}

pub fn block_is_present() -> bool {
    unsafe { BLOCK_BASE != 0 && BLOCK_CAPACITY != 0 }
}

pub fn block_capacity_sectors() -> u64 {
    unsafe { BLOCK_CAPACITY }
}

pub fn read_sector(sector: u64, output: &mut [u8; 512]) -> bool {
    if !block_is_present() && !init_block() {
        return false;
    }
    if !block_request(sector, false, None) {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            (&raw const BLOCK_REQUEST.data).cast::<u8>(),
            output.as_mut_ptr(),
            512,
        );
    }
    true
}

pub fn write_sector(sector: u64, input: &[u8; 512]) -> bool {
    if !block_is_present() && !init_block() {
        return false;
    }
    block_request(sector, true, Some(input))
}

fn block_request(sector: u64, write: bool, input: Option<&[u8; 512]>) -> bool {
    let _guard = BLOCK_LOCK.lock();
    unsafe {
        if sector >= BLOCK_CAPACITY || BLOCK_BASE == 0 {
            return false;
        }
        let request = &raw mut BLOCK_REQUEST;
        (*request).request_type = if write { 1 } else { 0 };
        (*request).reserved = 0;
        (*request).sector = sector;
        (*request).status = 0xff;
        if let Some(bytes) = input {
            (*request).data.copy_from_slice(bytes);
        }
        let header = request as u64;
        let data = &raw mut (*request).data as u64;
        let status = &raw mut (*request).status as u64;
        BLOCK_QUEUE.descriptor[0] = Descriptor {
            address: header,
            length: 16,
            flags: DESC_NEXT,
            next: 1,
        };
        BLOCK_QUEUE.descriptor[1] = Descriptor {
            address: data,
            length: 512,
            flags: DESC_NEXT | if write { 0 } else { DESC_DEVICE_WRITES },
            next: 2,
        };
        BLOCK_QUEUE.descriptor[2] = Descriptor {
            address: status,
            length: 1,
            flags: DESC_DEVICE_WRITES,
            next: 0,
        };
        let available = core::ptr::read_volatile(&raw const BLOCK_QUEUE.avail_index);
        let used_before = core::ptr::read_volatile(&raw const BLOCK_QUEUE.used_index);
        BLOCK_QUEUE.avail_ring[(available as usize) & (BLOCK_QUEUE_SIZE - 1)] = 0;
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        core::ptr::write_volatile(
            &raw mut BLOCK_QUEUE.avail_index,
            available.wrapping_add(1),
        );
        write32(BLOCK_BASE, 0x050, 0);
        let started = super::timer::read_counter();
        let timeout = super::timer::frequency().saturating_mul(5).max(1);
        while core::ptr::read_volatile(&raw const BLOCK_QUEUE.used_index) == used_before {
            if super::timer::read_counter().wrapping_sub(started) >= timeout {
                return false;
            }
            core::hint::spin_loop();
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        (*request).status == 0
    }
}

pub fn probe_block_driver() -> crate::drivers::ProbeResult {
    if find_device(DEVICE_ID_BLOCK).is_none() {
        crate::drivers::ProbeResult::NoMatch
    } else if init_block() {
        crate::drivers::ProbeResult::Bound
    } else {
        crate::drivers::ProbeResult::Failed
    }
}

pub fn acknowledge_block_interrupt() {
    if let Some(base) = find_device(DEVICE_ID_BLOCK) {
        unsafe {
            let status = read32(base, 0x060);
            if status != 0 {
                write32(base, 0x064, status);
            }
        }
    }
}

// -------------------------------------------------------------------------
// VirtIO input device (device 18)
// -------------------------------------------------------------------------

const INPUT_QUEUE_SIZE: usize = 64;

#[repr(C, align(4096))]
struct InputQueue {
    descriptor: [Descriptor; INPUT_QUEUE_SIZE],
    _descriptor_padding: [u8; 4096 - 16 * INPUT_QUEUE_SIZE],
    avail_flags: u16,
    avail_index: u16,
    avail_ring: [u16; INPUT_QUEUE_SIZE],
    used_event: u16,
    _avail_padding: [u8; 4096 - 6 - 2 * INPUT_QUEUE_SIZE],
    used_flags: u16,
    used_index: u16,
    used_ring: [UsedElement; INPUT_QUEUE_SIZE],
    avail_event: u16,
    _used_padding: [u8; 4096 - 6 - 8 * INPUT_QUEUE_SIZE],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    event_type: u16,
    code: u16,
    value: u32,
}

const EMPTY_INPUT_EVENT: InputEvent = InputEvent {
    event_type: 0,
    code: 0,
    value: 0,
};
static INPUT_LOCK: spin::Mutex<()> = spin::Mutex::new(());
static mut INPUT_QUEUE: InputQueue = InputQueue {
    descriptor: [EMPTY_DESCRIPTOR; INPUT_QUEUE_SIZE],
    _descriptor_padding: [0; 4096 - 16 * INPUT_QUEUE_SIZE],
    avail_flags: 0,
    avail_index: 0,
    avail_ring: [0; INPUT_QUEUE_SIZE],
    used_event: 0,
    _avail_padding: [0; 4096 - 6 - 2 * INPUT_QUEUE_SIZE],
    used_flags: 0,
    used_index: 0,
    used_ring: [EMPTY_USED; INPUT_QUEUE_SIZE],
    avail_event: 0,
    _used_padding: [0; 4096 - 6 - 8 * INPUT_QUEUE_SIZE],
};
static mut INPUT_EVENTS: [InputEvent; INPUT_QUEUE_SIZE] = [EMPTY_INPUT_EVENT; INPUT_QUEUE_SIZE];
static mut INPUT_BASE: u64 = 0;
static mut INPUT_USED_INDEX: u16 = 0;
static mut INPUT_WIDTH: u32 = 1024;
static mut INPUT_HEIGHT: u32 = 768;
static mut INPUT_ABS_X: i32 = 512;
static mut INPUT_ABS_Y: i32 = 384;

const POINTER_RING_SIZE: usize = 128;
struct PointerRing {
    values: [Option<crate::input::pointer::PointerEvent>; POINTER_RING_SIZE],
    head: usize,
    tail: usize,
}
impl PointerRing {
    const fn new() -> Self {
        Self {
            values: [None; POINTER_RING_SIZE],
            head: 0,
            tail: 0,
        }
    }
    fn push(&mut self, event: crate::input::pointer::PointerEvent) {
        let next = (self.tail + 1) % POINTER_RING_SIZE;
        if next != self.head {
            self.values[self.tail] = Some(event);
            self.tail = next;
        }
    }
    fn pop(&mut self) -> Option<crate::input::pointer::PointerEvent> {
        if self.head == self.tail {
            return None;
        }
        let value = self.values[self.head].take();
        self.head = (self.head + 1) % POINTER_RING_SIZE;
        value
    }
}
static POINTER_EVENTS: spin::Mutex<PointerRing> = spin::Mutex::new(PointerRing::new());

pub fn set_input_display_bounds(width: u32, height: u32) {
    unsafe {
        INPUT_WIDTH = width.max(1);
        INPUT_HEIGHT = height.max(1);
        INPUT_ABS_X = INPUT_ABS_X.clamp(0, INPUT_WIDTH.saturating_sub(1) as i32);
        INPUT_ABS_Y = INPUT_ABS_Y.clamp(0, INPUT_HEIGHT.saturating_sub(1) as i32);
    }
}

pub fn init_input() -> bool {
    let Some(base) = find_device(DEVICE_ID_INPUT) else {
        return false;
    };
    let _guard = INPUT_LOCK.lock();
    unsafe {
        if INPUT_BASE == base && read32(base, 0x070) & STATUS_DRIVER_OK != 0 {
            return true;
        }
        if !begin_modern_device(base, 0) {
            return false;
        }
        let queue = &raw mut INPUT_QUEUE;
        let descriptor = &raw mut (*queue).descriptor as u64;
        let available = &raw mut (*queue).avail_flags as u64;
        let used = &raw mut (*queue).used_flags as u64;
        let Some(size) = configure_queue(
            base,
            0,
            INPUT_QUEUE_SIZE as u32,
            descriptor,
            available,
            used,
        ) else {
            write32(base, 0x070, read32(base, 0x070) | STATUS_FAILED);
            return false;
        };
        for index in 0..size as usize {
            INPUT_QUEUE.descriptor[index] = Descriptor {
                address: (&raw mut INPUT_EVENTS[index]) as u64,
                length: core::mem::size_of::<InputEvent>() as u32,
                flags: DESC_DEVICE_WRITES,
                next: 0,
            };
            INPUT_QUEUE.avail_ring[index] = index as u16;
        }
        core::ptr::write_volatile(&raw mut INPUT_QUEUE.avail_index, size);
        INPUT_USED_INDEX = 0;
        if !finish_device(base) {
            return false;
        }
        INPUT_BASE = base;
        write32(base, 0x050, 0);
        true
    }
}

fn scale_absolute(value: i32, dimension: u32) -> i32 {
    let max = dimension.saturating_sub(1) as i64;
    ((value.clamp(0, 32767) as i64 * max) / 32767).clamp(0, max) as i32
}

pub fn poll_input() {
    if unsafe { INPUT_BASE == 0 } && !init_input() {
        return;
    }
    let _guard = INPUT_LOCK.lock();
    unsafe {
        let used_now = core::ptr::read_volatile(&raw const INPUT_QUEUE.used_index);
        let mut returned = false;
        while INPUT_USED_INDEX != used_now {
            let used = core::ptr::read_volatile(
                &raw const INPUT_QUEUE.used_ring
                    [(INPUT_USED_INDEX as usize) & (INPUT_QUEUE_SIZE - 1)],
            );
            let id = used.id as usize;
            if id < INPUT_QUEUE_SIZE && used.length >= core::mem::size_of::<InputEvent>() as u32 {
                let event = core::ptr::read_volatile(&raw const INPUT_EVENTS[id]);
                decode_input_event(event);
                let available = core::ptr::read_volatile(&raw const INPUT_QUEUE.avail_index);
                INPUT_QUEUE.avail_ring[(available as usize) & (INPUT_QUEUE_SIZE - 1)] = id as u16;
                core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
                core::ptr::write_volatile(
                    &raw mut INPUT_QUEUE.avail_index,
                    available.wrapping_add(1),
                );
                returned = true;
            }
            INPUT_USED_INDEX = INPUT_USED_INDEX.wrapping_add(1);
        }
        if returned {
            write32(INPUT_BASE, 0x050, 0);
        }
        let interrupt_status = read32(INPUT_BASE, 0x060);
        if interrupt_status != 0 {
            write32(INPUT_BASE, 0x064, interrupt_status);
        }
    }
}

unsafe fn decode_input_event(event: InputEvent) {
    unsafe {
    use crate::input::pointer::{MouseButton, PointerEvent};
    match event.event_type {
        1 => {
            let button = match event.code {
                0x110 => Some(MouseButton::Left),
                0x111 => Some(MouseButton::Right),
                0x112 => Some(MouseButton::Middle),
                _ => None,
            };
            if let Some(button) = button {
                POINTER_EVENTS.lock().push(PointerEvent::Button {
                    button,
                    pressed: event.value != 0,
                });
            }
        }
        2 => {
            let delta = event.value as i32;
            match event.code {
                0 => POINTER_EVENTS.lock().push(PointerEvent::Move {
                    dx: delta.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    dy: 0,
                }),
                1 => POINTER_EVENTS.lock().push(PointerEvent::Move {
                    dx: 0,
                    dy: delta.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                }),
                _ => {}
            }
        }
        3 => {
            match event.code {
                0 => INPUT_ABS_X = scale_absolute(event.value as i32, INPUT_WIDTH),
                1 => INPUT_ABS_Y = scale_absolute(event.value as i32, INPUT_HEIGHT),
                _ => return,
            }
            POINTER_EVENTS.lock().push(PointerEvent::Absolute {
                x: INPUT_ABS_X,
                y: INPUT_ABS_Y,
            });
        }
        _ => {}
    }
    }
}

pub fn input_has_pending_event() -> bool {
    poll_input();
    let ring = POINTER_EVENTS.lock();
    ring.head != ring.tail
}

pub fn input_try_read_event() -> Option<crate::input::pointer::PointerEvent> {
    poll_input();
    POINTER_EVENTS.lock().pop()
}

pub fn probe_input_driver() -> crate::drivers::ProbeResult {
    if find_device(DEVICE_ID_INPUT).is_none() {
        crate::drivers::ProbeResult::NoMatch
    } else if init_input() {
        crate::drivers::ProbeResult::Bound
    } else {
        crate::drivers::ProbeResult::Failed
    }
}

pub fn acknowledge_input_interrupt() {
    poll_input();
}

// -------------------------------------------------------------------------
// VirtIO network device (device 1)
// -------------------------------------------------------------------------

const NET_QUEUE_SIZE: usize = 64;
const NET_HEADER_SIZE: usize = 10;
const NET_FRAME_SIZE: usize = 1514;
const NET_F_MAC: u32 = 1 << 5;

#[repr(C, align(4096))]
struct NetQueue {
    descriptor: [Descriptor; NET_QUEUE_SIZE],
    _descriptor_padding: [u8; 4096 - 16 * NET_QUEUE_SIZE],
    avail_flags: u16,
    avail_index: u16,
    avail_ring: [u16; NET_QUEUE_SIZE],
    used_event: u16,
    _avail_padding: [u8; 4096 - 6 - 2 * NET_QUEUE_SIZE],
    used_flags: u16,
    used_index: u16,
    used_ring: [UsedElement; NET_QUEUE_SIZE],
    avail_event: u16,
    _used_padding: [u8; 4096 - 6 - 8 * NET_QUEUE_SIZE],
}

const EMPTY_NET_QUEUE: NetQueue = NetQueue {
    descriptor: [EMPTY_DESCRIPTOR; NET_QUEUE_SIZE],
    _descriptor_padding: [0; 4096 - 16 * NET_QUEUE_SIZE],
    avail_flags: 0,
    avail_index: 0,
    avail_ring: [0; NET_QUEUE_SIZE],
    used_event: 0,
    _avail_padding: [0; 4096 - 6 - 2 * NET_QUEUE_SIZE],
    used_flags: 0,
    used_index: 0,
    used_ring: [EMPTY_USED; NET_QUEUE_SIZE],
    avail_event: 0,
    _used_padding: [0; 4096 - 6 - 8 * NET_QUEUE_SIZE],
};

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct NetBuffer([u8; NET_HEADER_SIZE + NET_FRAME_SIZE]);

const EMPTY_NET_BUFFER: NetBuffer = NetBuffer([0; NET_HEADER_SIZE + NET_FRAME_SIZE]);
static NET_LOCK: spin::Mutex<()> = spin::Mutex::new(());
static mut NET_RX_QUEUE: NetQueue = EMPTY_NET_QUEUE;
static mut NET_TX_QUEUE: NetQueue = EMPTY_NET_QUEUE;
static mut NET_RX_BUFFERS: [NetBuffer; NET_QUEUE_SIZE] = [EMPTY_NET_BUFFER; NET_QUEUE_SIZE];
static mut NET_TX_BUFFER: NetBuffer = EMPTY_NET_BUFFER;
static mut NET_BASE: u64 = 0;
static mut NET_RX_USED: u16 = 0;
static mut NET_TX_USED: u16 = 0;
static mut NET_MAC: [u8; 6] = [0; 6];

pub fn init_network() -> bool {
    let Some(base) = find_device(DEVICE_ID_NETWORK) else {
        return false;
    };
    let _guard = NET_LOCK.lock();
    unsafe {
        if NET_BASE == base && read32(base, 0x070) & STATUS_DRIVER_OK != 0 {
            return true;
        }
        if !begin_modern_device(base, NET_F_MAC) {
            return false;
        }
        let rx = &raw mut NET_RX_QUEUE;
        let tx = &raw mut NET_TX_QUEUE;
        let Some(rx_size) = configure_queue(
            base,
            0,
            NET_QUEUE_SIZE as u32,
            (&raw mut (*rx).descriptor) as u64,
            (&raw mut (*rx).avail_flags) as u64,
            (&raw mut (*rx).used_flags) as u64,
        ) else {
            return false;
        };
        if configure_queue(
            base,
            1,
            NET_QUEUE_SIZE as u32,
            (&raw mut (*tx).descriptor) as u64,
            (&raw mut (*tx).avail_flags) as u64,
            (&raw mut (*tx).used_flags) as u64,
        )
        .is_none()
        {
            return false;
        }
        for index in 0..rx_size as usize {
            NET_RX_QUEUE.descriptor[index] = Descriptor {
                address: (&raw mut NET_RX_BUFFERS[index]) as u64,
                length: (NET_HEADER_SIZE + NET_FRAME_SIZE) as u32,
                flags: DESC_DEVICE_WRITES,
                next: 0,
            };
            NET_RX_QUEUE.avail_ring[index] = index as u16;
        }
        core::ptr::write_volatile(&raw mut NET_RX_QUEUE.avail_index, rx_size);
        NET_RX_USED = 0;
        NET_TX_USED = 0;
        let mut mac = [0u8; 6];
        for (index, byte) in mac.iter_mut().enumerate() {
            *byte = core::ptr::read_volatile((base + 0x100 + index as u64) as *const u8);
        }
        if mac == [0; 6] {
            return false;
        }
        NET_MAC = mac;
        if !finish_device(base) {
            return false;
        }
        NET_BASE = base;
        write32(base, 0x050, 0);

        let ll = crate::net::ipv6::Ipv6Addr::link_local_from_mac(NET_MAC);
        crate::net::set_our_ipv6(ll.0);
        if !crate::grid::is_active() {
            crate::grid::init(NET_MAC);
        }
        true
    }
}

pub fn network_is_present() -> bool {
    unsafe { NET_BASE != 0 }
}

pub fn network_mac() -> [u8; 6] {
    unsafe { NET_MAC }
}

pub fn transmit_frame(frame: &[u8]) -> bool {
    if frame.is_empty() || frame.len() > NET_FRAME_SIZE {
        return false;
    }
    if !network_is_present() && !init_network() {
        return false;
    }
    let _guard = NET_LOCK.lock();
    unsafe {
        NET_TX_BUFFER.0[..NET_HEADER_SIZE].fill(0);
        NET_TX_BUFFER.0[NET_HEADER_SIZE..NET_HEADER_SIZE + frame.len()].copy_from_slice(frame);
        NET_TX_QUEUE.descriptor[0] = Descriptor {
            address: (&raw mut NET_TX_BUFFER) as u64,
            length: (NET_HEADER_SIZE + frame.len()) as u32,
            flags: 0,
            next: 0,
        };
        let available = core::ptr::read_volatile(&raw const NET_TX_QUEUE.avail_index);
        let used_before = core::ptr::read_volatile(&raw const NET_TX_QUEUE.used_index);
        NET_TX_QUEUE.avail_ring[(available as usize) & (NET_QUEUE_SIZE - 1)] = 0;
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        core::ptr::write_volatile(
            &raw mut NET_TX_QUEUE.avail_index,
            available.wrapping_add(1),
        );
        write32(NET_BASE, 0x050, 1);
        let started = super::timer::read_counter();
        let timeout = super::timer::frequency().saturating_mul(2).max(1);
        while core::ptr::read_volatile(&raw const NET_TX_QUEUE.used_index) == used_before {
            if super::timer::read_counter().wrapping_sub(started) >= timeout {
                return false;
            }
            core::hint::spin_loop();
        }
        NET_TX_USED = NET_TX_USED.wrapping_add(1);
        true
    }
}

pub fn poll_network_rx() {
    if !network_is_present() && !init_network() {
        return;
    }
    let _guard = NET_LOCK.lock();
    unsafe {
        let used_now = core::ptr::read_volatile(&raw const NET_RX_QUEUE.used_index);
        let mut returned = false;
        while NET_RX_USED != used_now {
            let used = core::ptr::read_volatile(
                &raw const NET_RX_QUEUE.used_ring
                    [(NET_RX_USED as usize) & (NET_QUEUE_SIZE - 1)],
            );
            let id = used.id as usize;
            let length = used.length as usize;
            if id < NET_QUEUE_SIZE && length > NET_HEADER_SIZE {
                let payload_len = (length - NET_HEADER_SIZE).min(NET_FRAME_SIZE);
                crate::net::receive_raw_frame(
                    &NET_RX_BUFFERS[id].0[NET_HEADER_SIZE..NET_HEADER_SIZE + payload_len],
                );
                let available = core::ptr::read_volatile(&raw const NET_RX_QUEUE.avail_index);
                NET_RX_QUEUE.avail_ring[(available as usize) & (NET_QUEUE_SIZE - 1)] = id as u16;
                core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
                core::ptr::write_volatile(
                    &raw mut NET_RX_QUEUE.avail_index,
                    available.wrapping_add(1),
                );
                returned = true;
            }
            NET_RX_USED = NET_RX_USED.wrapping_add(1);
        }
        if returned {
            write32(NET_BASE, 0x050, 0);
        }
        let interrupt_status = read32(NET_BASE, 0x060);
        if interrupt_status != 0 {
            write32(NET_BASE, 0x064, interrupt_status);
        }
    }
}

pub fn probe_network_driver() -> crate::drivers::ProbeResult {
    if find_device(DEVICE_ID_NETWORK).is_none() {
        crate::drivers::ProbeResult::NoMatch
    } else if init_network() {
        crate::drivers::ProbeResult::Bound
    } else {
        crate::drivers::ProbeResult::Failed
    }
}

pub fn acknowledge_network_interrupt() {
    poll_network_rx();
}
