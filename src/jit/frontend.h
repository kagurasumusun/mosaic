/* Guest ARM -> JIT IR.
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
 * Everything else ends the block: a branch, a load or store (they need the MMU and
 * the abort path, and a compiled block must not grow a second copy of them), MRS or
 * MSR, the multiply space, anything touching R13-R15 (banked, or the PC itself), a
 * shift amount taken from a register, and a 32-bit shift - what ARM state spells
 * `LSR #0` and `ASR #0`, and which AArch64's five-bit shift field cannot say.
 * ADCS, SBCS and RSC are refused as a matter of policy: chaining a carry correctly
 * needs the flags as of *before* the operation, which costs a snapshot per
 * instruction and is not worth it while the interpreter is close behind.
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

#include "jit/ir.h"

namespace brain {

class Cpu;

/* Translates at most `max_words` guest words starting at `pc`.  Returns the
 * number consumed (0 if the first word is not translatable) and fills `out`.
 * `stop` in the block says why it ended. */
uint32_t jit_translate_arm_block(Cpu& cpu, uint32_t pc, uint32_t max_words,
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

}  // namespace brain

#endif
