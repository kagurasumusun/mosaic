/* brain-emu -- AArch32 execution state: the step driver.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * AArch32 is the 32-bit Arm execution state: two instruction states (ARM and
 * Thumb, selected by the T bit), a 32-bit register file, the A32/T32
 * encodings.  This layer is what "executing in AArch32 state" means around the
 * ISA: the pipeline step that primes the architectural view (cur/npc/regs[15])
 * and dispatches to the state's engine, and the fetch-abort boundary.
 *
 * It sits between the two layers it connects:
 *
 *   - below it, the ISA (arch/arm/isa/armv5/): the decoders and the execution
 *     semantics of every instruction, written against IsaState + IsaEnv;
 *   - above it, the core (arch/arm/cpu/...): run control (budgets, halting,
 *     stop requests, the JIT dispatcher) and the environment itself - memory
 *     and translation, the coprocessor, exception entry, JIT notification.
 *
 * The split is what lets the same engine serve both instruction states and
 * what keeps the ARM/Thumb state machine out of the ISA: the ISA defines what
 * an instruction does; this layer defines what it means to *be* in the ARM
 * state or the Thumb state and to fetch the next instruction from it.
 *
 * An AArch64 execution state (arch/arm/execution/aarch64/) would be the same
 * shape over a different ISA and a different register file - one step driver,
 * one state enum, no ARM/Thumb interworking.
 */
#ifndef BRAIN_EXEC_AARCH32_STEP_H
#define BRAIN_EXEC_AARCH32_STEP_H

#include "arch/arm/isa/armv5/memory.h"
#include "arch/arm/isa/armv5/state.h"

namespace brain::arm::execution::aarch32 {

namespace armv5 = brain::arm::isa::armv5;

/* Execute the one instruction at `s.cur` in the state the T bit selects:
 * prime the pipeline view (cur = npc, regs[15] = cur + 8 / cur + 4), fetch
 * through the environment (which translates and may abort the fetch), and run
 * the state's engine.  A prefetch abort is taken immediately through the
 * environment's take_exception, because no instruction retires.  The step
 * charges one instruction and one cycle even when the fetch aborts, so a
 * bounded run terminates even when the vector itself faults. */
void fetch_and_execute(armv5::IsaState& s, armv5::IsaEnv& env);

}  // namespace brain::arm::execution::aarch32

#endif  /* BRAIN_EXEC_AARCH32_STEP_H */
