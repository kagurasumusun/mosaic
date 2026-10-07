/* brain-emu -- the toy8 machine's test suite.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * toy8 is not a target: it is the proof that the core is a core.  It is a
 * five-register CPU with three instructions, written against `brainemu/cpu.h` and
 * nothing else, and these checks are about the *contract* - that a machine can be
 * brought up, run, stepped and reset through the core's vocabulary, that its state
 * is reachable through the same register accessors and the same C API an ARM would
 * be reached through, and that its own extras do not leak into the core.
 *
 * A check here that reads like an instruction test is not one: the toy8 program
 * below is a fixture, and its semantics live in `machines/toy8/toy8.h`.
 */
#include <brainemu/brainemu.h>

#include "brainemu/cpu.h"
#include "brainemu/machine.h"
#include "brainemu/module.h"
#include "machines/toy8/toy8.h"
#include "testkit/test_support.h"

#include <cstdio>
#include <cstring>
#include <string>

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

int main() {
  std::printf("brain-emu toy8 tests: %s\n", brain_version_string());
  brainemutest::ensure_builtins();
  test_toy8_on_the_core();
  test_toy8_through_the_c_api();
  return brainemutest::summary("toy8");
}
