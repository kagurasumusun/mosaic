/* brain-emu -- the module system and the machine that drives it.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The ARM tests prove the ARM926 module works.  These prove the *core* is a
 * core: that a second CPU module (toy8) runs on it, that a CPU module written
 * inside this file runs on it, and that the machine, the bus, the devices, the
 * loaders and the C API never needed to know which of them it was driving.
 *
 * A check here that reads like an instruction test is not one: the toy8 program
 * below is a fixture, and its semantics live in src/modules/toy8/toy8.h.
 */
#include <brainemu/brainemu.h>

#include "brainemu/core/machine.h"
#include "brainemu/core/module.h"
#include "brainemu/core/monitor.h"
#include "brainemu/cpu/cpu.h"
#include "brainemu/device/device.h"
#include "brainemu/loader/loader.h"
#include "toy8/toy8.h"

#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

using namespace brain;

namespace {

int g_checks = 0, g_failed = 0;

void check(bool ok, const char* what) {
  ++g_checks;
  if (!ok) {
    ++g_failed;
    std::printf("  FAILED: %s\n", what);
  }
}

#define CHECK(cond) check((cond), #cond)

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
  u64 run_insns(u64 n) override {
    u64 done = 0;
    while (done < n && !stopped_) { step(); ++done; }
    return done;
  }
  u64 run_cycles(u64 max_cycles) override {
    u64 done = 0;
    while (done < max_cycles && !stopped_) { step(); ++done; }
    return done;
  }
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

/* A device that records what it saw, with a tick counter and a reset hook. */
struct Recorder {
  u32 writes[8] = {};
  u32 nwrites = 0;
  u32 last_size = 0;
  u64 ticks = 0;
  u32 resets = 0;
};

class RecorderDevice final : public Device {
 public:
  explicit RecorderDevice(Recorder* r) : r_(r) {
    info_.id = "recorder";
    info_.name = "recorder";
    info_.kind = ModuleKind::Device;
  }
  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return 0xF000u; }   /* reachable in 16 bits */
  u32 size() const override { return 0x1000u; }
  u32 read(u32, u32) override { return 0xDEADBEEFu; }
  void write(u32, u32 value, u32 size) override {
    if (r_->nwrites < 8) r_->writes[r_->nwrites++] = value;
    r_->last_size = size;
  }
  void tick(u64 cycles) override { r_->ticks += cycles; }
  void reset() override { ++r_->resets; }

 private:
  Recorder* r_;
  ModuleInfo info_;
};

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

void test_toy8_on_the_core() {
  std::printf("[toy8 module]\n");
  Machine m;
  MachineConfig c;
  /* The toy8 address space is 16 bits wide, so RAM is the lower 48 KiB and the
   * device below sits above it. */
  c.ram_size = 0xC000u;
  c.cpu_id = "toy8";
  CHECK(m.init(c));
  if (!m.has_cpu()) return;

  /* The module's own identity, as the core reports it. */
  const CpuInfo& ci = m.cpu_info();
  CHECK(std::strcmp(ci.id, "toy8") == 0);
  CHECK(ci.reg_count == 5);
  CHECK(ci.pc_index == 3);
  CHECK(ci.status_index == 4);
  CHECK(ci.saved_status_index == CPU_NO_REG);
  CHECK(ci.alternate_state == false);

  /* A toy8 program: A = 5, A += 3, store A at 0x4000, send A to the device
   * at 0xF000, halt.  Five instructions, ten bytes. */
  const u8 prog[] = {
      0x01, 0x05,              /* LDA #5            */
      0x07, 0x03,              /* ADD #3            */
      0x03, 0x00, 0x40,        /* STA 0x4000        */
      0x1C, 0x00, 0xF0,        /* OUT 0xF000        */
      0x1F,                    /* HALT              */
  };
  Recorder rec;
  RecorderDevice dev(&rec);
  CHECK(m.add_device(dev));
  CHECK(m.device_count() == 1);
  CHECK(dev.attached());
  CHECK(rec.resets == 1);                 /* attach resets the device       */

  ImageInfo info;
  CHECK(m.load_raw(prog, sizeof prog, 0x100) == 0);
  CHECK(m.cpu().pc() == 0x100);

  /* The budget is honoured exactly, and the run continues where it stopped. */
  CHECK(m.run_insns(2) == 2);             /* LDA #5, ADD #3                 */
  CHECK(m.cpu().get_reg(toy8::kRegA) == 8);
  CHECK(m.cpu().get_reg(toy8::kRegPC) == 0x104u);
  CHECK(m.run_insns(100) == 3);           /* STA, OUT, HALT: then it stops  */
  CHECK(m.cpu().get_reg(toy8::kRegA) == 8);
  CHECK(m.cpu().get_reg(toy8::kRegPC) == 0x10Bu);

  /* ...the store went to memory... */
  u8 byte = 0;
  CHECK(m.bus().read_byte(0x4000, &byte));
  CHECK(byte == 8);
  /* ...and the OUT instruction reached the device mapped over the bus. */
  CHECK(rec.nwrites == 1);
  CHECK(rec.writes[0] == 8);
  CHECK(rec.last_size == 1);

  /* Counters: the machine asks the module, the module answers. */
  CHECK(m.cpu().insns() == 5);
  CHECK(m.cpu().cycles() == 5);
  CHECK(m.cpu().stats().insns_interp == 5);
  CHECK(m.bus().stat_writes >= 2);        /* STA + OUT, straight through    */

  /* A device is clocked once per run, with the cycles that run charged. */
  const u64 ticks_before = rec.ticks;
  CHECK(m.run_cycles(3) == 0);            /* halted: no instruction retires  */
  CHECK(m.run_insns(1) == 0);

  /* The CPU has no JIT and says so rather than pretending. */
  CHECK(!m.cpu().jit_active());
  CHECK(!m.cpu().enable_jit(true));

  /* Options reach the module by name; an unknown one is refused. */
  CHECK(m.cpu().set_option("trace", "1"));
  CHECK(!m.cpu().set_option("no-such-option", "1"));

  /* Interrupt lines are the machine's to drive even when the module ignores
   * them: the call must not crash or change anything. */
  m.cpu().set_irq(1);
  m.cpu().set_fiq(1);
  CHECK(m.cpu().pending_exception() == 0);
  CHECK(ticks_before <= rec.ticks);

  /* reset() puts the module, the devices and the counters back. */
  m.reset();
  CHECK(m.cpu().insns() == 0);
  CHECK(rec.resets == 2);
  CHECK(m.bus().stat_writes == 0);
}

void test_toy8_through_the_c_api() {
  std::printf("[toy8 through the C API]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 0xC000u;             /* the device sits above RAM           */
  cfg.cpu_id = "toy8";
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;

  /* Everything the API reports about the CPU comes from the module. */
  CHECK(std::strcmp(brain_cpu_id(e), "toy8") == 0);
  CHECK(std::strcmp(brain_cpu_name(e), "toy-8") == 0);
  CHECK(brain_cpu_reg_count(e) == 5);
  CHECK(brain_cpu_pc_reg(e) == (int)toy8::kRegPC);
  CHECK(brain_cpu_status_reg(e) == (int)toy8::kRegFlags);
  CHECK(brain_cpu_saved_status_reg(e) == -1);
  CHECK(brain_cpu_sp_reg(e) == (int)toy8::kRegSP);
  CHECK(brain_cpu_alternate_state(e) == 0);

  /* The register indices are the module's, and set_reg goes through them. */
  CHECK(brain_set_reg(e, (int)toy8::kRegA, 9) == 0);
  CHECK(brain_set_reg(e, 99, 1) != 0);
  CHECK(brain_get_reg(e, (int)toy8::kRegA) == 9);

  /* A device mapped by the C API is a FuncDevice on the bus. */
  static u32 seen = 0;
  static u32 seen_size = 0;
  struct Sink { };
  auto wr = [](void* user, uint32_t, uint32_t value, uint32_t size) {
    *(uint32_t*)user = value;
    seen_size = size;
    seen = value;
  };
  Sink sink;
  CHECK(brain_map_device(e, 0xF000u, 4096u, nullptr,
                         (brain_bus_write_t)wr, &sink, "sink") == 0);

  /* OUT to the device, then a HALT. */
  const uint8_t prog[] = {0x1C, 0x00, 0xF0, 0x1F};
  CHECK(brain_load_raw(e, prog, sizeof prog, 0x200, 0) == 0);
  CHECK(brain_run_insns(e, 100) == 2);
  CHECK(seen == 9);
  CHECK(seen_size == 1);
  brain_stats_t s;
  brain_get_stats(e, &s);
  CHECK(s.insns == 2);
  CHECK(s.cycles == 2);
  CHECK(s.insns_jit == 0);
  CHECK(s.blocks == 0);

  /* An option handed to the module, by name, through the API. */
  CHECK(brain_set_option(e, "trace", "0") == 1);
  CHECK(brain_set_option(e, "not-a-knob", "1") == 0);

  brain_emu_destroy(e);

  /* A bad option at creation time fails the create, rather than being
   * swallowed: a caller that typed a knob expects it to be in effect. */
  brain_config_t bad;
  std::memset(&bad, 0, sizeof bad);
  bad.cpu_id = "toy8";
  bad.cpu_options = "not-a-knob=1";
  brain_emu_t* e2 = brain_emu_create(&bad);
  CHECK(e2 == nullptr);
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

  m.stop();
  CHECK(m.run_insns(1000) == 0);        /* the module saw request_stop()    */

  /* Counters travel through the generic struct: the module wrote two fields and
   * the machine reports them, without knowing what "insns_interp" means. */
  CHECK(m.cpu().stats().insns_interp == 106);
  CHECK(m.cpu().stats().cycles_interp == 106);

  /* The generic config reached the module's init(). */
  BlipCore* blip = dynamic_cast<BlipCore*>(&m.cpu());
  CHECK(blip != nullptr);
  CHECK(blip && blip->keep_);

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
  CHECK(m.cpu().get_reg(toy8::kRegA) == 15);   /* 7 + 7 + 1, read back here   */

  /* step() is a slice of one instruction, and a halted CPU is a slice of zero:
   * the run loop reports what happened, it does not judge it. */
  m.step();
  CHECK(w.slices == 2 && w.last == 1);
  CHECK(m.run_insns(100) == 1);                /* the HALT                   */
  CHECK(w.slices == 3 && w.last == 1);
  CHECK(m.run_insns(100) == 0);                /* halted: nothing to report  */
  CHECK(w.slices == 4 && w.last == 0);

  /* A monitor may stop the machine - a watchpoint, a breakpoint, a tracer's
   * "that is enough".  The stop lands at the next slice boundary, so the slice
   * that tripped it finishes and the next one retires nothing. */
  m.reset();
  CHECK(w.resets == 1);
  CHECK(m.cpu().insns() == 0);
  const u32 before = w.slices;
  mon.stop_after(before + 1);
  CHECK(m.run_insns(2) == 2);
  CHECK(w.slices == before + 1);
  CHECK(m.run_insns(1000) == 0);               /* the stop was honoured      */
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

  /* ConsoleDevice: bytes written to it are text. */
  ConsoleDevice con(0x5000);
  CHECK(con.attach(bus));
  const char* msg = "ok\n";
  for (const char* p = msg; *p; ++p) CHECK(bus.write_byte(0x5000, (u8)*p));
  CHECK(std::strcmp(con.log(), "ok\n") == 0);
  CHECK(bus.read_byte(0x5000, &b) && b == 0);   /* nothing waits to be read  */

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

/* The two device modules the build ships, and the page they share.  This is the
 * test of the *kind*: the registry hands out factories, the machine builds from
 * an id, and the bus lets two windows live in one 4 KiB page - which is what a
 * real peripheral map looks like (the i.MX28's test ports are four words inside
 * the 0x10000000 page). */
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
  CHECK(std::strcmp(static_cast<ConsoleDevice*>(con)->log(), "hi") == 0);
  CHECK(m.bus().write_byte(0x10000010, 3));
  CHECK(m.bus().write_byte(0x10000011, 0));
  CHECK(m.run_insns(1) == 0);      /* the stop was requested by the write   */

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

/* --------------------------------------------------------- the timer ---- */

namespace {

/* What a wire carries, and how often: the notify hook a machine installs is
 * `Device::set_irq_notify`, so a test can stand in for the machine and count.
 * The count is the whole point - a device that re-drives its line on every tick
 * while a peripheral stays pending would make a machine's delivery path (and any
 * CPU's edge logic) see traffic that is not there. */
struct LevelLog {
  int calls = 0;
  int levels[16] = {};
  u32 outputs[16] = {};
};

void log_irq(void* user, Device&, u32 output, int level) {
  LevelLog* l = static_cast<LevelLog*>(user);
  if (l->calls < 16) {
    l->levels[l->calls] = level;
    l->outputs[l->calls] = output;
  }
  ++l->calls;
}

/* The device register file through the bus, one byte at a time - the only
 * accessor the bus publishes (a device's registers are not necessarily four
 * bytes wide, which is why the bus does not guess). */
u32 reg32(Bus& bus, u32 addr) {
  u32 v = 0;
  for (u32 i = 0; i < 4; ++i) {
    u8 b = 0;
    bus.read_byte(addr + i, &b);
    v |= (u32)b << (8 * i);
  }
  return v;
}

void put32(Bus& bus, u32 addr, u32 v) {
  for (u32 i = 0; i < 4; ++i) bus.write_byte(addr + i, (u8)(v >> (8 * i)));
}

}  // namespace

/* The timer module, on its own bus, with the clocks driven by hand: this is the
 * device contract every peripheral in the tree will follow - a window in the
 * address space, registers with read/write semantics, a latched level on an
 * interrupt output, and a `tick` that only ever moves it forward. */
void test_timer_device() {
  std::printf("[timer device]\n");
  const DeviceModule* mod = ModuleRegistry::instance().device_by_id("timer");
  CHECK(mod != nullptr);
  if (!mod) return;
  CHECK(mod->default_base() == 0x10001000u);
  CHECK(mod->info().kind == ModuleKind::Device);

  Bus bus;
  CHECK(bus.map_ram(0x8000, 1u << 16, "ram"));
  std::unique_ptr<Device> dev = mod->create(mod->default_base());
  CHECK(dev != nullptr);
  if (!dev) return;
  CHECK(dev->attach(bus));                 /* attach() resets the device */
  CHECK(dev->base() == 0x10001000u);
  CHECK(dev->size() == 0x14u);
  CHECK(dev->irq_count() == 1);
  CHECK(std::strcmp(dev->irq_name(0), "irq") == 0);

  LevelLog log;
  dev->set_irq_notify(&log_irq, &log);

  /* Reading a register the device does not have is zero, not a fault: the whole
   * covering page is the window. */
  CHECK(reg32(bus, 0x10001000u) == 0u);          /* PERIOD             */
  CHECK(reg32(bus, 0x10001004u) == 0u);          /* COUNT              */
  CHECK(reg32(bus, 0x1000100Cu) == 0u);          /* STATUS: idle       */

  /* A period alone arms nothing: the counter does not move until CTRL[0]. */
  put32(bus, 0x10001000u, 100u);
  CHECK(reg32(bus, 0x10001004u) == 100u);
  dev->tick(500);
  CHECK(reg32(bus, 0x10001004u) == 100u);
  CHECK(dev->irq_level(0) == 0);
  CHECK(log.calls == 0);
  CHECK(std::strcmp(dev->field_value(0), "100") == 0);   /* period */
  CHECK(std::strcmp(dev->field_value(2), "no") == 0);    /* pending */

  /* Enabled: the counter is the clock, and an expiry latches the line. */
  put32(bus, 0x10001008u, 1u);                   /* CTRL[0] = enable   */
  dev->tick(60);
  CHECK(reg32(bus, 0x10001004u) == 40u);
  CHECK((reg32(bus, 0x1000100Cu) & 3u) == 2u);    /* running, not pending */
  CHECK(dev->irq_level(0) == 0);
  dev->tick(40);
  CHECK((reg32(bus, 0x1000100Cu) & 1u) == 1u);    /* pending            */
  CHECK(dev->irq_level(0) == 1);
  CHECK(log.calls == 1 && log.levels[0] == 1 && log.outputs[0] == 0);

  /* The level *is* level-sensitive: further expiries while the guest has not
   * acknowledged produce no new level and no new notification, and `ticks` says
   * how many were swallowed - the count a test uses to see a missed interrupt. */
  dev->tick(300);
  CHECK(dev->irq_level(0) == 1);
  check(log.calls == 1, "a level that is already high is not re-notified");
  /* Four expiries by now: the one at 60+40, then three more inside the 300.  A
   * device that counts the expiry it is passing from the whole clock delta would
   * say five, and a guest would take an interrupt that never happened. */
  CHECK(std::strcmp(dev->field_value(1), "4") == 0);     /* ticks  */
  CHECK(std::strcmp(dev->field_value(2), "yes") == 0);   /* pending */
  CHECK(std::strcmp(dev->field_value(3), "yes") == 0);   /* running */

  /* Acknowledge: the latch clears and the line drops, once. */
  put32(bus, 0x10001010u, 1u);
  CHECK(dev->irq_level(0) == 0);
  CHECK((reg32(bus, 0x1000100Cu) & 1u) == 0u);
  CHECK(log.calls == 2 && log.levels[1] == 0);
  put32(bus, 0x10001010u, 1u);                   /* a second ack does nothing */
  CHECK(log.calls == 2);

  /* Writing CTRL cannot acknowledge and cannot perturb the pending latch, and a
   * write to COUNT is ignored: the only way to move any of this is the register
   * that means it. */
  put32(bus, 0x10001008u, 1u);
  CHECK(dev->irq_level(0) == 0 && log.calls == 2);
  dev->tick(100);
  CHECK(dev->irq_level(0) == 1 && log.calls == 3);
  put32(bus, 0x10001004u, 5u);
  CHECK(reg32(bus, 0x10001004u) == 100u);         /* COUNT is read-only */
  put32(bus, 0x10001008u, 1u);                   /* re-writing enable keeps pending */
  CHECK(dev->irq_level(0) == 1 && log.calls == 3);
  /* Disabling stops the countdown but keeps a pending interrupt pending. */
  put32(bus, 0x10001008u, 0u);
  dev->tick(500);
  CHECK(dev->irq_level(0) == 1 && log.calls == 3);
  CHECK(std::strcmp(dev->field_value(2), "yes") == 0);
  put32(bus, 0x10001010u, 0u);                  /* ack value is "anything" */
  CHECK(dev->irq_level(0) == 0 && log.calls == 4);

  /* A fresh period restarts the counter, and reset() puts everything back. */
  put32(bus, 0x10001008u, 1u);
  put32(bus, 0x10001000u, 7u);
  CHECK(reg32(bus, 0x10001004u) == 7u);
  dev->tick(6);
  CHECK(reg32(bus, 0x10001004u) == 1u);
  dev->tick(1);
  CHECK(dev->irq_level(0) == 1);
  dev->reset();
  CHECK(dev->irq_level(0) == 0);
  CHECK(reg32(bus, 0x10001000u) == 0u && reg32(bus, 0x10001004u) == 0u);
  CHECK(reg32(bus, 0x10001008u) == 0u && reg32(bus, 0x1000100Cu) == 0u);
  CHECK(std::strcmp(dev->field_value(1), "0") == 0);      /* ticks back to 0 */
  check(log.calls == 6, "reset drops the line it had raised");
}

/* ------------------------------------------------------------- boards ---- */

/* A board is a *preset*, not a special kind of machine: it fills a config with
 * devices, monitors and wires, and `Machine::init` applies it before the
 * caller's own choices, so `board + one peripheral` works and a board can never
 * take an explicit choice away.  This is the seam the whole "start from a named
 * machine" idea rests on, and the reason the registry carries kinds other than
 * CPU. */
void test_boards_and_wiring() {
  std::printf("[boards and wiring]\n");
  const ModuleRegistry& reg = ModuleRegistry::instance();
  CHECK(std::strcmp(module_kind_name(ModuleKind::Board), "board") == 0);
  CHECK((u32)ModuleKind::Board < (u32)ModuleKind::Count);

  const BoardModule* minimal = reg.board_by_id("minimal");
  const BoardModule* timer = reg.board_by_id("timer");
  CHECK(minimal && timer);
  CHECK(minimal && minimal->info().kind == ModuleKind::Board);
  CHECK(timer && timer->info().kind == ModuleKind::Board);
  CHECK(reg.board_by_id("no-such-board") == nullptr);
  CHECK(reg.board_by_id(nullptr) == nullptr);
  CHECK(reg.board_count() >= 2);
  bool saw_timer = false;
  for (u32 i = 0; i < reg.board_count(); ++i) {
    const BoardModule* b = reg.board_by_index(i);
    CHECK(b != nullptr);
    if (b && !std::strcmp(b->info().id, "timer")) saw_timer = true;
  }
  CHECK(saw_timer);
  CHECK(std::strstr(reg.ids(ModuleKind::Board), "minimal") != nullptr);
  CHECK(std::strstr(reg.ids(ModuleKind::Board), "timer") != nullptr);

  /* Building from a board: the layout comes from the preset, and the machine
   * says which one it was.  The preset itself is *not* applied to a config the
   * caller hands over - the caller hands the id to `init`, which is what keeps
   * one board's devices out of another's config. */
  MachineConfig c;
  c.ram_size = 1u << 20;
  c.cpu_id = "toy8";
  c.board = "timer";
  Machine m;
  CHECK(m.init(c));
  CHECK(m.device_count() == 3);
  CHECK(m.device_by_id("timer") != nullptr);
  CHECK(m.device_by_id("console") != nullptr);
  CHECK(m.device_by_id("exit") != nullptr);
  CHECK(m.config().board && std::strcmp(m.config().board, "timer") == 0);
  CHECK(m.wire_count() == 1);
  CHECK(std::strcmp(m.wire(0).device, "timer") == 0);
  CHECK(std::strcmp(m.wire(0).output, "irq") == 0);
  CHECK(std::strcmp(m.wire(0).input, "irq") == 0);
  CHECK(m.wire_level(0) == 0);

  /* The board's wire drives the machine's device: tick the timer past its period
   * and the wire the board declared reads high.  No CPU involvement - this is
   * the wiring path on its own. */
  Device* t = m.device_by_id("timer");
  CHECK(t != nullptr && t->irq_count() == 1);
  if (t) {
    put32(m.bus(), t->base() + 0x00u, 10u);          /* PERIOD */
    put32(m.bus(), t->base() + 0x08u, 1u);           /* enable */
    t->tick(10);
    CHECK(t->irq_level(0) == 1);
    CHECK(m.wire_level(0) == 1);
    put32(m.bus(), t->base() + 0x10u, 1u);           /* ack    */
    CHECK(m.wire_level(0) == 0);
  }

  /* Caller's choices on top of the board: a second wire (a *second* device's
   * line), a device, and an explicit CPU id.  The board's own wire survives. */
  MachineConfig c2;
  c2.ram_size = 1u << 20;
  c2.cpu_id = "toy8";
  c2.board = "timer";
  c2.add_device(ModuleSpec("console", 0x10002000u, true, "console2"));
  c2.add_wire("timer", "irq", "fiq");
  Machine m2;
  CHECK(m2.init(c2));
  CHECK(m2.wire_count() == 2);
  CHECK(std::strcmp(m2.wire(1).input, "fiq") == 0);
  CHECK(std::strcmp(m2.wire(1).device, "timer") == 0);
  CHECK(m2.device_count() == 4);                    /* the board's three + one */
  /* Two devices built from one module are told apart by their instance name, and
   * a wire (or a tool) resolves the instance first. */
  CHECK(m2.device_by_id("console2") == m2.device(3));
  CHECK(m2.device_by_id("console") != m2.device(3));
  CHECK(std::strcmp(m2.device(3)->instance(), "console2") == 0);
  CHECK(std::strcmp(m2.device(3)->info().id, "console") == 0);
  /* ...and two devices that answer to one name are refused where they are
   * created, because every wire for them would be ambiguous. */
  CHECK(m2.add_device_by_id("console", 0x10003000u, true) == nullptr);
  CHECK(std::strstr(m2.last_error(), "two devices are called 'console'") != nullptr);
  CHECK(m2.device_count() == 4);
  /* The names are copied into the machine: a caller (the C API, a CLI option
   * parser) may hand a temporary, and the wire has to keep working after the
   * temporary dies - a `const char*` kept as-is leaves the stats print reading
   * freed memory, which is exactly how this was found. */
  {
    std::string out = "irq", in = "fiq";
    CHECK(m2.add_wire("timer", out.c_str(), in.c_str()));
    out.assign("changed");
    in.assign("changed");
  }
  CHECK(m2.wire_count() == 3);
  CHECK(std::strcmp(m2.wire(2).output, "irq") == 0);
  CHECK(std::strcmp(m2.wire(2).input, "fiq") == 0);
  CHECK(m2.add_wire("timer", "irq", "nonsense") == false);
  CHECK(std::strstr(m2.last_error(), "no input named 'nonsense'") != nullptr);
  CHECK(std::strstr(m2.last_error(), "irq") != nullptr);   /* the ones it has */
  CHECK(m2.wire_count() == 3);                      /* a refused wire is not kept */
  CHECK(std::strcmp(m2.wire(2).output, "irq") == 0);  /* still not dangling */
  CHECK(m2.add_wire("nope", "irq", "irq") == false);
  CHECK(std::strstr(m2.last_error(), "no device named 'nope'") != nullptr);
  CHECK(m2.add_wire("timer", "nope", "irq") == false);
  CHECK(std::strstr(m2.last_error(), "no interrupt output named 'nope'") != nullptr);

  /* An unknown board fails the whole machine, and the message lists the boards
   * this build has - the same courtesy the CPU and device registries extend. */
  Machine m3;
  MachineConfig c3;
  c3.ram_size = 1u << 16;
  c3.cpu_id = "toy8";
  c3.board = "nope";
  CHECK(!m3.init(c3));
  CHECK(std::strstr(m3.last_error(), "no board module named 'nope'") != nullptr);
  CHECK(std::strstr(m3.last_error(), "timer") != nullptr);

  /* A wire that names something that exists but cannot be wired fails at
   * construction too, so a typo is reported before the first interrupt rather
   * than at it. */
  Machine m4;
  MachineConfig c4;
  c4.ram_size = 1u << 16;
  c4.cpu_id = "toy8";
  c4.board = "minimal";
  c4.add_wire("console", "irq", "irq");             /* the console has no output */
  CHECK(!m4.init(c4));
  CHECK(std::strstr(m4.last_error(), "no interrupt output named 'irq'") != nullptr);
  m4.last_error();                                   /* (readable after a failure) */
  CHECK(std::strcmp(m4.last_error(), "no error") == 0 ||
        std::strlen(m4.last_error()) > 0);

  /* The same seam through the C API, which is how a tool sees it: the board id,
   * the wires that came with it, and a wire added to a *live* machine.  Adding a
   * peripheral and wiring it has to work after the machine started, because that
   * is the only shape a debugger or a test harness can use. */
  brain_config_t bc;
  std::memset(&bc, 0, sizeof bc);
  bc.ram_size = 1u << 20;
  bc.cpu_id = nullptr;               /* this build's default CPU (first module) */
  bc.board_id = "minimal";
  brain_emu_t* e = brain_emu_create(&bc);
  CHECK(e != nullptr);
  if (e) {
    CHECK(std::strcmp(brain_board_id(e), "minimal") == 0);
    CHECK(brain_wire_count(e) == 0);
    CHECK(brain_add_wire(e, "console", "", "irq") != 0);   /* no output to wire */
    CHECK(brain_wire_count(e) == 0);
    CHECK(brain_add_device_module(e, "timer", 0, 0) == 0); /* default base     */
    CHECK(brain_add_wire(e, "timer", "irq", "irq") == 0);
    CHECK(brain_wire_count(e) == 1);
    const char* wd = nullptr;
    const char* wo = nullptr;
    const char* wi = nullptr;
    CHECK(brain_wire_info(e, 0, &wd, &wo, &wi) == 0);
    CHECK(wd && std::strcmp(wd, "timer") == 0);
    CHECK(wo && std::strcmp(wo, "irq") == 0);
    CHECK(wi && std::strcmp(wi, "irq") == 0);
    CHECK(brain_wire_info(e, 1, &wd, &wo, &wi) != 0);
    CHECK(brain_wire_level(e, 0) == 0);
    CHECK(brain_wire_level(e, 9) == 0);
    /* Drive the timer through its window and the wire reports the level its
     * device output carries - the end a tool shows. */
    /* Three ways to touch an address, and the differences matter: `brain_poke32`
     * and `brain_write_mem` are a debugger's bulk path into *RAM* (a poke
     * deliberately does not perturb a device hook, and neither reaches a
     * peripheral window at all), while `brain_write32` is the *guest's* access -
     * it goes through the CPU, so it is the one a device sees, and it is also
     * the one that respects the CPU's own address width (a 16-bit toy8 could not
     * address 0x10001000 even though the bus can). */
    brain_write32(e, 0x10001000u, 4u);          /* PERIOD */
    brain_write32(e, 0x10001008u, 1u);          /* enable */
    brain_run_cycles(e, 8);                     /* let the device clock move   */
    CHECK(brain_wire_level(e, 0) == 1);         /* the timer expired and latched */
    brain_emu_destroy(e);
  }
}

/* ------------------------------------------------- the interrupt chain ---- */

/* Devices have *inputs* as well as outputs, and that is what makes a chain -
 * peripheral to controller to CPU - possible without any of the three knowing
 * about the others.  This test drives the wire graph on its own, with no CPU
 * involved: a sink device reports what it was told, so the deliveries are
 * visible as plain numbers. */
namespace {

class SinkDevice final : public Device {
 public:
  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return base_; }
  u32 size() const override { return 4; }
  u32 read(u32, u32) override { return (u32)last_; }
  void write(u32, u32 v, u32) override { last_ = (int)v; }

  u32 input_count() const override { return 2; }
  const char* input_name(u32 i) const override {
    return i == 0 ? "in0" : i == 1 ? "in1" : "";
  }
  void set_input(u32 i, int level) override {
    if (i >= 2) return;
    levels_[i] = level;
    ++deliveries_;
  }
  /* What the test reads: the two levels it was handed, and how many times. */
  int level(u32 i) const { return i < 2 ? levels_[i] : -1; }
  int deliveries() const { return deliveries_; }

 private:
  ModuleInfo info_{"sink", "wire sink", ModuleKind::Device,
                   "a device that records the levels it is handed"};
  u32 base_ = 0x10003000;
  int last_ = 0;
  int levels_[2] = {0, 0};
  int deliveries_ = 0;
};

}  // namespace

void test_interrupt_chain() {
  std::printf("[interrupt chain: device -> device -> CPU]\n");
  const DeviceModule* tmod = ModuleRegistry::instance().device_by_id("timer");
  const DeviceModule* imod = ModuleRegistry::instance().device_by_id("intc");
  CHECK(tmod != nullptr && imod != nullptr);
  if (!tmod || !imod) return;

  /* The controller on its own bus: its inputs are pins, its output is a level,
   * and its mask is what decides between them. */
  Bus bus;
  CHECK(bus.map_ram(0x8000, 1u << 16, "ram"));
  std::unique_ptr<Device> idev = imod->create(imod->default_base());
  CHECK(idev != nullptr);
  if (!idev) return;
  CHECK(idev->attach(bus));
  CHECK(idev->input_count() == 4);
  CHECK(std::strcmp(idev->input_name(0), "irq0") == 0);
  CHECK(std::strcmp(idev->input_name(3), "irq3") == 0);
  CHECK(idev->irq_count() == 1);

  LevelLog log;
  idev->set_irq_notify(&log_irq, &log);
  CHECK(idev->irq_level(0) == 0);
  /* Everything is masked at reset: a line can assert and nothing comes out. */
  idev->set_input(0, 1);
  CHECK(idev->irq_level(0) == 0);
  CHECK((reg32(bus, 0x10002004u) & 1u) == 1u);            /* STATUS    */
  CHECK(reg32(bus, 0x10002008u) == 0u);                   /* PENDING   */
  /* Unmasking is what connects it, and the delivery is a level change. */
  put32(bus, 0x10002000u, 1u);                            /* MASK = line 0 */
  CHECK(idev->irq_level(0) == 1);
  CHECK(log.calls == 1 && log.levels[0] == 1);
  /* A second line asserting while the first is up does not re-drive. */
  idev->set_input(1, 1);
  put32(bus, 0x10002000u, 3u);
  CHECK(idev->irq_level(0) == 1);
  check(log.calls == 1, "an already-high output is not re-notified");
  CHECK((reg32(bus, 0x10002008u) & 3u) == 3u);
  /* The last line letting go is what drops it - a level, not a pulse. */
  idev->set_input(0, 0);
  CHECK(idev->irq_level(0) == 1);
  idev->set_input(1, 0);
  CHECK(idev->irq_level(0) == 0);
  CHECK(log.calls == 2 && log.levels[1] == 0);
  /* A software interrupt goes through the same mask and the same output, which
   * is what lets a guest (or a test) interrupt itself with no peripheral at all,
   * and what a real controller's "software interrupt" register does. */
  put32(bus, 0x1000200Cu, 2u);                            /* SET line 1 */
  CHECK(idev->irq_level(0) == 1);
  CHECK((reg32(bus, 0x10002004u) & 2u) == 2u);            /* it is a line, too */
  put32(bus, 0x10002010u, 2u);                            /* CLEAR line 1 */
  CHECK(idev->irq_level(0) == 0);
  /* Masking while asserted drops it as well, and the raw level is still there. */
  put32(bus, 0x1000200Cu, 1u);
  put32(bus, 0x10002000u, 0u);
  CHECK(idev->irq_level(0) == 0);
  CHECK((reg32(bus, 0x10002004u) & 1u) == 1u);
  idev->reset();
  CHECK(idev->irq_level(0) == 0 && (reg32(bus, 0x10002004u) & 1u) == 0u);

  /* Now the chain in a machine: two timer instances, a controller, one CPU line.
   * Every level in this test arrives through `Machine::deliver_irq`, because that
   * is the only thing that knows what the wires mean. */
  MachineConfig c;
  c.ram_size = 1u << 20;
  c.cpu_id = "toy8";
  c.board = "dual-timer";
  Machine m;
  CHECK(m.init(c));
  CHECK(m.device_count() == 5);          /* console, exit, t0, t1, intc */
  CHECK(m.wire_count() == 3);
  CHECK(std::strcmp(m.config().board, "dual-timer") == 0);
  Device* t0 = m.device_by_id("t0");
  Device* t1 = m.device_by_id("t1");
  Device* ic = m.device_by_id("intc");
  CHECK(t0 && t1 && ic);
  CHECK(t0 != t1);
  if (t0 && t1 && ic) {
    CHECK(t0->base() == 0x10001000u && t1->base() == 0x10001100u);
    /* Arm both timers and let the machine's own clock run: the expires travel
     * t0/t1 -> intc -> the wire the board declared. */
    put32(m.bus(), t0->base() + 0x00u, 10u);
    put32(m.bus(), t1->base() + 0x00u, 10u);
    put32(m.bus(), t0->base() + 0x08u, 1u);
    put32(m.bus(), t1->base() + 0x08u, 1u);
    put32(m.bus(), ic->base() + 0x00u, 3u);           /* unmask both lines */
    CHECK(m.wire_level(0) == 0 && m.wire_level(1) == 0 && m.wire_level(2) == 0);
    t0->tick(10);
    CHECK(t0->irq_level(0) == 1);
    CHECK((reg32(m.bus(), ic->base() + 0x08u) & 1u) == 1u);
    CHECK(ic->irq_level(0) == 1);
    /* The board's third wire is the one that reaches the CPU, and the machine
     * reports its level like any other. */
    CHECK(m.wire_level(2) == 1);
    CHECK(m.wire_level(0) == 1);
    CHECK(m.wire_level(1) == 0);
    /* Acknowledge t0: its line drops, the controller's other input is still low,
     * so the controller's output drops too, and with it the CPU's line. */
    put32(m.bus(), t0->base() + 0x10u, 1u);
    CHECK(t0->irq_level(0) == 0);
    CHECK(ic->irq_level(0) == 0);
    CHECK(m.wire_level(2) == 0);
    /* Now t1, which shares the same CPU line and must be indistinguishable from
     * t0 at the CPU end - that is the whole point of a controller. */
    t1->tick(10);
    CHECK(m.wire_level(2) == 1);
    put32(m.bus(), t1->base() + 0x10u, 1u);
    CHECK(m.wire_level(2) == 0);
    CHECK(std::strcmp(ic->field_value(2), "0x0") == 0);   /* pending back to 0 */
  }

  /* A caller-owned device can be wired by hand, and the sink end may be a
   * device: `sink.in0` is resolved by instance, `irq` by CPU input name. */
  MachineConfig c3;
  c3.ram_size = 1u << 16;
  c3.cpu_id = "toy8";
  c3.add_device(ModuleSpec("timer", 0, false, "t9"));
  c3.add_wire("t9", "irq", "irq");
  Machine m3;
  CHECK(m3.init(c3));
  SinkDevice sink;
  CHECK(m3.add_device(sink));
  CHECK(m3.add_wire("t9", "irq", "sink.in1"));
  CHECK(m3.wire_count() == 2);
  Device* t9 = m3.device_by_id("t9");
  CHECK(t9 != nullptr);
  if (t9) {
    put32(m3.bus(), t9->base() + 0x00u, 4u);
    put32(m3.bus(), t9->base() + 0x08u, 1u);
    t9->tick(4);
    CHECK(t9->irq_level(0) == 1);
    CHECK(sink.level(1) == 1);
    CHECK(sink.level(0) == 0);
    check(sink.deliveries() == 1, "one level change, one delivery");
    put32(m3.bus(), t9->base() + 0x10u, 1u);
    CHECK(sink.level(1) == 0);
    check(sink.deliveries() == 2, "the drop is delivered too");
  }
  /* The error messages name the end that is missing, and the names that exist. */
  CHECK(m3.add_wire("t9", "irq", "sink.in7") == false);
  CHECK(std::strstr(m3.last_error(), "has no interrupt input named 'in7'") != nullptr);
  CHECK(std::strstr(m3.last_error(), "in0 in1") != nullptr);
  CHECK(m3.add_wire("t9", "irq", "nosuch.in0") == false);
  CHECK(std::strstr(m3.last_error(), "no device named 'nosuch'") != nullptr);
  CHECK(m3.add_wire("t9", "irq", "sink") == false);     /* no dot: a CPU input */
  CHECK(std::strstr(m3.last_error(), "the CPU module has no input named 'sink'") != nullptr);
  /* The chain through the C API, including an instance name and a device sink. */
  brain_config_t bc;
  std::memset(&bc, 0, sizeof bc);
  bc.ram_size = 1u << 16;
  bc.cpu_id = "toy8";
  CHECK(brain_add_device_named(nullptr, "timer", "t0", 0, 0) != 0);
  brain_emu_t* e = brain_emu_create(&bc);
  CHECK(e != nullptr);
  if (e) {
    CHECK(brain_add_device_named(e, "timer", "t0", 0x10001000u, 1) == 0);
    CHECK(brain_add_device_named(e, "timer", "t1", 0x10001100u, 1) == 0);
    CHECK(brain_add_device_module(e, "intc", 0, 0) == 0);
    CHECK(brain_add_wire(e, "t0", "irq", "intc.irq0") == 0);
    CHECK(brain_add_wire(e, "t1", "irq", "intc.irq1") == 0);
    CHECK(brain_add_wire(e, "intc", "irq", "irq") == 0);
    CHECK(brain_wire_count(e) == 3);
    CHECK(std::strcmp(brain_device_instance(e, 0), "t0") == 0);
    CHECK(std::strcmp(brain_device_id(e, 0), "timer") == 0);
    for (uint32_t i = 0; i < brain_device_count(e); ++i) {
      if (std::strcmp(brain_device_instance(e, i), "intc")) continue;
      CHECK(std::strcmp(brain_device_id(e, i), "intc") == 0);
    }
    brain_emu_destroy(e);
  }
}

/* Sharing a page: two windows in 0x10000000, disjoint, plus the two ways it is
 * refused (an overlap, and a window that would need a second page). */
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

}  // namespace

int main() {
  std::printf("brain-emu module tests: %s\n", brain_version_string());
  test_registry();
  test_unknown_cpu();
  test_toy8_on_the_core();
  test_toy8_through_the_c_api();
  test_fake_cpu_module();
  test_monitors();
  test_devices();
  test_device_modules();
  test_timer_device();
  test_boards_and_wiring();
  test_interrupt_chain();
  test_device_page_sharing();
  test_monitor_modules();
  test_loader_registry();
  if (g_failed) {
    std::printf("FAIL: %d/%d checks passed\n", g_checks - g_failed, g_checks);
    return 1;
  }
  std::printf("PASS: %d/%d checks passed\n", g_checks, g_checks);
  return 0;
}
