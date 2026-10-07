/* brain-emu -- the core's test suite.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The ARM tests prove the ARM926 machine works.  These prove the *core* is a
 * core: that a machine the core knows nothing about - a CPU module with no
 * instruction set, a loader that is not ELF, a device that is a pair of callbacks -
 * runs on it, and that the bus, the registry, the run loop, the devices and the C
 * API never needed to know which one they were driving.
 *
 * Everything here is written against `brainemu/*.h` alone, so it is also the
 * statement of what the foundation promises without any component: the suite
 * compiles and passes with every machine removed except the fakes below.
 */
#include <brainemu/brainemu.h>

#include "brainemu/bus.h"
#include "brainemu/cpu.h"
#include "brainemu/device.h"
#include "brainemu/loader.h"
#include "brainemu/machine.h"
#include "brainemu/module.h"
#include "brainemu/monitor.h"
#include "testkit/test_support.h"

#include <cstdio>
#include <cstring>
#include <memory>
#include <string>

/* ------------------------------------------------------------ a fake CPU -- */

/* A CPU module that does nothing at all except count.  It has no instruction
 * set, no memory view and no exceptions, which makes it the sharpest test of
 * one property: that `run_insns`, `step`, `stop`, `reset`, the device clock and
 * the counters are the *machine's* job and not a CPU's.  It also proves the
 * interface is implementable without pulling in anything ARM-specific - this
 * file compiles with the ARM module present but never names it. */
class BlipCore final : public CpuCore {
 public:
  const CpuInfo& info() const override {
    static const CpuInfo k = [] {
      CpuInfo i;
      i.id = "blip";
      i.name = "blip";
      i.isa = "none: one counter, incremented by a nanosecond of host time";
      i.reg_count = 2;
      i.pc_index = 1;
      i.description = "a test CPU with no instruction set at all";
      return i;
    }();
    return k;
  }
  bool init(Bus& bus, const CpuOptions& opt) override {
    bus_ = &bus;
    keep_ = opt.cycle_model;   /* the generic option reaches the module */
    reset();
    return true;
  }
  void reset() override { steps_ = 0; cycles_ = 0; stopped_ = false; }
  /* The fake honours both bounds at once, like a real module: it stops at the
   * first one reached (0 = no bound), and a stop ends the call.  A stop request
   * belongs to the run it interrupts, so a run starts by clearing it - that is
   * what makes a stopped machine runnable again without a reset. */
  u64 run_until(u64 max_insns, u64 max_cycles) override {
    const u64 i0 = steps_, c0 = cycles_;
    stopped_ = false;
    while (!stopped_) {
      if (max_insns && steps_ - i0 >= max_insns) break;
      if (max_cycles && cycles_ - c0 >= max_cycles) break;
      step();
    }
    return steps_ - i0;
  }
  bool stop_requested() const override { return stopped_; }
  void step() override {
    ++steps_;
    ++cycles_;
    stats_.insns_interp = steps_;
    stats_.cycles_interp = cycles_;
  }
  void request_stop() override { stopped_ = true; }
  u64 insns() const override { return steps_; }
  u64 cycles() const override { return cycles_; }
  u32 get_reg(u32 index) const override {
    if (index == 0) return (u32)steps_;
    if (index == 1) return 0x4444u;
    return 0;
  }
  bool set_reg(u32 index, u32 value) override {
    (void)value;
    return index < 2;
  }
  u32 pc() const override { return pc_; }
  void set_pc(u32 addr, bool) override { pc_ = addr; }
  void set_entry(u32 addr, bool, u32) override { entry_ = addr; }
  u32 guest_read(u32, u32) override { return 0; }
  void guest_write(u32, u32, u32) override {}
  void set_irq(int level) override { irq_ = level; }
  void set_fiq(int level) override { fiq_ = level; }
  int pending_exception() const override { return 0; }
  const CpuStats& stats() const override { return stats_; }
  bool set_option(const char* key, const char* value) override {
    if (key && !std::strcmp(key, "squeak")) { squeak_ = value && *value; return true; }
    return false;
  }

  Bus* bus_ = nullptr;
  u32 pc_ = 0;
  bool keep_ = true, squeak_ = false;
  int irq_ = 0, fiq_ = 0;
  u32 entry_ = 0;
  u64 steps_ = 0, cycles_ = 0;
  bool stopped_ = false;
  CpuStats stats_{};
};

class BlipModule final : public CpuModule {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo k = {"blip", "blip", ModuleKind::Cpu,
                                 "the test CPU declared in tests/test_module.cpp"};
    return k;
  }
  std::unique_ptr<CpuCore> create() const override {
    return std::unique_ptr<CpuCore>(new BlipCore());
  }
};

const char* kBlipId = "blip";

/* A monitor: it observes, it never executes.  One instance does the counting a
 * test can check, the other stops the machine the moment it sees a slice. */
struct Watch {
  u32 slices = 0;
  u64 retired = 0;
  u64 last = 0;
  u32 resets = 0;
};

class CountingMonitor final : public Monitor {
 public:
  explicit CountingMonitor(Watch* w) : w_(w) {
    info_.id = "counter";
    info_.name = "slice counter";
    info_.kind = ModuleKind::Monitor;
    info_.description = "counts slices and retired instructions";
  }
  const ModuleInfo& info() const override { return info_; }
  void on_slice(Machine& m, u64 retired) override {
    ++w_->slices;
    w_->retired += retired;
    w_->last = retired;
    if (stop_at_ && w_->slices >= stop_at_) m.stop();
  }
  void on_reset(Machine&) override { ++w_->resets; }
  void stop_after(u32 slices) { stop_at_ = slices; }

 private:
  Watch* w_;
  u32 stop_at_ = 0;
  ModuleInfo info_{};
};

/* --------------------------------------------------------------- sections -- */

void test_registry() {
  std::printf("[module registry]\n");
  register_builtin_modules();
  const ModuleRegistry& reg = ModuleRegistry::instance();

  /* The build's primary target is the ARM926: a default machine is an
   * ARM926EJ-S, and asking for it by name finds the same module. */
  CHECK(reg.cpu_count() >= 2);
  CHECK(std::strcmp(reg.cpu_by_index(0)->info().id, "arm926") == 0);
  CHECK(reg.cpu_by_id("arm926") == reg.cpu_by_index(0));
  CHECK(std::strcmp(default_cpu_id(), "arm926") == 0);

  /* The second CPU is the demo ISA, and both loaders are present. */
  bool saw_toy8 = false;
  for (u32 i = 0; i < reg.cpu_count(); ++i)
    if (!std::strcmp(reg.cpu_by_index(i)->info().id, "toy8")) saw_toy8 = true;
  CHECK(saw_toy8);
  CHECK(reg.loader_by_id("elf32") != nullptr);
  CHECK(reg.loader_by_id("raw") != nullptr);
  CHECK(reg.loader_by_probe("garbage", 7) == nullptr);   /* raw is opt-in */

  /* The C API sees the same list, with the kind spelled out. */
  CHECK(brain_module_count() == reg.count());
  bool saw_loader_kind = false, saw_cpu_kind = false;
  for (u32 i = 0; i < brain_module_count(); ++i) {
    if (!std::strcmp(brain_module_kind(i), "loader")) saw_loader_kind = true;
    if (!std::strcmp(brain_module_kind(i), "cpu")) saw_cpu_kind = true;
  }
  CHECK(saw_cpu_kind);
  CHECK(saw_loader_kind);
  CHECK(std::strcmp(brain_default_cpu_id(), "arm926") == 0);
}

void test_unknown_cpu() {
  std::printf("[unknown cpu id]\n");
  Machine m;
  MachineConfig c;
  c.ram_size = 1u << 16;
  c.cpu_id = "no-such-cpu";
  CHECK(!m.init(c));
  CHECK(!m.has_cpu());
  /* The error names the id that failed *and* the ids that would have worked:
   * a typo has to be self-diagnosing. */
  const std::string err = m.last_error();
  CHECK(err.find("no-such-cpu") != std::string::npos);
  CHECK(err.find("arm926") != std::string::npos);
  CHECK(err.find("toy8") != std::string::npos);

  /* And a good id works on a fresh machine, so the failure above left nothing
   * behind. */
  Machine good;
  MachineConfig g;
  g.ram_size = 1u << 16;
  g.cpu_id = "toy8";
  CHECK(good.init(g));
  CHECK(std::strcmp(good.cpu_info().id, "toy8") == 0);
}

void test_fake_cpu_module() {
  std::printf("[a CPU module written in the test]\n");
  static const BlipModule kBlip;
  ModuleRegistry::instance().add_cpu(&kBlip);

  Machine m;
  MachineConfig c;
  c.ram_size = 0xC000u;       /* the recorder device sits above RAM          */
  c.cpu_id = kBlipId;
  CHECK(m.init(c));
  if (!m.has_cpu()) return;
  CHECK(std::strcmp(m.cpu_info().id, "blip") == 0);
  CHECK(m.cpu_info().reg_count == 2);

  Recorder rec;
  RecorderDevice dev(&rec);
  CHECK(m.add_device(dev));

  /* The machine owns the run loop: a CPU that only counts still respects the
   * budget, the cycle budget, the device clock and stop(). */
  CHECK(m.run_insns(100) == 100);
  CHECK(m.cpu().insns() == 100);
  m.step();
  CHECK(m.cpu().insns() == 101);
  CHECK(m.run_cycles(5) == 5);
  CHECK(rec.ticks == 106);              /* the device saw every cycle       */

  /* Counters travel through the generic struct: the module wrote two fields and
   * the machine reports them, without knowing what "insns_interp" means. */
  CHECK(m.cpu().stats().insns_interp == 106);
  CHECK(m.cpu().stats().cycles_interp == 106);

  /* The generic config reached the module's init(). */
  BlipCore* blip = dynamic_cast<BlipCore*>(&m.cpu());
  CHECK(blip != nullptr);
  CHECK(blip && blip->keep_);

  /* A stop request belongs to the run it interrupts: `m.stop()` outside a run
   * then `run_insns` runs, because the call clears the flag and starts fresh.
   * That is what makes a stopped machine runnable again without a reset - and a
   * caller that wants it to stay stopped keeps its own flag (the monitors test
   * above does exactly that, and the CPU's own is cleared by `reset`). */
  m.stop();
  CHECK(m.cpu().stop_requested());
  CHECK(m.run_insns(1000) == 1000);
  CHECK(!m.cpu().stop_requested());

  /* A raw load moves the PC (nothing more: no stack, no state bit)... */
  const uint8_t img[] = {0, 0, 0, 0};
  CHECK(m.load_raw(img, sizeof img, 0x1234u) == 0);
  CHECK(blip && blip->pc_ == 0x1234u);
  /* ...and set_entry is the seam for everything a loader adds on top. */
  m.cpu().set_entry(0x2000u, false, 0x9000u);
  CHECK(blip && blip->entry_ == 0x2000u);

  /* A stop request from a previous run does not survive a reset. */
  m.reset();
  CHECK(m.run_insns(3) == 3);

  /* Two machines of different CPUs at once: nothing in the core is global but
   * the registry. */
  Machine arm;
  MachineConfig ac;
  ac.ram_size = 1u << 16;
  CHECK(arm.init(ac));
  CHECK(std::strcmp(arm.cpu_info().id, "arm926") == 0);
  CHECK(m.run_insns(2) == 2);
  CHECK(arm.cpu().insns() == 0);
}

void test_monitors() {
  std::printf("[monitors]\n");
  Machine m;
  MachineConfig c;
  c.ram_size = 1u << 16;
  c.cpu_id = "toy8";
  CHECK(m.init(c));
  if (!m.has_cpu()) return;

  Watch w;
  CountingMonitor mon(&w);
  m.add_monitor(mon);
  CHECK(m.monitor_count() == 1);
  CHECK(m.monitor(0) == &mon);
  CHECK(m.monitor(1) == nullptr);
  CHECK(std::strcmp(mon.info().kind == ModuleKind::Monitor
                        ? module_kind_name(mon.info().kind) : "", "monitor") == 0);

  /* Five instructions (LDA #7, ADD #7, INC A, NOP, HALT), one slice per call:
   * the monitor sees the slice count and the retired count, and its view of the
   * machine is readable - that is what a monitor is for. */
  const u8 prog[] = {0x01, 0x07, 0x07, 0x07, 0x0D, 0x00, 0x1F};
  CHECK(m.load_raw(prog, sizeof prog, 0x100) == 0);
  CHECK(m.run_insns(3) == 3);
  CHECK(w.slices == 1);
  CHECK(w.retired == 3);
  CHECK(w.last == 3);
  CHECK(m.cpu().get_reg(0) == 15);             /* 7 + 7 + 1, read back here   */

  /* step() is a slice of one instruction.  Then the HALT retires, which is a
   * slice - and the run then asks for another one, which retires nothing, so it
   * is not a slice at all: a slice is a unit of work the monitor is told about,
   * and a CPU that cannot get anywhere ends the run instead of reporting empty
   * slices for ever.  The HALT count is the monitor's, and it did not move. */
  m.step();
  CHECK(w.slices == 2 && w.last == 1);
  CHECK(m.run_insns(100) == 1);                /* the HALT                   */
  CHECK(w.slices == 3 && w.last == 1);
  CHECK(m.run_insns(100) == 0);                /* halted: no work, no slice  */
  CHECK(w.slices == 3 && w.last == 1);

  /* A monitor may stop the machine - a watchpoint, a breakpoint, a tracer's
   * "that is enough".  The stop is seen at the next slice boundary, so the slice
   * that tripped it is reported in full and the call ends there. */
  m.reset();
  CHECK(w.resets == 1);
  CHECK(m.cpu().insns() == 0);
  const u32 before = w.slices;
  mon.stop_after(before + 1);
  CHECK(m.run_insns(2) == 2);
  CHECK(w.slices == before + 1);
  /* The monitor's condition still holds, so every call trips it at its first
   * boundary: the call runs one slice (the toy-8 module charges several cycles
   * per instruction, so a cycle-bounded slice is a couple of hundred of them)
   * and then stops, having retired less than it was asked for. */
  const u64 st2 = m.run_insns(1000);
  CHECK(st2 > 0 && st2 < 1000);
  CHECK(w.slices == before + 2);

  /* Reset reaches the monitor last, after the CPU and the devices - and it is
   * the machine's reset, not the module's, that calls it. */
  m.reset();
  CHECK(w.resets == 2);
}

void test_devices() {
  std::printf("[device adapters]\n");
  Bus bus;
  /* 16 KiB of RAM at 0, so the device windows below are in the gaps. */
  CHECK(bus.map_ram(0, 0x4000u, "ram"));

  /* BufferDevice: the caller's buffer *is* the memory, writable or not. */
  static u8 buf[0x2000];
  std::memset(buf, 0xAB, sizeof buf);
  BufferDevice rom("rom", "rom", 0x10000, 0x1000, buf, /*writable=*/false);
  BufferDevice ram("scratch", "scratch", 0x11000, 0x1000, buf, /*writable=*/true);
  CHECK(rom.attach(bus));
  CHECK(ram.attach(bus));
  u8 b = 0;
  CHECK(bus.read_byte(0x10000, &b) && b == 0xAB);
  CHECK(!bus.write_byte(0x10000, 7));           /* ROM is read-only         */
  CHECK(bus.write_byte(0x11000, 7));
  CHECK(buf[0] == 7);                           /* caller's buffer, written */
  CHECK(buf[0x1000] == 0xAB);                   /* ...and the ROM half still */
  CHECK(rom.attach(bus));                       /* already attached: no-op  */
  /* A device whose window is not a whole page is refused, not rounded. */
  BufferDevice odd("odd", "odd", 0x20000, 0x800, buf, true);
  CHECK(!odd.attach(bus));

    /* A device that overlaps a RAM page is refused: the identity fast path and a
   * callback cannot both serve one address. */
  FuncDevice clash(
      "clash", "clash", 0x3000, 0x1000,
      [](void*, u32, u32) -> u32 { return 1; },
      [](void*, u32, u32, u32) {}, nullptr);
  CHECK(!clash.attach(bus));

  /* reset() releases the bus's own RAM but leaves a caller's buffer alone. */
  bus.reset();
  CHECK(bus.region_count() == 0);
  CHECK(buf[0] == 7);
  CHECK(!bus.read_byte(0x10000, &b));            /* the ROM is unmapped too  */
  CHECK(!bus.read_byte(0x0, &b));                /* ...and so is the RAM     */
}

void test_device_modules() {
  std::printf("[device modules]\n");
  const ModuleRegistry& reg = ModuleRegistry::instance();
  const DeviceModule* exit = reg.device_by_id("exit");
  const DeviceModule* console = reg.device_by_id("console");
  CHECK(exit != nullptr && console != nullptr);
  CHECK(exit && exit->info().kind == ModuleKind::Device);
  CHECK(exit && std::strcmp(module_kind_name(exit->info().kind), "device") == 0);
  CHECK(exit && exit->default_base() == 0x10000010u);
  CHECK(console && console->default_base() == 0x10000000u);
  CHECK(reg.device_by_id("no-such-device") == nullptr);
  CHECK(reg.device_by_id(nullptr) == nullptr);

  /* Every registered kind is listed, and the ids of a kind are joined for an
   * error message. */
  bool saw_exit = false, saw_console = false;
  for (u32 i = 0; i < reg.count(); ++i) {
    const ModuleInfo* mi = reg.info(i);
    if (!mi) continue;
    if (mi->kind == ModuleKind::Device && !std::strcmp(mi->id, "exit")) saw_exit = true;
    if (mi->kind == ModuleKind::Device && !std::strcmp(mi->id, "console")) saw_console = true;
  }
  CHECK(saw_exit && saw_console);
  CHECK(std::strstr(reg.ids(ModuleKind::Device), "exit") != nullptr);
  CHECK(std::strstr(reg.ids(ModuleKind::Device), "console") != nullptr);
  CHECK(std::strcmp(reg.ids(ModuleKind::Cpu), "") != 0);

  /* A machine built by id: both modules attach, in one page, and the exit port
   * stops the run when the guest writes to it. */
  MachineConfig c;
  c.ram_size = 1u << 16;
  c.cpu_id = "toy8";
  c.add_device(ModuleSpec("console"));
  c.add_device(ModuleSpec("exit"));
  Machine m;
  CHECK(m.init(c));
  CHECK(m.device_count() == 2);
  Device* con = m.device_by_id("console");
  Device* ex = m.device_by_id("exit");
  CHECK(con && std::strcmp(con->info().name, "test console") == 0);
  CHECK(ex && ex->base() == 0x10000010u);
  CHECK(con && con->base() == 0x10000000u);
  CHECK(std::strcmp(m.last_error(), "no error") == 0);

  /* toy8 reaches the bus with STA/OUT: `STA (X)` writes through it.  Whatever
   * lands on the console device is text; the exit port's fields say what the
   * guest stored and that the run stopped. */
  CHECK(m.bus().write_byte(0x10000000, 'h'));
  CHECK(m.bus().write_byte(0x10000000, 'i'));
  CHECK(std::strcmp(con->field_value(1), "hi") == 0);   /* the console log */
  CHECK(m.bus().write_byte(0x10000010, 3));
  CHECK(m.bus().write_byte(0x10000011, 0));
  /* The store reached the device, and the device asked the run to stop - which
   * ends the call it lands in.  This one lands outside a run (the test wrote to
   * the bus itself), so there is no run to end and the flag is simply set until
   * the next one starts; a guest that stores here mid-run is what the board
   * tests do. */
  CHECK(m.cpu().stop_requested());

  /* The fields are how a tool reads a device it knows nothing about. */
  CHECK(ex->field_count() == 3);
  CHECK(std::strcmp(ex->field_name(0), "code") == 0);
  CHECK(std::strcmp(ex->field_value(0), "0x00000003") == 0);
  CHECK(std::strcmp(ex->field_value(2), "1") == 0);          /* one write  */
  CHECK(con->field_count() == 2);
  CHECK(std::strcmp(con->field_name(0), "bytes") == 0);
  CHECK(std::strcmp(con->field_value(0), "2") == 0);

  /* An id nothing registers fails, and the message says what does. */
  Machine m2;
  MachineConfig c2;
  c2.ram_size = 1u << 16;
  c2.cpu_id = "toy8";
  CHECK(m2.init(c2));
  CHECK(m2.add_device_by_id("nope", 0, true) == nullptr);
  CHECK(std::strstr(m2.last_error(), "no device module named 'nope'") != nullptr);
  CHECK(std::strstr(m2.last_error(), "exit") != nullptr);
  CHECK(m2.add_monitor_by_id("nope") == nullptr);
  CHECK(std::strstr(m2.last_error(), "no monitor module named 'nope'") != nullptr);
}

void test_device_page_sharing() {
  std::printf("[device page sharing]\n");
  Bus bus;
  CHECK(bus.map_ram(0, 0x4000u, "ram"));
  u8 seen = 0;
  FuncDevice lo("lo", "lo", 0x10000000, 4,
                [](void* u, u32, u32) -> u32 { return *(u8*)u; },
                [](void* u, u32, u32 v, u32) { *(u8*)u = (u8)v; }, &seen);
  CHECK(lo.attach(bus));
  CHECK(bus.write_byte(0x10000000, 0x11) && seen == 0x11);

  /* A second window in the same page is fine as long as it does not overlap. */
  u8 other = 0;
  FuncDevice hi("hi", "hi", 0x10000010, 4,
                [](void* u, u32, u32) -> u32 { return *(u8*)u; },
                [](void* u, u32, u32 v, u32) { *(u8*)u = (u8)v; }, &other);
  CHECK(hi.attach(bus));
  CHECK(bus.write_byte(0x10000010, 0x22) && other == 0x22);
  CHECK(seen == 0x11);                       /* the first device was untouched */
  u8 b = 0;
  CHECK(bus.read_byte(0x10000000, &b) && b == 0x11);
  CHECK(bus.read_byte(0x10000010, &b) && b == 0x22);

  /* An address in the page that belongs to no window still reaches the device
   * that owns the page - which is what it sees when it is alone. */
  seen = 0;
  CHECK(bus.read_byte(0x10000004, &b));
  CHECK(seen == 0);

  /* Overlapping the first window is refused, and so is a window wide enough to
   * need a second page: sharing is one page, two or more windows. */
  FuncDevice clash("clash", "clash", 0x10000002, 4,
                   [](void*, u32, u32) -> u32 { return 0; },
                   [](void*, u32, u32, u32) {}, nullptr);
  CHECK(!clash.attach(bus));
  FuncDevice wide("wide", "wide", 0x10000FF0, 0x20,
                  [](void*, u32, u32) -> u32 { return 0; },
                  [](void*, u32, u32, u32) {}, nullptr);
  CHECK(!wide.attach(bus));

  /* A shared page is one region and goes as one when it is unmapped. */
  CHECK(bus.unmap(0x10000000, 4));
  CHECK(!bus.read_byte(0x10000010, &b));
  CHECK(!bus.read_byte(0x10000000, &b));
}

void test_monitor_modules() {
  std::printf("[monitor modules]\n");
  const ModuleRegistry& reg = ModuleRegistry::instance();
  CHECK(reg.monitor_by_id("no-such-monitor") == nullptr);
  Machine m;
  MachineConfig c;
  c.ram_size = 1u << 16;
  c.cpu_id = "toy8";
  CHECK(m.init(c));

  /* A monitor module registered inside a test - the point being that the core
   * needs no line of code to know about it, only a factory.  It counts slices
   * and describes itself through fields. */
  struct SliceCounter : Monitor {
    const ModuleInfo& info() const override {
      static const ModuleInfo mi = {"slices", "slice counter", ModuleKind::Monitor,
                                    "counts the slices the run loop reports"};
      return mi;
    }
    void on_slice(Machine&, u64 retired) override {
      ++slices;
      retired_total += retired;
    }
    u32 field_count() const override { return 2; }
    const char* field_name(u32 i) const override {
      return i == 0 ? "slices" : "retired";
    }
    const char* field_value(u32 i) const override {
      static char n[2][24];
      if (i == 0) std::snprintf(n[0], sizeof(n[0]), "%llu", (unsigned long long)slices);
      else std::snprintf(n[1], sizeof(n[1]), "%llu", (unsigned long long)retired_total);
      return n[i];
    }
    u64 slices = 0, retired_total = 0;
  };
  struct CounterModule : MonitorModule {
    const ModuleInfo& info() const override {
      static const ModuleInfo mi = {"slices", "slice counter", ModuleKind::Monitor,
                                    "counts the slices the run loop reports"};
      return mi;
    }
    std::unique_ptr<Monitor> create() const override {
      return std::unique_ptr<Monitor>(new SliceCounter());
    }
  };
  static const CounterModule mod;
  ModuleRegistry::instance().add_monitor(&mod);

  /* By id, through the machine: built, told about the run, readable through the
   * self-description the core only forwards. */
  const u8 prog[] = {0x01, 0x07, 0x07, 0x07, 0x0D, 0x00, 0x1F};
  CHECK(m.load_raw(prog, sizeof prog, 0x100) == 0);
  Monitor* mon = m.add_monitor_by_id("slices");
  CHECK(mon != nullptr);
  CHECK(m.monitor_count() == 1);
  CHECK(m.monitor_by_id("slices") == mon);
  CHECK(m.run_insns(3) == 3);
  CHECK(std::strcmp(mon->field_value(0), "1") == 0);      /* one slice   */
  CHECK(std::strcmp(mon->field_value(1), "3") == 0);      /* three insns */
  CHECK(std::strcmp(mon->info().id, "slices") == 0);
}

void test_loader_registry() {
  std::printf("[loader module]\n");
  Bus bus;
  CHECK(bus.map_ram(0x8000, 1u << 16, "ram"));

  /* The ELF loader claims an ELF by content, so a caller can hand it anything;
   * `raw` never claims. */
  const u8 magic[52] = {0x7f, 'E', 'L', 'F', 1, 1};
  CHECK(loader_elf32().probe(magic, sizeof magic));
  CHECK(!loader_raw().probe(magic, sizeof magic));
  const Loader* l = ModuleRegistry::instance().loader_by_probe(magic, sizeof magic);
  CHECK(l == &loader_elf32());
  CHECK(l && std::strcmp(l->info().id, "elf32") == 0);
  CHECK(std::strcmp(ModuleRegistry::instance().loader_by_id("raw")->info().id,
                    "raw") == 0);
}

/* A machine can be built twice, and its configuration belongs to it.
 *
 * Two real bugs live behind this test.  `init` used to *add* to whatever the
 * machine already had (so a second build left the first build's devices in the
 * list, and `reinit` leaked one deque per call), and the configuration the machine
 * kept held the caller's `const char*` - which the C API and the command line both
 * fill from temporaries, so `reinit` re-read names that no longer existed.  The
 * check below makes the second one fail loudly rather than subtly: the caller's
 * strings are overwritten before the machine is asked to rebuild.
 */
void test_rebuild_and_config_ownership() {
  std::printf("[rebuild, and whose strings they are]\n");
  MachineConfig c;
  c.ram_size = 1u << 20;
  c.cpu_id = "toy8";
  c.board = "timer";
  c.ram_name = "ram-for-the-first-build";
  /* Long enough to be heap-allocated on purpose: a short name would live inside
   * the std::string object and stay readable after it dies. */
  std::string instance =
      "console-instance-name-long-enough-to-be-heap-allocated";
  c.add_device(ModuleSpec("console", 0x10002000u, true, instance.c_str()));
  std::string out = "irq", in = "fiq";
  c.add_wire("timer", out.c_str(), in.c_str());

  Machine m;
  CHECK(m.init(c));
  CHECK(m.device_count() == 4);
  CHECK(m.wire_count() == 2);
  CHECK(m.device_by_id(instance.c_str()) != nullptr);
  /* The machine's copy is its own: nothing the caller does can reach it. */
  instance.assign(instance.size(), 'x');
  out.assign(8, 'y');
  in.assign(8, 'z');
  c.cpu_id = "no-such-cpu";
  c.board = "no-such-board";
  c.ram_name = nullptr;
  CHECK(std::strcmp(m.config().cpu_id, "toy8") == 0);
  CHECK(std::strcmp(m.config().board, "timer") == 0);
  CHECK(std::strcmp(m.config().ram_name, "ram-for-the-first-build") == 0);
  CHECK(m.device_by_id("console-instance-name-long-enough-to-be-heap-allocated") != nullptr);
  CHECK(std::strcmp(m.wire(0).output, "irq") == 0);
  CHECK(std::strcmp(m.wire(1).input, "fiq") == 0);

  /* Rebuilding from that configuration gives a machine with the same shape, not
   * twice as many devices. */
  CHECK(m.reinit());
  CHECK(m.device_count() == 4);
  CHECK(m.wire_count() == 2);
  CHECK(m.device_by_id("console-instance-name-long-enough-to-be-heap-allocated") != nullptr);
  CHECK(std::strcmp(m.wire(0).device, "timer") == 0);
  CHECK(std::strcmp(m.config().board, "timer") == 0);
  /* A third build, and a reset in between: the counters are the new machine's. */
  CHECK(m.reinit());
  CHECK(m.device_count() == 4);
  m.reset();
  CHECK(m.reinit());
  CHECK(m.device_count() == 4 && m.wire_count() == 2);

  /* A build that fails leaves a machine that can still be built. */
  MachineConfig bad;
  bad.ram_size = 1u << 20;
  bad.cpu_id = "toy8";
  bad.add_wire("timer", "irq", "irq");          /* no timer in this config */
  Machine m2;
  CHECK(!m2.init(bad));
  CHECK(std::strstr(m2.last_error(), "no device named 'timer'") != nullptr);
  CHECK(m2.init(MachineConfig()));              /* the default machine comes up */
  CHECK(m2.device_count() == 0 && m2.wire_count() == 0);
  CHECK(m2.has_cpu());
}

int main() {
  std::printf("brain-emu core tests: %s\n", brain_version_string());
  brainemutest::ensure_builtins();
  test_registry();
  test_unknown_cpu();
  test_fake_cpu_module();
  test_monitors();
  test_devices();
  test_device_modules();
  test_device_page_sharing();
  test_monitor_modules();
  test_loader_registry();
  test_rebuild_and_config_ownership();
  return brainemutest::summary("core");
}
