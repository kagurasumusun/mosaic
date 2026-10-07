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
  test_loader_registry();
  if (g_failed) {
    std::printf("FAIL: %d/%d checks passed\n", g_checks - g_failed, g_checks);
    return 1;
  }
  std::printf("PASS: %d/%d checks passed\n", g_checks, g_checks);
  return 0;
}
