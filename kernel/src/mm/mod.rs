//! Memory management subsystem.

pub mod address_space;
pub mod frame_alloc;
pub mod heap;
pub mod kaslr;
pub mod page_cache;
#[cfg(target_arch = "x86_64")]
#[path = "page_table_x86_64.rs"]
pub mod page_table;
#[cfg(target_arch = "aarch64")]
#[path = "page_table_aarch64.rs"]
pub mod page_table;
pub mod phys;
pub mod reserved;
pub mod swap;
