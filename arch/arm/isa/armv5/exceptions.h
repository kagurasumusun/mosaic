/* brain-emu -- ARMv5TEJ ISA: the exception model.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * What the architecture defines about exceptions: the vector table, the mode
 * each exception enters, the LR offset it saves, the FSR encoding of the
 * memory-management faults, and the *architectural* half of exception entry
 * (banking, SPSR, LR, CPSR mode/I/F/E/T, PC := vector).
 *
 * What is deliberately NOT here, because it is core policy: taking the
 * pending exception at a step boundary, writing FSR/FAR into the system
 * control coprocessor's registers, cycle/statistics accounting, clearing the
 * core's halt state, and notifying the JIT.  The embedding core wraps the
 * pure entry below with those (the ARM926EJ-S core's Cpu::take_exception /
 * Cpu::take_interrupt do exactly that).
 *
 * References: ARM DDI 0100I A2.1.13 (exception entry), A2.5 (vector table);
 * the FSR encoding is the ARMv4/v5-style one of TRM table 3-9 (shared by
 * every ARMv5 MMU core: ARM926EJ-S, ARM946E-S, ARM1026EJ-S).
 */
#ifndef BRAIN_ISA_ARMV5_EXCEPTIONS_H
#define BRAIN_ISA_ARMV5_EXCEPTIONS_H

#include "arch/arm/isa/armv5/memory.h"
#include "arch/arm/isa/armv5/state.h"

namespace brain::arm::isa::armv5 {

/* Fault status: the ARMv4/v5-style encoding of TRM table 3-9.
 * FSR[7:4] = domain, FSR[11] = fault came from a second level (page) table,
 * FSR[10] = write access, FSR[9] = section translation fault is *not* used
 * here; bit 8 = external abort on linefill (unused). */
enum : u32 {
  FS926_ALIGN         = 0x01, /* b00x1, alignment                             */
  FS926_EXT_LINEFILL  = 0x08, /* b10x0, external abort on linefill            */
  FS926_TRANS_SECTION = 0x05, /* b0101, translation fault, first level         */
  FS926_TRANS_PAGE    = 0x07, /* b0111, translation fault, second level        */
  FS926_DOMAIN_SECT   = 0x09, /* b1001, domain fault, first level             */
  FS926_DOMAIN_PAGE   = 0x0B, /* b1011, domain fault, second level            */
  FS926_PERM_SECT     = 0x0D, /* b1101, permission fault, first level          */
  FS926_PERM_PAGE     = 0x0F, /* b1111, permission fault, second level         */
  FS926_EXT_TRANS_L1  = 0x0C, /* b1100, external abort on first level table walk*/
  FS926_EXT_TRANS_L2  = 0x0E, /* b1110, external abort on second level walk     */
};
enum : u32 {
  /* TRM table 2-15: the FSR is only [3:0] fault type and [7:4] domain - bit
   * [8] "always reads as zero, writes ignored" and [31:9] UNP/SBZ, so there
   * is no write bit and no level bit here (the level is part of the 4-bit
   * fault type, table 3-9).  Nothing else may be OR-ed into what the guest
   * reads back. */
  FSR_DOMAIN_SHIFT = 4,
};

/* Vector offset of an exception (from the vector base, low or high). */
u32 vector_offset(u32 exc);
/* The mode an exception switches to. */
u32 exception_mode(u32 exc);
/* The LR value saved on entry: cur + offset (the offset depends on the
 * state the exception was taken from). */
u32 exception_lr_offset(u32 exc, bool thumb);

/* The architectural half of exception entry: bank the current registers,
 * save CPSR into SPSR_<mode>, write LR, install the new mode's bank, set
 * CPSR mode/I/F/E (E from `e_bit`, which the core derives from SCTLR.B) and
 * clear T/J, point the PC at `vector`, and clear the pending-exception
 * state.  The core adds cycle/statistics accounting, the halt clear and the
 * JIT notification around this. */
void enter_exception(IsaState& s, u32 exc, u32 vector, u32 mode, u32 lr,
                     u32 e_bit);

/* ---- raising a pending exception ----------------------------------------
 * These only *record* the exception in the state; the core takes it at the
 * next step boundary (or immediately from the fetch path).  The abort kinds
 * also bump the page-fault counter, which is why they take the environment. */
void raise_undef(IsaState& s, u32 insn);
void raise_swi(IsaState& s, u32 imm);
void raise_dabort(IsaState& s, IsaEnv& env, u32 fs, u32 va);
void raise_pabort(IsaState& s, IsaEnv& env, u32 fs);

}  // namespace brain::arm::isa::armv5

#endif  /* BRAIN_ISA_ARMV5_EXCEPTIONS_H */
