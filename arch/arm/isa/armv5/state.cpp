/* brain-emu -- ARMv5TEJ ISA: register banking and PC writes.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The banking rules are architectural (ARM DDI 0100I A2.3.2, "Register
 * banking"): which registers a mode actually banks, and that R0-R7 - and
 * R8-R12 outside FIQ - name the same physical registers in all modes, so a
 * mode change must leave them untouched.  Copying a whole bank instead made
 * any exception wipe r0-r7 of the faulting thread, which is why this file
 * spells the split out.  The shared view of R8-R12 lives in the user bank,
 * so leaving FIQ restores it.
 *
 * set_reg() applies the R15 write rules (A2.4.4): a write to the PC is a
 * branch - the pipeline view is rebuilt by the next fetch, and in Thumb
 * state bit[0] selects the execution state, as for BX.
 */
#include "arch/arm/isa/armv5/state.h"

namespace brain::arm::isa::armv5 {

static bool bank_is_fiq(u32 idx) { return idx == BANK_FIQ; }

u32 IsaState::bank_for_mode(u32 mode) const {
  switch (mode & CPSR_MODE_M) {
    case MODE_FIQ: return BANK_FIQ;
    case MODE_IRQ: return BANK_IRQ;
    case MODE_SVC: return BANK_SVC;
    case MODE_ABT: return BANK_ABT;
    case MODE_UND: return BANK_UND;
    default: return BANK_USR;   /* USR and SYS share the user bank */
  }
}

void IsaState::flush_bank() {
  Bank& b = bank[bank_idx];
  b.r[13] = regs[13];
  b.r[14] = regs[14];
  Bank& shared = bank_is_fiq(bank_idx) ? b : bank[BANK_USR];
  for (u32 i = 8; i < 13; ++i) shared.r[i] = regs[i];
  /* SPSR_<mode> is deliberately *not* written here.  It is not a mirror of the
   * CPSR: it holds the value an exception saved on entry, and MRS/MSR see it as
   * bank[bank_idx].spsr.  Saving the current CPSR on every mode change made an
   * IRQ taken inside a Supervisor handler (or a detour through System mode)
   * overwrite the SPSR that handler was about to return with. */
}

void IsaState::load_bank(u32 idx) {
  bank_idx = idx;
  const Bank& b = bank[idx];
  regs[13] = b.r[13];
  regs[14] = b.r[14];
  const Bank& shared = bank_is_fiq(idx) ? b : bank[BANK_USR];
  for (u32 i = 8; i < 13; ++i) regs[i] = shared.r[i];
}

void IsaState::switch_mode(u32 new_mode) {
  new_mode &= CPSR_MODE_M;
  cpsr = (cpsr & ~CPSR_MODE_M) | new_mode;
  priv = (new_mode != MODE_USR);
  const u32 idx = bank_for_mode(new_mode);
  if (idx == bank_idx) return;   /* USR<->SYS share a bank: nothing to move. */
  flush_bank();
  load_bank(idx);
}

void IsaState::set_reg(u32 n, u32 val) {
  if (n == 15) {
    if (thumb) {
      npc = val & ~1u;             /* bit[0] selects the state, as for BX     */
      if (!(val & 1u)) { thumb = 0; cpsr &= ~(CPSR_T | CPSR_J); }
    } else {
      npc = val & ~3u;             /* bits[1:0] ignored in ARM state          */
    }
    /* cur/regs[15] are refreshed by the next fetch: see the execution layer's
     * fetch_and_execute (arch/arm/execution/aarch32/step.cpp). */
    return;
  }
  regs[n] = val;
  /* Keep the bank of the mode we are in in step, or the next mode change (an
   * exception, an MSR) would restore the stale value over this one.  Only
   * R13/R14 and, in FIQ, R8-R12 are banked - see IsaState::flush_bank(). */
  if (n >= 13 || (bank_idx == BANK_FIQ && n >= 8)) bank[bank_idx].r[n] = val;
}

bool valid_mode(u32 m) {
  switch (m) {
    case MODE_USR: case MODE_FIQ: case MODE_IRQ: case MODE_SVC:
    case MODE_ABT: case MODE_UND: case MODE_SYS:
      return true;
    default:
      return false;
  }
}

}  // namespace brain::arm::isa::armv5
