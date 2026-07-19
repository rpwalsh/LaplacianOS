use spin::Mutex;

pub const BLOCK_SIZE: usize = 512;
pub const BLOCK_COUNT: usize = 256;

struct BlockState {
    bytes: [u8; BLOCK_SIZE * BLOCK_COUNT],
    writes: u64,
}

impl BlockState {
    const fn new() -> Self {
        Self {
            bytes: [0; BLOCK_SIZE * BLOCK_COUNT],
            writes: 0,
        }
    }
}

static DEVICE: Mutex<BlockState> = Mutex::new(BlockState::new());

pub fn init() {
    if crate::arch::block::init() {
        crate::arch::serial::write_line(b"[block] virtio block device online");
    } else {
        crate::arch::serial::write_line(b"[block] no hardware disk; volatile ramdisk online");
    }
}

pub fn read(block: u32, buf: &mut [u8; BLOCK_SIZE]) -> bool {
    // Prefer real virtio-blk when available.
    if crate::arch::block::is_present() {
        return crate::arch::block::read_sector(block as u64, buf);
    }
    let start = block as usize * BLOCK_SIZE;
    let end = start + BLOCK_SIZE;
    if end > BLOCK_SIZE * BLOCK_COUNT {
        return false;
    }
    let state = DEVICE.lock();
    buf.copy_from_slice(&state.bytes[start..end]);
    true
}

pub fn write(block: u32, buf: &[u8; BLOCK_SIZE]) -> bool {
    // Prefer real virtio-blk when available.
    if crate::arch::block::is_present() {
        return crate::arch::block::write_sector(block as u64, buf);
    }
    let start = block as usize * BLOCK_SIZE;
    let end = start + BLOCK_SIZE;
    if end > BLOCK_SIZE * BLOCK_COUNT {
        return false;
    }
    let mut state = DEVICE.lock();
    state.bytes[start..end].copy_from_slice(buf);
    state.writes = state.writes.saturating_add(1);
    true
}

pub const fn block_count() -> u32 {
    BLOCK_COUNT as u32
}

pub fn backend_name() -> &'static [u8] {
    if crate::arch::block::is_present() {
        b"virtio-blk"
    } else {
        b"volatile-ramdisk"
    }
}

pub fn write_count() -> u64 {
    DEVICE.lock().writes
}
