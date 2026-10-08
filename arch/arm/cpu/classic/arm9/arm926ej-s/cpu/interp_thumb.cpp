/* brain-emu: Thumb state interpreter (ARMv5T, i.e. the 16-bit instruction set).
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Design note: Thumb is not a separate machine here.  decode_thumb() maps every
 * 16-bit format onto the same ArmDec record the ARM decoder produces, and the
 * work is then done by Cpu::exec_arm_dec() in interp_arm.cpp.  That is faithful
 * to the architecture -- A6.4/A6.6/A6.7 describe almost every Thumb instruction
 * as "equivalent to <ARM instruction>, encoding <word>" -- and it means one
 * barrel shifter, one flag update, one abort path and one cycle model serve both
 * states.  What stays Thumb-specific is exactly what the manual keeps separate:
 *
 *   - no condition codes: only B<cond> is conditional (A6.3.1);
 *   - the PC reads as `instruction address + 4` instead of +8 (A2.4.3), which
 *     the fetch stage installs in regs[15];
 *   - literal loads and ADR use that PC with bits[1:0] treated as zero, because
 *     a Thumb instruction may sit on a halfword boundary;
 *   - writing R15 is an interworking branch (bit[0] picks the state, A6.6.3) and
 *     POP {PC} updates *only* the T bit -- never the flags or the mode
 *     (A7.1.49: "Only the T-bit in the CPSR can be updated by the POP
 *     instruction");
 *   - the long branch is a pair of independent halfwords (A7.1.17), executed as
 *     two instructions by exec_arm_dec's AOP_B kinds 4/5/6;
 *   - unaligned LDR/STR addresses are UNPREDICTABLE and, with alignment
 *     checking on, fault (A7.1.49 Notes).
 */
#include "arch/arm/cpu/classic/arm9/arm926ej-s/cpu/cpu.h"
#include "arch/arm/cpu/classic/arm9/arm926ej-s/cpu/decode.h"
#include "arch/arm/cpu/classic/arm9/arm926ej-s/cpu/decode_thumb.h"
#include "arch/arm/cpu/classic/arm9/arm926ej-s/cpu/ops.h"

namespace brain::arm926 {

void Cpu::exec_thumb(u32 hw) {
  insns++;
  stats.insns_interp++;
  cycles += CycleModel::kIssue;

  ArmDec d;
  decode_thumb(hw, &d);

  if (BRAIN_UNLIKELY(d.op == AOP_UNDEF)) {
    /* Includes the encodings ARMv6 later filled in (BKPT-adjacent hints, the
     * 0xB800/0xB900 LDM/STM-without-writeback forms, REV, IT, ...) plus the
     * whole 0x0100-0x0BFF/0x41000-wide space: they are undefined on ARMv5T. */
    raise_undef(hw);
    return;
  }

  /* Thumb has exactly one conditional branch (A6.3.1); a failed condition makes
   * it a NOP, and it is the not-taken branch the counter wants. */
  if (d.op == AOP_B && d.kind == 3 && !condition_true(d.cond)) {
    stats.branch_not++;
    return;
  }

  exec_arm_dec(d);
}

}  // namespace brain::arm926
