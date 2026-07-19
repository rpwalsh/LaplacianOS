// sdk/gl-sdk/src/thread.rs
//
// Minimal thread-spawn/join interface for the gl-sdk rasterizer.
//
// On bare-metal (target_os = "none"): real LaplacianOS kernel syscalls via int 0x80.
// On host test builds: returns None so pipeline.rs falls back to serial execution.

/// Handle returned by a successful thread_spawn.  Must be passed to thread_join.
#[derive(Clone, Copy)]
pub struct ThreadHandle {
    pub tid: u64,
    pub stack_base: u64,
}

#[cfg(target_os = "none")]
mod imp {
    use super::ThreadHandle;
    use core::arch::asm;

    const SYS_THREAD_SPAWN: u64 = 0x010;
    const SYS_THREAD_JOIN: u64 = 0x011;
    const SYS_MMAP: u64 = 0x200;
    const SYS_MUNMAP: u64 = 0x201;

    const PROT_RW: u64 = 0x3; // PROT_READ | PROT_WRITE
    const MAP_PRIV_AN: u64 = 0x5; // MAP_PRIVATE | MAP_ANON
    const STACK_SIZE: u64 = 64 * 1024; // 64 KB per worker thread

    const SYS_THREAD_EXIT: u64 = 0x012;
    const ERR: u64 = u64::MAX;

    #[inline]
    #[cfg(target_arch = "x86_64")]
    fn syscall6(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> u64 {
        let ret: u64;
        unsafe {
            asm!(
                "int 0x80",
                inlateout("rax") nr => ret,
                in("rdi") a0,
                in("rsi") a1,
                in("rdx") a2,
                in("r10") a3,
                in("r8")  a4,
                in("r9")  a5,
                lateout("rcx") _,
                lateout("r11") _,
            );
        }
        ret
    }

    #[inline]
    #[cfg(target_arch = "aarch64")]
    fn syscall6(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> u64 {
        let mut ret = a0;
        unsafe {
            asm!(
                "svc #0",
                in("x8") nr,
                inlateout("x0") ret,
                in("x1") a1,
                in("x2") a2,
                in("x3") a3,
                in("x4") a4,
                in("x5") a5,
                options(nostack)
            );
        }
        ret
    }

    /// Placed at the return-address slot on every thread stack.
    /// When a thread entry function returns (via `ret`), it lands here and
    /// calls SYS_THREAD_EXIT so the kernel can mark the task as dead.
    unsafe extern "C" fn thread_exit_stub(_: u64) -> ! {
        let _ = syscall6(SYS_THREAD_EXIT, 0, 0, 0, 0, 0, 0);
        loop {
            #[cfg(target_arch = "x86_64")]
            unsafe { asm!("ud2", options(nomem, nostack)) };
            #[cfg(target_arch = "aarch64")]
            unsafe { asm!("brk #0", options(nomem, nostack)) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[repr(C)]
    struct ThreadStart {
        entry: unsafe extern "C" fn(u64),
        arg: u64,
    }

    #[cfg(target_arch = "aarch64")]
    unsafe extern "C" fn aarch64_thread_trampoline(start: u64) -> ! {
        let start = unsafe { &*(start as *const ThreadStart) };
        unsafe { (start.entry)(start.arg) };
        unsafe { thread_exit_stub(0) }
    }

    pub fn thread_spawn(entry: unsafe extern "C" fn(u64), arg: u64) -> Option<ThreadHandle> {
        let stack = syscall6(SYS_MMAP, 0, STACK_SIZE, PROT_RW, MAP_PRIV_AN, 0, 0);
        if stack == ERR || stack == 0 {
            return None;
        }
        let stack_top = stack + STACK_SIZE;
        // Write the exit stub into the return-address slot (stack_top - 8).
        // The kernel adjusts RSP to stack_top - 8 on entry, so `ret` from the
        // thread function pops this address and calls SYS_THREAD_EXIT cleanly.
        // SAFETY: stack_top - 8 is within the mmap'd region [stack, stack_top).
        #[cfg(target_arch = "x86_64")]
        let (spawn_entry, spawn_arg, initial_stack) = unsafe {
            let ret_slot = (stack_top - 8) as *mut u64;
            ret_slot.write_volatile(thread_exit_stub as *const () as u64);
            (entry as *const () as u64, arg, stack_top)
        };
        #[cfg(target_arch = "aarch64")]
        let (spawn_entry, spawn_arg, initial_stack) = unsafe {
            let start_ptr = (stack_top - core::mem::size_of::<ThreadStart>() as u64)
                as *mut ThreadStart;
            start_ptr.write(ThreadStart { entry, arg });
            (
                aarch64_thread_trampoline as *const () as u64,
                start_ptr as u64,
                start_ptr as u64,
            )
        };
        let tid = syscall6(
            SYS_THREAD_SPAWN,
            spawn_entry,
            spawn_arg,
            initial_stack,
            0,
            0,
            0,
        );
        if tid == ERR {
            let _ = syscall6(SYS_MUNMAP, stack, STACK_SIZE, 0, 0, 0, 0);
            return None;
        }
        Some(ThreadHandle {
            tid,
            stack_base: stack,
        })
    }

    pub fn thread_join(handle: ThreadHandle) {
        let _ = syscall6(SYS_THREAD_JOIN, handle.tid, 0, 0, 0, 0, 0);
        let _ = syscall6(SYS_MUNMAP, handle.stack_base, STACK_SIZE, 0, 0, 0, 0);
    }
}

#[cfg(not(target_os = "none"))]
mod imp {
    use super::ThreadHandle;

    // On the host (test builds), gl-sdk is no_std and has no thread runtime.
    // Returning None triggers the serial fallback in pipeline.rs.
    pub fn thread_spawn(_entry: unsafe extern "C" fn(u64), _arg: u64) -> Option<ThreadHandle> {
        None
    }

    pub fn thread_join(_handle: ThreadHandle) {}
}

pub use imp::{thread_join, thread_spawn};
