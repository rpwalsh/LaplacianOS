//! Architecture-neutral PCI/PCIe configuration-space access.

#[cfg(target_arch = "x86_64")]
pub use crate::arch::x86_64::pci::*;

#[cfg(target_arch = "aarch64")]
pub use crate::arch::aarch64::pci::*;
