/* The ARM926EJ-S CPU module: what the core sees, and what a board sees.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Two layers live here, and the difference matters:
 *
 *   * `Cpu` (state.h) is the processor - the register file, CP15, the TLB, the
 *     interpreter and the JIT.  It is a plain struct with fixed, documented
 *     offsets because generated code addresses its fields directly; nothing in
 *     this header changes that.
 *   * `Core` (arm926.cpp) is the *module*: a CpuCore implementation that the
 *     machine can drive without knowing it is looking at an ARM.  It is a thin
 *     adapter - every call forwards, and the two places where it does more than
 *     forward (register indexing and `set_entry`) are where the module's own
 *     semantics have to be translated into the core's vocabulary.
 *
 * A board or a tool that wants ARM specifics (the CP15, the Thumb bit, the JIT
 * knobs) asks the module for them by name - `machine.cpu_as<arm926::Core>()` -
 * and gets the Cpu.  Everything else goes through the core.
 */
#ifndef ARM926_ARM926_H
#define ARM926_ARM926_H

#include <memory>

#include "arch/arm/cpu/classic/arm9/arm926ej-s/cpu/cpu.h"
#include "brainemu/module.h"

namespace brain::arm926 {

/* The module object, and the identity it registers under. */
const CpuModule& module();
const CpuInfo& info();

/* The concrete adapter: a Cpu plus the interface. */
class Core final : public CpuCore {
 public:
  Core() = default;

  const CpuInfo& info() const override;
  bool init(Bus& bus, const CpuOptions& opt) override;
  void reset() override;
  void sync() override;
  u64 run_until(u64 max_insns, u64 max_cycles) override;
  bool stop_requested() const override;
  void step() override;
  void request_stop() override;
  u64 insns() const override;
  u64 cycles() const override;
  u32 get_reg(u32 index) const override;
  bool set_reg(u32 index, u32 value) override;
  u32 pc() const override;
  void set_pc(u32 addr, bool alternate_state = false) override;
  void set_entry(u32 addr, bool alternate_state, u32 stack) override;
  u32 guest_read(u32 addr, u32 size) override;
  void guest_write(u32 addr, u32 value, u32 size) override;
  void set_irq(int level) override;
  void set_fiq(int level) override;
  int pending_exception() const override;
  const CpuStats& stats() const override;
  bool set_option(const char* key, const char* value) override;
  bool enable_jit(bool on) override;
  bool jit_active() const override;

  /* Everything ARM-specific: the CP15, the banks, the JIT, the cycle model. */
  Cpu& cpu() { return cpu_; }
  const Cpu& cpu() const { return cpu_; }

 private:
  Cpu cpu_{};
};

/* The module's id, so a caller can say which CPU it wants without a string. */
constexpr const char* kId = "arm926";

}  // namespace brain::arm926

namespace brain {
/* Adds this machine's CPU module to the registry; called by the build's module
 * list (`builtin/builtin.cpp`), which is the only place that says what a build
 * contains. */
void register_arm926_module();
}  // namespace brain

#endif  /* ARM926_ARM926_H */
