/* brain-emu -- the core: one machine, one bus, one CPU, any number of modules.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A Machine is what is left when everything specific to a processor is taken
 * out and put in a module:
 *
 *   * the address space (Bus) and the RAM window;
 *   * the CPU slot - one CPU module, chosen by id at init;
 *   * the devices attached to the address space, and the clock they are ticked
 *     from;
 *   * the run loop's shape: reset, run a budget, step, stop, and the counters
 *     that come out of it;
 *   * the C API's handle (api.cpp is a thin translation of these calls).
 *
 * Nothing here knows an opcode, a CPSR or a Thumb bit.  The CPU module does the
 * executing; the machine decides *when* to execute, what else is on the bus, and
 * how the world around the CPU sees its state.
 *
 * Every call is either a slice boundary (run/step/reset) or user-facing
 * (registers, memory, stats), so the one virtual call per slice is free.
 */
#ifndef BRAINEMU_CORE_MACHINE_H
#define BRAINEMU_CORE_MACHINE_H

#include <memory>
#include <string>
#include <vector>

#include "brainemu/core/bus.h"
#include "brainemu/core/module.h"
#include "brainemu/cpu/cpu.h"
#include "brainemu/loader/loader.h"
#include "brainemu/types.h"

namespace brain {

class Device;

struct MachineConfig {
  /* Memory.  `ram_size` of 0 means "the caller maps its own memory" (a board
   * model, or a test that wants the whole space to itself). */
  u32 ram_base = 0;
  u32 ram_size = 32u << 20;
  const char* ram_name = "ram";

  /* Which CPU module to instantiate.  Null (or "") picks the first one
   * registered, which is the tree's primary target. */
  const char* cpu_id = nullptr;

  bool use_jit = false;
  bool trace = false;
  bool cycle_model = true;
  bool big_endian_data = false;
  bool high_vectors = false;
};

class Machine {
 public:
  Machine();
  ~Machine();

  Machine(const Machine&) = delete;
  Machine& operator=(const Machine&) = delete;

  /* Builds the machine: registers the built-in modules (once), maps RAM,
   * creates the CPU module and resets.  Returns false with `last_error()`
   * set - an unknown cpu id says which ids exist. */
  bool init(const MachineConfig& cfg);

  /* Re-create the machine from the last configuration. */
  bool reinit();
  const MachineConfig& config() const { return cfg_; }

  void reset();

  Bus& bus() { return bus_; }
  const Bus& bus() const { return bus_; }

  CpuCore& cpu() { return *cpu_; }
  const CpuCore& cpu() const { return *cpu_; }
  const CpuInfo& cpu_info() const { return cpu_->info(); }
  bool has_cpu() const { return cpu_ != nullptr; }

  /* Execution.  `run_*` return `this->cpu().insns()` deltas, and tick the
   * devices with the cycles they charged. */
  u64 run_insns(u64 n);
  u64 run_cycles(u64 max_cycles);
  void step();
  void stop();

  /* Modules.  `add_device` maps the device through its own window; the machine
   * keeps the pointer to reset and tick it, so the device must outlive the
   * machine (attach a member, or a heap object the caller owns). */
  bool add_device(Device& d);
  u32 device_count() const { return (u32)devices_.size(); }
  Device* device(u32 i) { return i < devices_.size() ? devices_[i] : nullptr; }

  /* Images.  `load_image` picks a loader by content (and falls back to a raw
   * load at `addr`), then points the CPU at the entry: PC, instruction-set bit
   * from bit 0, and for an ELF image a stack at the top of RAM. */
  int load_raw(const void* data, size_t size, u32 addr);
  int load_elf32(const void* data, size_t size, ImageInfo* info);
  int load_image(const void* data, size_t size, u32 addr, ImageInfo* info);

  /* The last failure: what `init`/`load_*` refused, and why.  A machine's
   * error is set by the machine - a caller that wants to report its own
   * failure has its own string. */
  const std::string& err() const { return err_; }
  const char* last_error() const { return err_.empty() ? "no error" : err_.c_str(); }

 private:
  bool build_cpu(const char* id);
  void set_entry_from(const ImageInfo& img);

  Bus bus_;
  std::unique_ptr<CpuCore> cpu_{};
  std::vector<Device*> devices_{};
  MachineConfig cfg_{};
  u64 device_cycles_ = 0;
  std::string err_{};
};

/* --------------------------------------------------------------- helpers -- */
/* The id a default machine is built around: the first CPU module registered
 * (the build's primary target, which is the ARM926EJ-S here).  Used by the C
 * API and by tools that take a --cpu id. */
const char* default_cpu_id();

}  // namespace brain

#endif  /* BRAINEMU_CORE_MACHINE_H */
