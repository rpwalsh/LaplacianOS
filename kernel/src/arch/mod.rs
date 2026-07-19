//! Architecture-specific code.

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;

pub mod interrupts;
pub mod input;
pub mod cpu;
pub mod block;
pub mod entropy;
pub mod machine;
pub mod net;
pub mod paging;
pub mod pci;
pub mod serial;
pub mod timer;
pub mod user;
