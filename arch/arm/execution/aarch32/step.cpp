/* brain-emu -- AArch32 execution state: the step driver.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The pipeline step shared by both instruction states.  What is AArch32 (as
 * opposed to ISA) here: the T bit selecting which decoder runs, and the
 * pipeline rule regs[15] == cur + 8 (ARM) / cur + 4 (Thumb) that every
 * instruction reads the PC through.  The fetch itself - translation, the RAM
 * fast path, the abort - belongs to the core and arrives through IsaEnv.
 */
#include "arch/arm/execution/aarch32/step.h"

#include "arch/arm/isa/armv5/interp.h"

namespace brain::arm::execution::aarch32 {

void fetch_and_execute(armv5::IsaState& s, armv5::IsaEnv& env) {
  /* Prime the pipeline: `cur` becomes the instruction we are about to execute
   * and R15 gets the value that instruction will read (cur + 8 / cur + 4).
   * A prefetch abort is reported against `cur` before it is overwritten. */
  s.cur = s.npc;
  /* A fetch that aborts retires no instruction, but the step still has to
   * charge one instruction and one cycle: the abort has been taken inside
   * prepare_fetch(), the pipeline has moved to the vector, and a bounded run
   * (either insn- or cycle-budgeted) must be able to terminate even when the
   * vector itself faults. */
  if (BRAIN_UNLIKELY(!env.prepare_fetch(env.ctx))) { s.insns++; s.cycles++; return; }

  if (s.thumb) {
    const u32 hw = env.fetch16(env.ctx, s.cur);
    if (BRAIN_UNLIKELY(s.exectrap)) {
      env.take_exception(env.ctx, armv5::EXC_PABORT);
      s.insns++;
      s.cycles++;
      return;
    }
    s.insn = hw;
    s.npc = s.cur + 2;
    s.regs[15] = s.cur + 4;
    armv5::exec_thumb(s, hw, env);
    return;
  }

  const u32 w = env.fetch32(env.ctx, s.cur);
  if (BRAIN_UNLIKELY(s.exectrap)) {
    env.take_exception(env.ctx, armv5::EXC_PABORT);
    s.insns++;
    s.cycles++;
    return;
  }
  s.insn = w;
  s.npc = s.cur + 4;
  s.regs[15] = s.cur + 8;
  armv5::exec_arm(s, w, env);
}

}  // namespace brain::arm::execution::aarch32
