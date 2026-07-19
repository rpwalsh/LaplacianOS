//! Hardware entropy interface.

pub fn random_u64() -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::paging::rdrand64()
    }
    #[cfg(target_arch = "aarch64")]
    {
        // FEAT_RNG is optional on AArch64 and is not present on QEMU's
        // cortex-a72. The virtio-rng transport populates the kernel entropy
        // pool when the platform exposes it; do not fabricate random bytes.
        crate::arch::aarch64::virtio_mmio::random_u64()
    }
}
