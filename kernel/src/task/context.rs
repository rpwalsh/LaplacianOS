//! Architecture register state saved by the low-level context switch.

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CpuContext {
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
}

#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CpuContext {
    // AAPCS64 callee-saved register set.
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64,
    /// AArch64 link register. Fresh tasks return through the task-exit
    /// trampoline; suspended tasks retain their caller return address.
    pub x30: u64,
    /// Resume address captured by the context-switch assembly.
    pub pc: u64,
    pub sp: u64,
    /// Saved DAIF mask bits. Fresh tasks start with IRQ/FIQ unmasked.
    pub daif: u64,
}

impl CpuContext {
    pub const fn zero() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            Self {
                rbx: 0,
                rbp: 0,
                r12: 0,
                r13: 0,
                r14: 0,
                r15: 0,
                rip: 0,
                rsp: 0,
                rflags: 0,
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            Self {
                x19: 0,
                x20: 0,
                x21: 0,
                x22: 0,
                x23: 0,
                x24: 0,
                x25: 0,
                x26: 0,
                x27: 0,
                x28: 0,
                x29: 0,
                x30: 0,
                pc: 0,
                sp: 0,
                daif: 0,
            }
        }
    }

    pub const fn new_kernel(
        entry: u64,
        stack_top: u64,
        return_address: u64,
        initial_pc: u64,
    ) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let _ = return_address;
            let _ = initial_pc;
            Self {
                rbx: 0,
                rbp: 0,
                r12: 0,
                r13: 0,
                r14: 0,
                r15: 0,
                rip: entry,
                rsp: stack_top,
                rflags: 0x200,
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            Self {
                // The AArch64 first-dispatch shim branches to the real entry
                // in x19 only after it has safely unmasked interrupts.
                x19: entry,
                x20: 0,
                x21: 0,
                x22: 0,
                x23: 0,
                x24: 0,
                x25: 0,
                x26: 0,
                x27: 0,
                x28: 0,
                x29: 0,
                x30: return_address,
                pc: initial_pc,
                sp: stack_top,
                daif: 0,
            }
        }
    }

    /// Ensure a saved task will resume with normal IRQ delivery enabled.
    pub fn enable_interrupts(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            self.rflags |= 0x200;
        }
        #[cfg(target_arch = "aarch64")]
        {
            // DAIF.I is bit 7 and DAIF.F is bit 6.
            self.daif &= !((1 << 7) | (1 << 6));
        }
    }
}
