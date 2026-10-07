/* The shadow buffer: the one structure the dispatcher (plain C++) and generated
 * host code both touch.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Layout (byte offsets, little-endian - every host this JIT targets is):
 *
 *   +0x00  u32 regs[16]   guest R0..R15; R15 is an *output*: the address the
 *                         block stopped at, which the dispatcher turns back
 *                         into the pipeline view (cur/npc/regs[15]).
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
 *   +0x5C  u32 retired    instructions this entry actually retired (an abort
 *                         cuts a block short).
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
  SH_R15       = 0x3C,
  SH_CPSR      = 0x40,
  SH_EFLAGS    = 0x44,
  SH_VFLAG     = 0x48,
  SH_ABORT_PC  = 0x4C,
  SH_CPU       = 0x50,
  SH_MEM_FAST  = 0x58,
  SH_RETIRED   = 0x5C,
  SH_WORDS     = 24,          /* the whole buffer, in 32-bit words */
};

}  // namespace brain::arm926

#endif  /* BRAIN_JIT_ABI_H */
