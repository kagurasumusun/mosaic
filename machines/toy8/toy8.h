/* toy8: a deliberately tiny second CPU module.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This is not an emulator of a real chip.  It exists to answer one question with
 * code instead of with a promise: *can a second processor run on this core?*
 * It has eight opcodes' worth of instruction set, five registers, no MMU and no
 * JIT, and it is built by the same Makefile, selected by the same
 * `--cpu toy8`, driven by the same C API, mapped into the same bus with the same
 * devices, and reported by the same `brain_stats_t`.
 *
 * It is also the template for a real one: a new CPU module is a `CpuCore`
 * implementation plus a `CpuModule` factory that registers itself (see
 * docs/MODULES.md).  The ISA below is written down in the header rather than in
 * a manual, because a reader who wants to add a CPU should be able to see the
 * whole contract in one screen.
 *
 *   registers   A, X, SP, PC, FLAGS (Z, N, C in bits 0, 1, 2)
 *   memory      a 16-bit little-endian address space, reached through the Bus
 *   entry       the PC starts wherever the loader put the image
 *
 *   0x00 NOP          0x01 LDA #i8      0x02 LDX #i8      0x03 STA a16
 *   0x04 LDA a16      0x05 STA (X)      0x06 LDA (X)      0x07 ADD #i8
 *   0x08 SUB #i8      0x09 CMP #i8      0x0A AND #i8      0x0B OR  #i8
 *   0x0C XOR #i8      0x0D INC A        0x0E DEC A        0x0F TAX
 *   0x10 TXA          0x11 INX          0x12 DEX          0x13 JMP a16
 *   0x14 JZ a16       0x15 JNZ a16      0x16 JC a16       0x17 JN a16
 *   0x18 JSR a16      0x19 RTS          0x1A PUSH A       0x1B POP A
 *   0x1C OUT a16      0x1D IN a16       0x1E BRK (stop)   0x1F HALT
 *
 * `OUT a16` stores A through the bus, so a device attached at that address sees
 * it; `IN a16` reads one.  Both are how the module reaches the world outside
 * itself - the same Bus every other module uses.
 */
#ifndef TOY8_TOY8_H
#define TOY8_TOY8_H

#include <memory>

#include "brainemu/module.h"

namespace brain::toy8 {

/* The module id: what `cpu_id` selects and `--cpu` takes. */
constexpr const char* kId = "toy8";

/* Register indices as the core sees them (get_reg/set_reg/CpuInfo). */
enum : u32 {
  kRegA = 0,
  kRegX = 1,
  kRegSP = 2,
  kRegPC = 3,
  kRegFlags = 4,
  kRegCount = 5,
};

/* FLAGS bits. */
enum : u8 { kZ = 1u << 0, kN = 1u << 1, kC = 1u << 2 };

const CpuModule& module();
void register_module();

}  // namespace brain::toy8

namespace brain {
/* Adds the toy8 demo CPU to the registry; called by the build's module list
 * (`builtin/builtin.cpp`).  See machines/arm926/arm926.h for the same line in a
 * machine that needs more than one file. */
void register_toy8_module();
}  // namespace brain

#endif  /* TOY8_TOY8_H */
