/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Shared by the decoder's AArch64 assembly kernels. Assembler, not C: the
 * .inc name keeps it out of the C formatting and analysis gates.
 *
 * mk/flags.mk hands -mbranch-protection=standard to the assembler as well as
 * the compiler, which defines the ACLE feature macros tested here. A function
 * begins with a BTI landing pad when branch protection is on, and each file
 * carries the GNU property note that says so: the linker ANDs that note over
 * every input object, so one object without it would turn the feature off for
 * the whole binary. The kernels are leaf functions that never spill the link
 * register, so return-address signing has nothing to sign and they are
 * PAC-compatible as written.
 */
#ifndef OMT_VMX_AARCH64_INC
#define OMT_VMX_AARCH64_INC

#if defined(__ARM_FEATURE_BTI_DEFAULT) && __ARM_FEATURE_BTI_DEFAULT
#define VMX_BTI_C bti c
#define VMX_FEATURE_BTI 1
#else
#define VMX_BTI_C
#define VMX_FEATURE_BTI 0
#endif

#if defined(__ARM_FEATURE_PAC_DEFAULT) && __ARM_FEATURE_PAC_DEFAULT
#define VMX_FEATURE_PAC 2
#else
#define VMX_FEATURE_PAC 0
#endif

/* A global function: VMX_FUNCTION(name) ... VMX_END(name). */
#define VMX_FUNCTION(name) \
    .text; .globl name; .type name, %function; .p2align 4; name: VMX_BTI_C
#define VMX_END(name) .size name, . - name

#if VMX_FEATURE_BTI || VMX_FEATURE_PAC
/* NT_GNU_PROPERTY_TYPE_0 carrying GNU_PROPERTY_AARCH64_FEATURE_1_AND. */
        .pushsection .note.gnu.property, "a"
        .p2align 3
        .word   4
        .word   16
        .word   5
        .asciz  "GNU"
        .word   0xc0000000
        .word   4
        .word   VMX_FEATURE_BTI | VMX_FEATURE_PAC
        .word   0
        .popsection
#endif

#endif
