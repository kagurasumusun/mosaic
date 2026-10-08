/* brain-emu -- ARMv5TEJ ISA: the environment contract (memory + coprocessor).
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The ISA engine (interp*.cpp) executes instructions against an IsaState and
 * asks the embedding core for everything that is not the instruction set:
 * translated memory accesses, instruction fetch, coprocessor register
 * transfers (MCR/MRC), taking a pending exception, JIT notification, and the
 * cache-sync hook.  IsaEnv is that contract: a table of function pointers the
 * core fills in once, each receiving the core's own object as `ctx`.
 *
 * Why callbacks and not an abstract base class: the interpreter is the hottest
 * code in the emulator and is compiled once into the library, so the call
 * target must be a plain pointer, and the ARM926EJ-S core (a single struct)
 * must stay the only object on the stack - no second state object, no vtable.
 * The dispatcher's per-block handoff already works this way (the JIT shadow,
 * jit/abi.h).
 *
 * The stats pointer is the engine's accounting sink: the interpreter retires
 * into the core's CpuStats so a run reports one set of numbers no matter which
 * engine executed what (the JIT updates the same struct directly).
 */
#ifndef BRAIN_ISA_ARMV5_MEMORY_H
#define BRAIN_ISA_ARMV5_MEMORY_H

#include "brainemu/cpu.h"
#include "brainemu/types.h"

namespace brain::arm::isa::armv5 {

/* Memory-access flags.  MF_SZ* select the access size (MF_SZ_M is the mask);
 * the rest describe the access to the memory system: a write, an instruction
 * fetch (which faults differently from a data read), an unaligned access the
 * ISA permits, and a SWP's read-modify-write (which must be atomic). */
enum : u32 {
  MF_WRITE = 1u << 0,
  MF_FETCH = 1u << 1,
  MF_SWP   = 1u << 2,
  MF_UNAL  = 1u << 3,
  MF_SZ_M  = 3u << 4,
  MF_SZ8   = 0u << 4,
  MF_SZ16  = 1u << 4,
  MF_SZ32  = 2u << 4,
};

/* The environment the ISA engine runs against.  Filled in by the core; every
 * callback receives `ctx` (the core's own object) as its first argument. */
struct IsaEnv {
  void* ctx;          /* the core object (a Cpu* for the ARM926EJ-S)           */
  CpuStats* stats;    /* engine accounting sink (never null)                   */

  /* ---- memory and translation ------------------------------------------
   * read/write perform a translated guest access and report faults by
   * raising a pending exception (see raise_dabort) and returning - the
   * engine checks the state's exectrap flag after each call.  fetch16/32
   * read the instruction stream.  prepare_fetch translates `cur` up front:
   * it returns false when the fetch itself aborts, leaving a pending
   * prefetch abort for the engine to take. */
  u32 (*mem_read)(void* ctx, u32 addr, u32 size, u32 flags);
  void (*mem_write)(void* ctx, u32 addr, u32 val, u32 size, u32 flags);
  u32 (*fetch16)(void* ctx, u32 addr);
  u32 (*fetch32)(void* ctx, u32 addr);
  bool (*prepare_fetch)(void* ctx);

  /* ---- coprocessor (MCR/MRC) -------------------------------------------
   * present() answers "does this core implement cp/opc1 (and the MCR2/MRC2
   * alias `second`)"; the engine raises UNDEF when it does not, or when the
   * access is attempted from User mode (a privileged operation).  write/read
   * transfer one register; read's return value goes to Rd (or to the PC when
   * Rd == R15).  A core with no coprocessor leaves these null and present()
   * is never consulted (the engine treats a null table as "no coprocessor"). */
  bool (*coproc_present)(void* ctx, u32 cp, u32 opc1, bool second);
  void (*coproc_write)(void* ctx, u32 cp, u32 crn, u32 opc1, u32 crm, u32 opc2,
                       u32 val);
  u32 (*coproc_read)(void* ctx, u32 cp, u32 crn, u32 opc1, u32 crm, u32 opc2);

  /* ---- exceptions and environment --------------------------------------
   * take_exception enters a pending exception now (the fetch path needs
   * this when the abort happens before any instruction retires).
   * notify_jit tells the core a translated-code-visible environment value
   * changed (an MSR), so it can drop stale blocks.  sync_caches asks the
   * core to recompute whatever it caches about the environment (the
   * ARM926EJ-S core recomputes its MMU/alignment/endianness view and the
   * l4 policy bit). */
  void (*take_exception)(void* ctx, u32 exc);
  void (*notify_jit)(void* ctx, const char* why);
  void (*sync_caches)(void* ctx);
};

}  // namespace brain::arm::isa::armv5

#endif  /* BRAIN_ISA_ARMV5_MEMORY_H */
