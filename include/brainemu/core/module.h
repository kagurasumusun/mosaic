/* brain-emu -- modules: the one extension point of the core.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The emulator is one core plus modules.  The core owns the address space, the
 * machine's lifetime and the C API; a module owns one piece of the emulated
 * world and registers itself under an id:
 *
 *   cpu      a processor: registers, an instruction set, its own timing
 *   device   something mapped into the address space (UART, timer, exit port)
 *   loader   an image format (ELF, raw, a firmware container)
 *   monitor  an observer of the running machine (tracer, profiler, gdb stub)
 *
 * A CPU module is the only kind the core *drives* (it executes when asked);
 * devices and monitors are called by the bus and the run loop, and loaders fill
 * memory.  Adding a processor therefore touches no core file - which is the
 * whole point of the split: the ARM926EJ-S is the first CPU module, not the
 * shape of the emulator.
 *
 * All four kinds are registry-managed, and all four are created through a
 * factory here rather than by the core naming a class: a machine is assembled
 * out of *ids* (`MachineConfig::cpu_id`, `devices[]`, `monitors[]`, and the
 * loader a `load_image` call probes for), so a build can contain a peripheral
 * the core has never heard of and a caller can select it by name.  The two
 * adapters the core itself ships - the exit port and the console - are modules
 * in exactly that sense (src/modules/console/, src/modules/exit/).
 *
 * Registration is explicit rather than magically static-initialised, because
 * this library is a static archive and an unreferenced object file inside one is
 * simply not linked in: `register_builtin_modules()` (src/modules/builtin.cpp)
 * is the list the core calls, and a build that leaves a module out says so once,
 * in that file, instead of dropping it silently.
 */
#ifndef BRAINEMU_CORE_MODULE_H
#define BRAINEMU_CORE_MODULE_H

#include <memory>

#include "brainemu/cpu/cpu.h"
#include "brainemu/types.h"

namespace brain {

enum class ModuleKind : u32 {
  Cpu = 0,
  Device,
  Loader,
  Monitor,
  Count,
};

const char* module_kind_name(ModuleKind k);

struct ModuleInfo {
  const char* id = "";             /* "arm926": the id a caller selects by    */
  const char* name = "";           /* human-readable name                     */
  ModuleKind kind = ModuleKind::Cpu;
  const char* description = "";
};

/* A CPU module: a factory plus the identity of what it builds.  One instance
 * exists per module for the life of the process; the cores it creates are
 * independent of it and of each other. */
class CpuModule {
 public:
  virtual ~CpuModule() = default;
  virtual const ModuleInfo& info() const = 0;
  virtual std::unique_ptr<CpuCore> create() const = 0;
};

/* A device module: a factory for one instance of a peripheral, placed at the
 * base address the caller chooses.  `default_base()` is what a caller that has
 * no opinion gets (the address the adapter normally sits at, so `--device
 * console` alone works and `--device console@0x40000000` moves it).  A module
 * whose window is not fixed can still ignore the argument; the core only ever
 * passes it through. */
class Device;
class DeviceModule {
 public:
  virtual ~DeviceModule() = default;
  virtual const ModuleInfo& info() const = 0;
  virtual std::unique_ptr<Device> create(u32 base) const = 0;
  virtual u32 default_base() const { return 0; }
};

/* A monitor module: a factory for one observer (see core/monitor.h).  A monitor
 * with state of its own - a profiler, a coverage collector - is created per
 * machine, and the machine keeps it. */
class Monitor;
class MonitorModule {
 public:
  virtual ~MonitorModule() = default;
  virtual const ModuleInfo& info() const = 0;
  virtual std::unique_ptr<Monitor> create() const = 0;
};

/* A loader module: `probe` claims an image by content and `load` places it.
 * Declared here and defined in brainemu/loader/loader.h, so a CPU module never
 * has to pull the loader interface in. */
class Loader;

/* The process-wide registry.  Not thread-safe by design: modules register
 * during start-up, before any machine exists. */
class ModuleRegistry {
 public:
  static ModuleRegistry& instance();

  void add_cpu(const CpuModule* m);
  const CpuModule* cpu_by_id(const char* id) const;
  const CpuModule* cpu_by_index(u32 i) const;
  u32 cpu_count() const;

  void add_loader(const Loader* l);
  const Loader* loader_by_id(const char* id) const;
  /* The loader that claims this image, or null: ELF by magic, raw as the
   * fallback the caller has to ask for explicitly. */
  const Loader* loader_by_probe(const void* data, u64 size) const;
  u32 loader_count() const;

  void add_device(const DeviceModule* m);
  const DeviceModule* device_by_id(const char* id) const;
  const DeviceModule* device_by_index(u32 i) const;
  u32 device_count() const;

  void add_monitor(const MonitorModule* m);
  const MonitorModule* monitor_by_id(const char* id) const;
  const MonitorModule* monitor_by_index(u32 i) const;
  u32 monitor_count() const;

  /* Every module kind, for a `--list-modules` in a tool or a GUI picker.  The
   * order is the kind order above (all CPUs, then loaders, devices, monitors),
   * so a listing groups itself. */
  u32 count() const;
  const ModuleInfo* info(u32 i) const;
  /* The ids of one kind, joined for an error message ("console, exit").  Used
   * when an unknown id is refused, so the message can say what exists. */
  const char* ids(ModuleKind kind) const;

 private:
  ModuleRegistry() = default;
  static constexpr u32 kMaxCpus = 16;
  static constexpr u32 kMaxLoaders = 16;
  static constexpr u32 kMaxDevices = 16;
  static constexpr u32 kMaxMonitors = 16;
  const CpuModule* cpus_[kMaxCpus] = {};
  u32 ncpus_ = 0;
  const Loader* loaders_[kMaxLoaders] = {};
  u32 nloaders_ = 0;
  const DeviceModule* devices_[kMaxDevices] = {};
  u32 ndevices_ = 0;
  const MonitorModule* monitors_[kMaxMonitors] = {};
  u32 nmonitors_ = 0;
  /* The last `ids()` answer, so the accessor can return a `const char*`.  One
   * buffer per instance and single-threaded by design, like the registry. */
  mutable char idbuf_[256] = {};
};

/* Called once by the core (Machine::init) - see the note at the top. */
void register_builtin_modules();

}  // namespace brain

#endif  /* BRAINEMU_CORE_MODULE_H */
