/* brain-emu -- ARMv5TEJ ISA: the interpreter engine's entry points.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The engine is a set of free functions over the architectural state
 * (IsaState) plus the environment contract (IsaEnv) the core fills in:
 *
 *   exec_arm(s, insn, env)      decode + execute one ARM-state word
 *   exec_arm_dec(s, d, env)     execute one already-decoded instruction
 *                               (both ARM- and Thumb-decoded ArmDecs)
 *   exec_thumb(s, hw, env)      decode + execute one Thumb halfword
 *
 * plus the operand/addressing/PSR helpers the two execution states share
 * (arm_op2, arm_ldst_offset, arm_block_addrs, restore_cpsr_from_spsr,
 * load_pc, apply_msr).  The JIT's out-of-line helpers call the same
 * exec_arm/exec_thumb entry points, so a bail-out instruction and an
 * interpreted one are the same code.
 *
 * The AArch32 execution-state driver (the pipeline step that primes
 * cur/npc/regs[15] and dispatches to the state's engine) lives one level up,
 * in arch/arm/execution/aarch32/.
 */
#ifndef BRAIN_ISA_ARMV5_INTERP_H
#define BRAIN_ISA_ARMV5_INTERP_H

#include "arch/arm/isa/armv5/decode.h"
#include "arch/arm/isa/armv5/memory.h"
#include "arch/arm/isa/armv5/state.h"

namespace brain::arm::isa::armv5 {

/* operand2 of a data-processing instruction (register/immediate, shifter
 * applied) and the addressing helpers shared by every load/store form. */
Shifted arm_op2(const IsaState& s, const ArmDec& d);
u32 arm_ldst_offset(const IsaState& s, const ArmDec& d);
void arm_block_addrs(const IsaState& s, const ArmDec& d, u32 n, u32* start,
                     u32* wb);

/* PSR semantics: restoring CPSR from SPSR (MOVS/LDM with S and PC in the
 * list), writing the PC (a branch - the pipeline view is rebuilt by the next
 * fetch), and MSR (which may change the mode, hence the register bank). */
void restore_cpsr_from_spsr(IsaState& s, IsaEnv& env);
void load_pc(IsaState& s, u32 value, bool from_load = false);
void apply_msr(IsaState& s, u32 mask, u32 val, bool spsr, IsaEnv& env);

/* ---- the engines --------------------------------------------------------- */
void exec_arm(IsaState& s, u32 insn, IsaEnv& env);
void exec_arm_dec(IsaState& s, const ArmDec& d, IsaEnv& env);
void exec_thumb(IsaState& s, u32 hw, IsaEnv& env);

}  // namespace brain::arm::isa::armv5

#endif  /* BRAIN_ISA_ARMV5_INTERP_H */
