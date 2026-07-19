/* LaplacianOS AArch64 kernel image for the QEMU virt direct-kernel boot flow. */
ENTRY(_start)

SECTIONS
{
    . = 0x40080000;
    __kernel_start = .;
    __kernel_phys_link_base = .;

    .text ALIGN(4K) :
    {
        KEEP(*(.text.boot))
        KEEP(*(.text.exceptions))
        *(.text .text.*)
    }
    __text_end = .;

    .rodata ALIGN(4K) :
    {
        __rodata_start = .;
        *(.rodata .rodata.*)
    }
    __rodata_end = .;

    .data ALIGN(4K) :
    {
        __data_start = .;
        *(.data .data.*)
    }

    .bss ALIGN(4K) (NOLOAD) :
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* The active boot stack must not be inside the range zeroed by the Rust
       trampoline: compiler spills and the preserved DTB argument live here. */
    .boot_stack ALIGN(4K) (NOLOAD) :
    {
        . = ALIGN(16);
        __boot_stack_bottom = .;
        . += 0x40000;
        __boot_stack_top = .;
    }

    __kernel_end = .;

    /DISCARD/ :
    {
        *(.eh_frame*)
        *(.comment*)
        *(.note*)
    }
}
