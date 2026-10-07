/* Thumb state decoder (ARMv5T: the 16-bit set plus the 32-bit BL/BLX pair).
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Semantics/encoding source: ARM DDI 0100I chapter A6 (formats) and A7
 * (per-instruction descriptions), plus the ARM926EJ-S TRM.  Every Thumb format
 * is mapped onto the *same* ArmDec the ARM decoder produces, so both states
 * execute through Cpu::exec_arm_dec() and cannot drift apart.
 *
 * `second` is the halfword that follows a 32-bit (0xF000-0xFFFF) instruction,
 * or 0 for a 16-bit one.  A 16-bit word whose bits[15:13] are 111 is always
 * the first half of a 32-bit instruction in ARMv5 -- which is the only way to
 * know the instruction size before decoding.
 */
#ifndef BRAIN_CORE_DECODE_THUMB_H
#define BRAIN_CORE_DECODE_THUMB_H

#include "core/decode.h"

namespace brain {

/* Decode one Thumb instruction into `d`; AOP_UNDEF marks "not an ARMv5T
 * encoding".  Every Thumb instruction is 16 bits wide in ARMv5T -- including
 * the three halfword forms that an assembler pairs up to make BL/BLX (A6.3.2,
 * A7.1.17): each halfword is separately defined, so the decoder handles them
 * one at a time and no look-ahead is needed. */
u32 decode_thumb(u32 hw, ArmDec* d);

/* The instruction size in Thumb state is always two bytes in ARMv5T. */
ALWAYS_INLINE u32 thumb_insn_size(u32 hw) { (void)hw; return 2; }

}  // namespace brain

#endif
