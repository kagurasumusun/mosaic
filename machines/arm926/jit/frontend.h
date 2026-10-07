/* Guest ARM -> JIT IR.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * `jit_translate_arm_block` walks guest words from `pc` and appends IR for every
 * instruction that a block is allowed to contain, stopping (successfully) at the
 * first one that is not.  Returning a short block is normal: the caller runs the
 * ops it got and hands the rest back to the dispatcher.
 *
 * Only these are accepted, which is what makes the translation small enough to
 * trust: unconditional (cond == AL) ARM-state data processing (AOP_DP), with
 * R0-R12 as every operand, a shift by an immediate in any of the four directions
 * including RRX (the decoder names that case SH_RRX, so neither this file nor the
 * emitter has to know about the zero field), rotated immediates, and the S forms
 * except where a shift or rotation supplies the carry (AArch64 has no
 * flag-setting ORR/EOR, and its `ADDS #imm` takes C out of the sum rather than out
 * of the rotator).  The carry and overflow bits ARM leaves alone are repaired
 * around the operation; see the comments at flag_patches().
 *
 * Since then a block also owns its loads, stores and its terminating branch
 * (see `translate_ldst`/`translate_branch` below), and there are two front ends'
 * worth of decoding in one file: `decode_arm` words or `decode_thumb` halfwords,
 * the same IR either way.  What stays out in *both* states: a branch that is not
 * the block's last instruction (so also the LR-setting first half of a Thumb
 * long branch), MRS or MSR, the multiply space, LDM/STM/SWP, anything touching
 * R13-R15 (banked, or the PC itself - Thumb's ADR is the exception, because it
 * folds to a constant), a shift amount taken from a register, a 32-bit shift
 * (what ARM state spells `LSR #0` and `ASR #0`, and which AArch64's five-bit
 * shift field cannot say), an access or a predicated instruction where the
 * block could not reproduce the interpreter's answer, and the shapes whose
 * flags the host cannot produce for free.
 * ADCS and SBCS are refused as a matter of policy: chaining a carry correctly needs
 * the flags as of *before* the operation, which costs a snapshot per instruction and
 * is not worth it while the interpreter is close behind.  RSC is not: it is the
 * reversed subtraction, so the host's own subtract-with-borrow is already it (x86-64
 * `cmc` + `sbb` with the operands swapped, AArch64 a plain `SBC`), with nothing to
 * snapshot beyond the borrow-in a materialised shift would otherwise destroy.
 *
 * tests/test_jit_arm64.py is what keeps all of this honest: every emitted encoding
 * is disassembled and every accepted shape is executed on an AArch64 model and
 * compared against the interpreter, for every DP opcode x shift type x shift
 * amount x S x immediate-or-register.
 */
#ifndef BRAIN_JIT_FRONTEND_H
#define BRAIN_JIT_FRONTEND_H

#include <cstdint>
#include <vector>

#include "machines/arm926/jit/ir.h"

namespace brain::arm926 {

class Cpu;

/* Translates at most `max_words` guest *instructions* starting at `pc` (2 bytes
 * wide in Thumb state, 4 in ARM).  Returns the number consumed (0 if the first
 * one is not translatable) and fills `out`.  `stop` in the block says why it
 * ended, and the block's `thumb` says which state it is a block of.
 *
 * `jit_translate_block` picks the state from the machine (`cpu.thumb`), which is
 * what the dispatcher and the cache use - the two entry points below exist so a
 * test can name the state it means. */
uint32_t jit_translate_arm_block(Cpu& cpu, uint32_t pc, uint32_t max_words,
                                 IrBlock* out);
uint32_t jit_translate_thumb_block(Cpu& cpu, uint32_t pc, uint32_t max_insns,
                                   IrBlock* out);
uint32_t jit_translate_block(Cpu& cpu, uint32_t pc, uint32_t max_words,
                             IrBlock* out);

/* The same, over a caller-supplied word array, so a test can translate code that
 * is not in a machine. */
uint32_t jit_translate_arm_words(const uint32_t* words, uint32_t n, uint32_t pc,
                                 IrBlock* out);

/* IR -> AArch64 words.  Returns false if the host is not one this emitter can be
 * executed on (the words are still produced; the flag is for the caller's
 * benefit when it wants to *run* them). */
bool jit_emit_arm64(const IrBlock& blk, bool with_entry_exit,
                    std::vector<uint32_t>* out);

}  // namespace brain::arm926

#endif
