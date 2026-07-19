/* LaplacianOS Kernel Linker Script — x86_64
 *
 * Places kernel at 1 MiB physical, with 4 KiB aligned sections.
 * Entry point is laplacianos_kmain.
 */

ENTRY(laplacianos_kmain)

SECTIONS
{
    . = 1M;

    __kernel_start = .;
    __kernel_phys_link_base = .;

    .text ALIGN(4K) :
    {
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

    .bss ALIGN(4K) :
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    __kernel_end = .;

    /DISCARD/ :
    {
        *(.eh_frame)
        *(.comment)
        *(.note*)
    }
}
