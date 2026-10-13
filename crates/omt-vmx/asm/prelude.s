// The assembler macros the decoder's AArch64 kernels are written against.
//
// A function is .text, global, typed, and 16-byte aligned. There is no BTI
// landing pad and no GNU property note: the stable toolchain that builds the
// appliance does not enable branch protection for the Rust code either, and
// the linker ANDs the note over every object, so marking only these two
// functions would change nothing. The kernels are leaf functions that never
// spill the link register, so they stay compatible with BTI and PAC should
// branch protection be turned on later (add the pad and note then).

.macro vmx_function name
        .text
        .globl  \name
        .type   \name, %function
        .p2align 4
\name:
.endm

.macro vmx_end name
        .size   \name, . - \name
.endm
