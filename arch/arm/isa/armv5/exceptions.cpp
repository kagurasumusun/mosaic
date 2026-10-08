/* brain-emu -- ARMv5TEJ ISA: the exception model's tables and the pure
 * (architectural) half of exception entry.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The core-side half - taking the pending exception, FSR/FAR into the system
 * control coprocessor, cycle/statistics accounting, clearing the halt state,
 * notifying the JIT - lives with the core (the ARM926EJ-S core's
 * Cpu::take_exception / Cpu::take_interrupt / Cpu::enter_exception).
 */
#include "arch/arm/isa/armv5/exceptions.h"

#include "arch/arm/isa/armv5/ops.h"          /* CycleModel (core-side charge) */

namespace brain::arm::isa::armv5 {

/* Mode and vector for an exception - the two tables of DDI 0100I A2.6 and the
 * layout of the vector page.  Shared by the traps and the interrupts. */
u32 vector_offset(u32 e) {
  switch (e) {
    case EXC_RESET: return VEC_RESET;
    case EXC_UNDEF: return VEC_UNDEF;
    case EXC_SWI: return VEC_SWI;
    case EXC_PABORT: return VEC_PABORT;
    case EXC_DABORT: return VEC_DABORT;
    case EXC_IRQ: return VEC_IRQ;
    case EXC_FIQ: return VEC_FIQ;
    default: return VEC_RSVD;
  }
}

u32 exception_mode(u32 e) {
  switch (e) {
    case EXC_UNDEF: return MODE_UND;
    case EXC_IRQ:   return MODE_IRQ;
    case EXC_FIQ:   return MODE_FIQ;
    case EXC_PABORT:
    case EXC_DABORT: return MODE_ABT;
    default: return MODE_SVC;   /* SWI, reset */
  }
}

u32 exception_lr_offset(u32 e, bool thumb) {
  if (thumb) {
    switch (e) {
      case EXC_UNDEF:  return 2;   /* the next halfword                     */
      case EXC_SWI:    return 2;
      case EXC_PABORT: return 4;   /* aborted + 4 = retry the fetch          */
      case EXC_DABORT: return 8;   /* aborted + 8 = re-execute               */
      default: return 0;           /* interrupts do not use this table       */
    }
  }
  switch (e) {
    case EXC_UNDEF:  return 4;     /* the next instruction                  */
    case EXC_SWI:    return 4;
    case EXC_PABORT: return 4;     /* aborted + 4                           */
    case EXC_DABORT: return 8;     /* aborted + 8                           */
    default: return 0;             /* interrupts do not use this table       */
  }
}

void enter_exception(IsaState& s, u32 exc, u32 vector, u32 mode, u32 lr,
                     u32 e_bit) {
  const u32 old_cpsr = s.cpsr;
  s.flush_bank();

  const u32 idx = s.bank_for_mode(mode);
  Bank& nb = s.bank[idx];
  nb.spsr = old_cpsr;
  nb.r[14] = lr;
  s.regs[13] = nb.r[13];
  s.regs[14] = nb.r[14];
  const Bank& shared = (idx == BANK_FIQ) ? nb : s.bank[BANK_USR];
  for (u32 i = 8; i < 13; ++i) s.regs[i] = shared.r[i];
  s.bank_idx = idx;

  /* Keep N,Z,C,V,Q and (ARMv5) the A bit.  Set I, and F for FIQ/reset.
   * E comes from the core (`e_bit`: SCTLR.B on an ARM926EJ-S).  Force ARM
   * state (T=0, J=0). */
  u32 keep = old_cpsr & (CPSR_NVQC | CPSR_A);
  u32 f_bit = (exc == EXC_FIQ || exc == EXC_RESET) ? CPSR_F : (old_cpsr & CPSR_F);
  s.cpsr = keep | e_bit | f_bit | CPSR_I | mode;
  s.thumb = 0;
  s.vflag = (s.cpsr >> 28) & 1;
  s.priv = 1;

  /* The handler is the *next* instruction to fetch; cur/regs[15] are primed
   * by the fetch, so LR keeps pointing at the faulting instruction. */
  s.npc = vector;
  s.insn = 0;
  s.exc = 0;
  s.exectrap = 0;
  /* The core adds: halted = 0, cycles += CycleModel::kExceptionCycles,
   * stats.exc[exc]++ and jit->on_exception(). */
}

void raise_undef(IsaState& s, u32 i) {
  s.insn = i; s.exc = EXC_UNDEF; s.exectrap = EXC_UNDEF;
}
void raise_swi(IsaState& s, u32 imm) {
  (void)imm; s.exc = EXC_SWI; s.exectrap = EXC_SWI;
}
void raise_dabort(IsaState& s, IsaEnv& env, u32 fs, u32 va) {
  s.exc = EXC_DABORT; s.exectrap = EXC_DABORT; s.exc_fsr = fs; s.exc_far = va;
  env.stats->page_faults++;
}
void raise_pabort(IsaState& s, IsaEnv& env, u32 fs) {
  s.exc = EXC_PABORT; s.exectrap = EXC_PABORT; s.exc_fsr = fs; s.exc_far = s.cur;
  env.stats->page_faults++;
}

}  // namespace brain::arm::isa::armv5
