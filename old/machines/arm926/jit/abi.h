/* The shadow buffer: the one structure the dispatcher (plain C++) and generated
 * host code both touch.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Layout (byte offsets, little-endian - every host this JIT targets is):
 *
 *   +0x00  u32 regs[16]   guest R0..R15; R15 is an *output*: the address the
 *                         block stopped at, which the dispatcher turns back
 *                         into the pipeline view (cur/npc/regs[15]).
 *                         R0-R12 are the allocator's window; R13/R14 are
 *                         ordinary input/output words the dispatcher keeps in
 *                         step (a block may use them as a base register or as
 *                         the link of a `bl`, but never as an operand), so
 *                         `regs[13]`/`regs[14]` mean exactly what they say.
 *   +0x40  u32 cpsr       the guest CPSR.  A backend may write it itself
 *                         (AArch64 does) or leave the flag bits to the
 *                         dispatcher (x86-64 does: it hands back an EFLAGS
 *                         image instead of unpacking four flag bits itself).
 *   +0x44  u32 eflags     in: the flags at entry, as an image ready to be
 *                         loaded with `popfq`; out: the reverse.
 *   +0x48  u32 vflag      guest V as 0/1 - kept out of the host flag register
 *                         on x86-64, because OF has no cheap writer while SF,
 *                         ZF and CF do (see emit_x64.cpp).
 *   +0x4C  u32 abort_pc   0 = the block ran to its end; anything else is the
 *                         guest address of an access that took a data abort,
 *                         which the dispatcher turns into cur/npc before the
 *                         exception is taken.
 *   +0x50  u64 cpu        the Cpu* the block belongs to (memory helpers and
 *                         the inline RAM fast path).
 *   +0x58  u32 mem_fast   1 while a guest load/store may bypass the helper:
 *                         dispatcher-computed (MMU off, RAM window, no
 *                         alignment checking, no SMC watch) and only read by
 *                         generated code - which is why one bit computed per
 *                         entry replaces five loads and branches per access.
 *                         A block that uses memory is entered only while this
 *                         is 1 *and* the window it baked still matches (see
 *                         jit_cache_enter), so the bit is also the guard.
 *   +0x5C  u32 retired    instructions this entry actually retired.  A block
 *                         that leaves early (an access it must not perform, a
 *                         taken branch) writes what it *did* retire here, so
 *                         the dispatcher's counters and budgets stay exact.
 *   +0x60  u32 cycles     cycles this entry actually charged, on the same
 *                         model the interpreter uses (kIssue per instruction
 *                         plus whatever the instruction's own IR op adds).
 *   +0x64  u32 branch     1 when the branch at the end of the block was taken, 0
 *                         when it was not.  A block whose branch never ran (it
 *                         left through a bail) leaves this stale, which the
 *                         dispatcher sees in `retired` and ignores.  It exists
 *                         because `stats.branch_taken/branch_not` are part of
 *                         what a run reports and both engines have to agree on
 *                         them: the interpreter counts a branch when it retires
 *                         one, and a block is the only thing that knows the
 *                         outcome of the one it performed.
 *   +0x68  u64 ram        the identity RAM window's host base (null when there
 *                         is none).  Loaded once per access for generated
 *                         code, so the window can move without re-emitting.
 *   +0x70  u64 codemap    the JIT's own byte-per-4KiB-page map of "some
 *                         translated block came from this page" (see
 *                         jit_cache_create).  A guest *store* a block wanted to
 *                         perform inline tests this first: a page that holds
 *                         code is exactly the page whose store has to go back
 *                         through the interpreter, whose write path is the one
 *                         that tells the cache to drop its blocks.  That keeps
 *                         the invariant `docs/MODULES.md` states - every writer
 *                         reports - true for inlined stores as well.
 *
 * Nothing here is ABI-stable across releases: it is the contract between two
 * files in this directory, and tests/test_jit_x64.cpp pins the offsets it cares
 * about against offsetof(Cpu, ...) so a layout change cannot silently move RAM
 * out from under a block.
 */
#ifndef BRAIN_JIT_ABI_H
#define BRAIN_JIT_ABI_H

#include <cstdint>

namespace brain::arm926 {

enum : uint32_t {
  SH_R0        = 0x00,
  SH_REG_BYTES = 4,
  SH_R13       = 0x34,
  SH_R14       = 0x38,
  SH_R15       = 0x3C,
  SH_CPSR      = 0x40,
  SH_EFLAGS    = 0x44,
  SH_VFLAG     = 0x48,
  SH_ABORT_PC  = 0x4C,
  SH_CPU       = 0x50,
  SH_MEM_FAST  = 0x58,
  SH_RETIRED   = 0x5C,
  SH_CYCLES    = 0x60,
  SH_BRANCH    = 0x64,
  SH_RAM       = 0x68,
  SH_CODEMAP   = 0x70,
  SH_WORDS     = 30,          /* the whole buffer, in 32-bit words */


  /* The registers the dispatcher copies in and out.  R0-R12 are the ones a
   * block may name as an operand; R13/R14 are copied because a block may touch
   * them as an address, and R15 is the exit address, so it is an output only. */
  SH_REGS_IN   = 15,
};

}  // namespace brain::arm926

#endif  /* BRAIN_JIT_ABI_H */
