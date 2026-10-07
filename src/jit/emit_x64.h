/* x86-64 code generation for the JIT's IR.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A block is `void host_block(u32* shadow)`: it loads the guest registers the
 * block actually mentions out of the shadow buffer, runs the ops entirely in
 * host registers, and stores back only what it changed.  Nothing about the
 * guest's memory is touched here - a block is a straight-line computation, and
 * the dispatcher (jit/cache.cpp) is what turns the shadow buffer back into the
 * machine state the interpreter would have had.
 *
 * Register plan (System V, chosen so the whole block is one leaf function):
 *
 *   RBP  the shadow pointer.  Callee-saved, so it survives a helper call, and
 *        every guest load/store is one [rbp+disp] away from it.
 *   R11  scratch: the materialised second operand, and the carry stash of the
 *        flag paths.  Never allocated to a guest register.
 *   R15  the guest's V flag, 0/1 in r15d.  See below.
 *   pool RAX RCX RDX RSI RDI R8 R9 R10 RBX R12 R13 R14 - the registers a guest
 *        register may live in.
 *
 * The three-way split of the flags is the whole trick of this backend, and it
 * comes from which host flags are cheap to write:
 *
 *   N, Z, C live in the host SF/ZF/CF for the duration of a block.  Every
 *   operation ARM needs on them exists on x86 with the same meaning: add/sub
 *   produce the carry ARM wants out of the adder, `shl/shr/sar/ror/rcr` produce
 *   exactly the shifter carry ARM's logical operations take (bit 32-n / n-1 / 0
 *   of the operand), and `and/or/xor/test` produce N and Z the same way.
 *
 *   V lives in r15d, because x86 has no cheap writer for OF: `seto` is one
 *   instruction, while *restoring* OF from a saved bit would take a pushfq/
 *   popfq pair.  Keeping V out of RFLAGS means a logical operation (which
 *   clobbers CF and OF) only has to restore C - one `bt` - and never touches V.
 *
 *   AArch64 could keep V inside NZCV because MSR/MRS move all four bits at
 *   once; on x86-64 that would cost a pushfq/popfq per flag-setting logical
 *   operation, which is the common case in real ARM code.
 *
 * The other half of the design is that flags are only materialised when they
 * are live.  `jit_emit_x64` runs a backward liveness pass over the block (the
 * IR has no branches, so "live" is a linear property), which decides for each
 * instruction whether its flags have to be correct afterwards.  An `adds` whose
 * result nothing reads is emitted as a bare `add`; a chain of logical
 * operations in front of a `cmp` is emitted without any flag preservation at
 * all - they cannot be observed, so nothing has to preserve them.  When they
 * are live, an operation that must not write them uses the flag-free forms
 * (`mov`, `lea`, `not`) where it can and a pushfq/popfq pair where it cannot.
 */
#ifndef BRAIN_JIT_EMIT_X64_H
#define BRAIN_JIT_EMIT_X64_H

#include <cstdint>
#include <vector>

#include "jit/ir.h"

namespace brain {

/* What the dispatcher has to do around a block, per backend.  The default
 * ({0, 0}) is "the block neither reads nor writes the flags", which is exactly
 * what the AArch64 emitter's own prologue/epilogue already handle: it loads and
 * stores the CPSR word in the shadow itself. */
struct JitBlockMeta {
  uint8_t entry_flags = 0;   /* 1: the block reads shadow[SH_EFLAGS]/[SH_VFLAG] */
  uint8_t exit_flags = 0;    /* 0: flags untouched; 1: an EFLAGS image + V was
                              *    left in the shadow for the dispatcher to
                              *    merge; 2: the CPSR word was written by the
                              *    block itself (AArch64) */
};

/* Translates `blk` into x86-64.  With `with_entry_exit` the block gets the
 * prologue/epilogue that make it callable as `void (*)(u32*)`.  Returns false -
 * with `*out` left empty - if any instruction is outside the emitter's set, so
 * that a caller can never install a partial translation. */
bool jit_emit_x64(const IrBlock& blk, bool with_entry_exit, std::vector<uint8_t>* out,
                  JitBlockMeta* meta);

/* Per-instruction acceptance, used by the front end (jit/frontend.cpp) to decide
 * how far a block may be translated: a block only ever contains instructions
 * this backend emits. */
bool jit_x64_accepts(const IrInsn& in);

}  // namespace brain

#endif  /* BRAIN_JIT_EMIT_X64_H */
