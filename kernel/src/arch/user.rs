//! User-mode entry operations shared by the scheduler.

#[cfg(target_arch = "x86_64")]
pub use crate::arch::x86_64::ring3::{
    enter_user_mode, enter_user_mode_with_arg, init_fast_syscalls,
    set_syscall_kernel_stack, unwind_nonreturning_fast_syscall,
};

#[cfg(target_arch = "aarch64")]
pub use crate::arch::aarch64::user::{
    enter_user_mode, enter_user_mode_with_arg, init_fast_syscalls,
    set_syscall_kernel_stack,
};
