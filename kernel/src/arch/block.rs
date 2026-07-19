//! Platform block-device transport.

#[cfg(target_arch = "x86_64")]
pub use crate::arch::x86_64::virtio_blk::{
    capacity_sectors, init, is_present, read_sector, write_sector,
};

#[cfg(target_arch = "aarch64")]
pub use crate::arch::aarch64::virtio_mmio::{
    block_capacity_sectors as capacity_sectors, block_is_present as is_present,
    init_block as init, read_sector, write_sector,
};
