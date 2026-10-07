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

#include <deque>
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
class Monitor;

/* One module a machine is asked to build, by id.  `base` is the window a device
 * wants, and `has_base == false` means "whatever the module's own default is"
 * (a device module knows where its peripheral normally sits).  A monitor
 * ignores both. */
struct ModuleSpec {
  const char* id = nullptr;
  u32 base = 0;
  bool has_base = false;

  ModuleSpec() = default;
  ModuleSpec(const char* i, u32 b = 0, bool hb = false) : id(i), base(b), has_base(hb) {}
};

/* One connection in a machine: a device's interrupt output to one of the CPU's
 * inputs, both by name.  A board (or a caller) declares the wires; the machine
 * delivers the levels.  Names are resolved when a level *changes*, not when the
 * wire is declared, so a board may name a device or an input that does not exist
 * yet - which is the order `Machine::init` builds things in anyway, and it also
 * makes `brain_add_wire` work on a live machine. */
struct WireSpec {
  const char* device = nullptr;   /* module id of the device, e.g. "timer"   */
  const char* output = nullptr;   /* its output's name; null/"" = the first  */
  const char* input = nullptr;    /* the CPU input's name, e.g. "irq"        */
  WireSpec() = default;
  WireSpec(const char* d, const char* o, const char* i) : device(d), output(o), input(i) {}
};

struct MachineConfig {
  /* Memory.  `ram_size` of 0 means "the caller maps its own memory" (a board
   * model, or a test that wants the whole space to itself). */
  u32 ram_base = 0;
  u32 ram_size = 32u << 20;
  const char* ram_name = "ram";

  /* Which CPU module to instantiate.  Null (or "") picks the first one
   * registered, which is the tree's primary target. */
  const char* cpu_id = nullptr;

  /* The rest of the machine, by id: devices to map and monitors to tell.  Both
   * arrays are small and fixed - a board is a handful of peripherals - and a
   * caller that wants more than `kMaxModules` builds them itself with
   * `add_device`/`add_monitor`. */
  static constexpr u32 kMaxModules = 16;
  ModuleSpec devices[kMaxModules];
  u32 device_count = 0;
  ModuleSpec monitors[kMaxModules];
  u32 monitor_count = 0;

  void add_device(const ModuleSpec& s) {
    if (device_count < kMaxModules) devices[device_count++] = s;
  }
  void add_monitor(const ModuleSpec& s) {
    if (monitor_count < kMaxModules) monitors[monitor_count++] = s;
  }

  /* Which board (a named machine preset) to start from, or null for "none".
   * `Machine::init` applies the board first and then the caller's own choices on
   * top: its `cpu_id` if it set one, its RAM, and every device/monitor/wire it
   * adds itself.  So `--board timer --device console` is a board plus one
   * peripheral, and a board can never take away what the caller asked for. */
  const char* board = nullptr;

  static constexpr u32 kMaxWires = 16;
  WireSpec wires[kMaxWires];
  u32 wire_count = 0;

  void add_wire(const WireSpec& w) {
    if (wire_count < kMaxWires) wires[wire_count++] = w;
  }
  void add_wire(const char* device, const char* output, const char* input) {
    add_wire(WireSpec(device, output, input));
  }

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
   * machine (attach a member, or a heap object the caller owns).  It also calls
   * `Device::on_attach`, so a device that has to reach the machine gets the
   * chance before anything can use it. */
  bool add_device(Device& d);
  u32 device_count() const { return (u32)devices_.size(); }
  Device* device(u32 i) { return i < devices_.size() ? devices_[i] : nullptr; }
  const Device* device(u32 i) const {
    return i < devices_.size() ? devices_[i] : nullptr;
  }

  /* The same, by module id - what `MachineConfig::devices[]` is shorthand for.
   * The machine owns what it creates; the device is mapped and reset like any
   * other.  A failure says which ids exist. */
  Device* add_device_by_id(const char* id, u32 base, bool has_base = true);

  /* Wiring: connect a device's interrupt output to one of the CPU's inputs.
   * Both ends are named, and neither module learns anything about the other -
   * which is what makes a device and a CPU independently reusable.  A wire to a
   * device, an output or an input that does not exist is refused with the ids and
   * names that do (see `err()`); `wires()` is what a board declared. */
  bool add_wire(const WireSpec& w);
  bool add_wire(const char* device_id, const char* output, const char* input) {
    return add_wire(WireSpec(device_id, output, input));
  }
  u32 wire_count() const { return (u32)wires_.size(); }
  const WireSpec& wire(u32 i) const { return wires_[i]; }
  /* The level a device's output is currently driving, and the delivery point the
   * machine installed on it (see Device::drive_irq). */
  int wire_level(u32 i) const;
  /* Deliver one level change to whatever the wire names.  Public because it is
   * the callback a device's `IrqNotify` gets; callers use the wires above. */
  static void deliver_irq(void* user, Device& d, u32 output, int level);
  /* Look one up by module id, whether the machine built it or the caller did. */
  Device* device_by_id(const char* id);
  Device* add_device_by_id(const ModuleSpec& spec) {
    return add_device_by_id(spec.id, spec.base, spec.has_base);
  }

  /* Monitors: observers the run loop reports every slice and every reset to
   * (see brainemu/core/monitor.h).  The machine keeps the pointer, so the
   * monitor must outlive it - a member, or an object the caller owns. */
  void add_monitor(Monitor& m);
  u32 monitor_count() const { return (u32)monitors_.size(); }
  Monitor* monitor(u32 i) { return i < monitors_.size() ? monitors_[i] : nullptr; }
  const Monitor* monitor(u32 i) const {
    return i < monitors_.size() ? monitors_[i] : nullptr;
  }
  Monitor* add_monitor_by_id(const char* id);
  Monitor* monitor_by_id(const char* id);

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
  /* The board, if the config named one: fills the effective config before
   * anything is built.  Unknown ids fail the machine and say what exists. */
  bool apply_board(const char* id, MachineConfig* eff);
  /* Look a wire's two ends up by name.  Fills `why` with what is missing. */
  bool resolve_wire(const WireSpec& w, u32* input_index, std::string* why);
  /* Point every device's interrupt notify at the machine.  Called by add_device
   * (both the direct and the by-id path), because a device can be added long
   * after init - before anything can raise a level. */
  void install_wiring(Device& d);
  void set_entry_from(const ImageInfo& img);
  /* Tick the devices with the cycles the run charged and tell the monitors
   * what happened - the one place a slice ends for everything but the CPU. */
  void end_slice(u64 retired);

  Bus bus_;
  std::unique_ptr<CpuCore> cpu_{};
  /* A wire's names are copied into storage the machine owns: a caller may hand a
   * temporary (`brain_add_wire` with a `std::string::c_str()`, a CLI parsing an
   * option), and the wire outlives the call that declared it.  `std::deque` so
   * the pointers stay valid as more are interned. */
  std::vector<WireSpec> wires_{};
  std::deque<std::string> wire_names_{};
  WireSpec intern_wire(const WireSpec& w);
  std::vector<Device*> devices_{};
  std::vector<Monitor*> monitors_{};
  /* The devices and monitors the machine itself created from module ids.  They
   * are destroyed here, which is after the bus released its regions and after
   * the CPU is gone - a device never outlives its window. */
  std::vector<std::unique_ptr<Device>> owned_devices_{};
  std::vector<std::unique_ptr<Monitor>> owned_monitors_{};
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
