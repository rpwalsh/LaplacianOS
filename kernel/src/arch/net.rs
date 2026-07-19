//! Platform network-device transport.

#[cfg(target_arch = "x86_64")]
pub use crate::arch::x86_64::virtio_net::{
    init, is_present, poll_rx, probe_driver, transmit,
};

#[cfg(target_arch = "aarch64")]
pub use crate::arch::aarch64::virtio_mmio::{
    init_network as init, network_is_present as is_present, poll_network_rx as poll_rx,
    probe_network_driver as probe_driver, transmit_frame as transmit,
};
