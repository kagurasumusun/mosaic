/* brain-emu -- the CPU contract of the core.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The core knows exactly this much about a processor: it has registers, it
 * executes instructions when asked, it can be interrupted, and it reports
 * counters.  Everything else - opcodes, exceptions, an MMU, an instruction set
 * that switches between ARM and Thumb - belongs to the CPU *module* that
 * implements this interface, and the module is free to be as specific as its
 * hardware is: the ARM926EJ-S module has a CP15, a TLB and a JIT, and the core
 * never learns about any of them.
 *
 * What matters for the shape of this interface is the cost of the calls.  A
 * `CpuCore` is only ever called at a *slice* boundary - run a budget of
 * instructions, step once, ask for a register, raise an input pin - never once
 * per guest instruction.  So the virtual calls here are noise, and a module's
 * inner interpreter or JIT stays a plain, non-virtual, monomorphic function
 * over its own state.  (Which is also why the interface does not try to model
 * "one instruction": an ISA with variable-length or parallel instructions is
 * exactly the case a per-instruction interface would punish.)
 */
#ifndef BRAINEMU_CPU_CPU_H
#define BRAINEMU_CPU_CPU_H

#include <cstdint>

#include "brainemu/types.h"

namespace brain {

class Bus;

/* A register index the core does not have.  Modules put the real index of
 * their PC, their flags register and their saved flags register here, which is
 * how a debugger or a loader can find them without knowing the ISA. */
enum : u32 { CPU_NO_REG = 0xFFFFFFFFu };

/* What the core can say about a CPU without knowing its instruction set. */
struct CpuInfo {
  const char* id = "";            /* module id, e.g. "arm926"                */
  const char* name = "";          /* "ARM926EJ-S"                            */
  const char* isa = "";           /* "ARMv5TEJ"                              */
  const char* description = "";
  u32 reg_count = 0;              /* registers get_reg/set_reg address       */
  u32 pc_index = CPU_NO_REG;      /* the program counter among them          */
  u32 status_index = CPU_NO_REG;  /* flags/mode word (ARM: CPSR)             */
  u32 saved_status_index = CPU_NO_REG;  /* saved copy (ARM: SPSR)            */
  u32 sp_index = CPU_NO_REG;      /* stack pointer, for image loading        */
  /* A second instruction set the PC can be in (ARM: Thumb, selected by bit 0
   * of an entry address).  A CPU without one ignores the flag everywhere. */
  bool alternate_state = false;
  const char* alternate_state_name = "";
};

/* Knobs the core itself exposes.  A module interprets the ones its hardware
 * has and ignores the rest, which is what keeps `brain_config_t` (and the CLI)
 * ISA-agnostic: `big_endian_data` means BE8 to an ARM module and "the endian
 * switch the guest set" to another, and a module with no such concept simply
 * does not look at it. */
struct CpuOptions {
  bool use_jit = false;
  bool trace = false;
  bool cycle_model = true;
  bool big_endian_data = false;
  bool high_vectors = false;
};

/* Counters, as the C API reports them.  Modules fill in what they have; a field
 * a CPU has no concept of stays zero, which is the honest answer. */
struct CpuStats {
  u64 insns_interp = 0, insns_jit = 0;
  /* No "links" field: this core's blocks return to the dispatcher instead of
   * being chained to each other, so there is nothing to count. */
  u64 blocks = 0, flushes = 0, smc_flushes = 0;
  u64 tlb_hits = 0, tlb_miss = 0, walks = 0, page_faults = 0;
  u64 exc[16] = {};               /* indexed by the module's exception ids  */
  u64 cycles_interp = 0, cycles_jit = 0;
  u64 code_bytes = 0;
  u64 branch_taken = 0, branch_not = 0;
  u64 helper_calls = 0;
};

/* The interface every CPU module implements.  Lifetime: created empty by the
 * module's factory, `init`ed once against a bus, then `reset` whenever the
 * machine resets. */
class CpuCore {
 public:
  virtual ~CpuCore() = default;

  virtual const CpuInfo& info() const = 0;

  /* Bind to the machine's address space and finish setup.  The bus outlives
   * the CPU. */
  virtual bool init(Bus& bus, const CpuOptions& opt) = 0;

  virtual void reset() = 0;

  /* Commit anything deferred before the caller observes or runs the machine
   * (the ARM module's MMU/cache state lives in CP15 registers the guest writes
   * with MSR, and this is where that becomes the memory view). */
  virtual void sync() {}

  /* Execution.  `run_insns(0)` and `run_cycles(0)` mean "until stopped"; both
   * return the number of instructions retired by this call. */
  virtual u64 run_insns(u64 n) = 0;
  virtual u64 run_cycles(u64 max_cycles) = 0;
  virtual void step() = 0;                 /* exactly one instruction        */
  virtual void request_stop() = 0;

  virtual u64 insns() const = 0;
  virtual u64 cycles() const = 0;

  /* Register access.  Indices are the module's own 0..reg_count-1 plus
   * `status_index` / `saved_status_index`; anything else is refused. */
  virtual u32 get_reg(u32 index) const = 0;
  virtual bool set_reg(u32 index, u32 value) = 0;
  virtual u32 pc() const = 0;
  virtual void set_pc(u32 addr, bool alternate_state = false) = 0;
  /* Point the CPU at the entry of a freshly loaded image.  Beyond the PC this
   * is where a module does the start-up state a program expects from its
   * loader: the ARM one seeds the stack pointer in the banks a boot would use
   * when `stack` is non-zero, and an ISA whose entry address carries an
   * instruction-set bit in bit 0 gets that from `alternate_state`. */
  virtual void set_entry(u32 addr, bool alternate_state, u32 stack) = 0;

  /* An access *as the guest would make it*: through the module's own view of
   * memory (the ARM module translates it, may abort, and sees its MMU), and
   * through the bus, so a device hook sees it.  This is what a debugger or a
   * `--dump` of an MMIO register wants.  `size` is 1, 2 or 4, and an access the
   * module cannot complete reads as zero / is dropped.  A module with no MMU of
   * its own implements these as a plain bus access - which is a two-line
   * function, and worth writing out rather than hiding in a default, because
   * whether a debug read translates addresses is the module's decision. */
  virtual u32 guest_read(u32 addr, u32 size) = 0;
  virtual void guest_write(u32 addr, u32 value, u32 size) = 0;

  /* Interrupt inputs, sampled at instruction boundaries.  IRQ and FIQ are the
   * ARM names for the two every core in this tree has so far, and they are what
   * `input 0` and `input 1` below mean; the generic surface is what a *board*
   * uses, because a board should not have to know whether the CPU it wired is an
   * ARM.  A CPU with a different interrupt model (a vector of 32 lines, a
   * priority controller, an NMI) overrides the generic three and leaves the
   * ARM-shaped pair as the alias a caller that knows it has an ARM still uses. */
  virtual void set_irq(int level) = 0;
  virtual void set_fiq(int level) = 0;

  /* The generic view of the same thing: `input_name(i)` is the name a board
   * wires to ("irq", "fiq"), and `set_input` drives one.  Returns false for an
   * input the CPU does not have, so a mis-wired board can be reported instead of
   * silently ignored. */
  virtual u32 input_count() const { return 2; }
  virtual const char* input_name(u32 i) const {
    return i == 0 ? "irq" : i == 1 ? "fiq" : "";
  }
  virtual bool set_input(u32 i, int level) {
    if (i == 0) { set_irq(level); return true; }
    if (i == 1) { set_fiq(level); return true; }
    return false;
  }

  /* Non-zero when an exception is being taken (the exception ids are the
   * module's; the C API maps the ones it knows). */
  virtual int pending_exception() const = 0;

  virtual const CpuStats& stats() const = 0;

  /* Optional capabilities.  `set_option` is the escape hatch that keeps the
   * core's config surface from growing an ISA-specific field per idea: keys
   * are documented by the module, and an unknown key returns false. */
  virtual bool set_option(const char* key, const char* value) {
    (void)key; (void)value; return false;
  }
  virtual bool enable_jit(bool on) { (void)on; return false; }
  virtual bool jit_active() const { return false; }
};

}  // namespace brain

#endif  /* BRAINEMU_CPU_CPU_H */
