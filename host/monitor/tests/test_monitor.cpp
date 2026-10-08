/* brain-emu -- the host monitors' test suite.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Two things are tested, both against a machine built through the C API (a
 * toy8 with five instructions: LDA #7, ADD #7, INC A, NOP, HALT):
 *
 *   - `CommandMonitor`: every command is one `execute()` call whose output is
 *     text, so the whole console is testable without a terminal.
 *   - `StatsMonitor`: the observer the CLI's `--monitor stats` installs,
 *     driven through the machine and read back through the monitor fields.
 */
#include <brainemu/brainemu.h>

#include <cstdio>
#include <cstring>
#include <string>

#include "brainemu/module.h"
#include "brainemu/monitor.h"
#include "host/monitor/command_monitor.h"
#include "testkit/test_support.h"

namespace {

/* LDA #7, ADD #7, INC A, NOP, HALT - five instructions, r0 ends at 15. */
const unsigned char kProg[] = {0x01, 0x07, 0x07, 0x07, 0x0D, 0x00, 0x1F};

brain_emu_t* make_machine() {
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof(cfg));
  cfg.cpu_id = "toy8";
  cfg.ram_size = 1u << 16;
  brain_emu_t* e = brain_emu_create(&cfg);
  if (!e) return nullptr;
  if (brain_load_raw(e, kProg, sizeof kProg, 0x100, 0) != 0) {
    brain_emu_destroy(e);
    return nullptr;
  }
  return e;
}

bool contains(const std::string& hay, const char* needle) {
  return hay.find(needle) != std::string::npos;
}

}  // namespace

int main() {
  brainemutest::ensure_builtins();

  /* The module is in the registry, with the id the CLI's --monitor takes. */
  {
    const brain::MonitorModule* m =
        brain::ModuleRegistry::instance().monitor_by_id("stats");
    CHECK(m != nullptr);
    if (m) {
      CHECK(std::strcmp(m->info().id, "stats") == 0);
      CHECK(m->info().kind == brain::ModuleKind::Monitor);
      auto mon = m->create();
      CHECK(mon != nullptr);
      CHECK(mon->field_count() == 4);
      CHECK(std::strcmp(mon->field_name(0), "slices") == 0);
    }
  }

  brain_emu_t* e = make_machine();
  CHECK(e != nullptr);
  if (!e) return brainemutest::summary("host/monitor");
  CHECK(brain_add_device_module(e, "console", 0, 0) == 0);
  CHECK(brain_add_monitor_module(e, "stats") == 0);

  brain::host::CommandMonitor mon(e);

  /* info: what machine this is. */
  {
    const std::string out = mon.execute("info");
    CHECK(contains(out, "toy8"));
    CHECK(contains(out, "version:"));
  }
  /* help lists the commands; an empty line is nothing; an unknown command is
   * named. */
  CHECK(contains(mon.execute("help"), "continue"));
  CHECK(mon.execute("").empty());
  CHECK(contains(mon.execute("frobnicate"), "unknown command"));

  /* regs / reg: the five toy8 registers, and a read/write of one. */
  CHECK(contains(mon.execute("regs"), "r0"));
  CHECK(contains(mon.execute("reg 0"), "r0 = 0x00000000"));
  CHECK(contains(mon.execute("reg 0 42"), "r0 = 0x0000002a"));
  CHECK(contains(mon.execute("reg 0"), "r0 = 0x0000002a"));
  /* A read of a register that does not exist reports zero; a write to one is
   * refused. */
  CHECK(contains(mon.execute("reg 99"), "r99 = 0x00000000"));
  CHECK(mon.execute("reg 99 1") == "no such register\n");

  /* mem / x: the program as the guest reads it, and the gdb spelling. */
  {
    /* LDA #7 is the 16-bit little-endian word 0x0701 at 0x100. */
    const std::string out = mon.execute("mem 0x100 2");
    CHECK(contains(out, "0x00000100:"));
    CHECK(contains(out, "0701"));
    CHECK(contains(out, "0707"));
    const std::string xout = mon.execute("x/2xw 0x100");
    CHECK(contains(xout, "0x00000100:"));
    CHECK(contains(xout, "0701"));
    /* the word at 0x100 is LDA #7 and ADD #7: bytes 01 07 07 07 = 0x07070701 */
    const std::string dout = mon.execute("x/1dw 0x100");
    CHECK(contains(dout, "117901057"));       /* 0x07070701 in decimal */
    const std::string cout = mon.execute("x/1cw 0x100");
    CHECK(contains(cout, "..."));
  }

  /* set: write a word, read it back through the bus. */
  CHECK(contains(mon.execute("set 0x200 0x12345678"), "0x12345678"));
  CHECK(contains(mon.execute("mem 0x200 1"), "12345678"));

  /* step: one instruction, and the PC is reported. */
  CHECK(contains(mon.execute("step"), "pc = 0x00000102"));
  CHECK(contains(mon.execute("reg 0"), "r0 = 0x00000007"));   /* LDA #7 */

  /* break / continue: the breakpoint is a stop - the run steps until the PC
   * reaches it and reports how many instructions that took.  The NOP sits at
   * 0x105, three instructions in. */
  CHECK(contains(mon.execute("break 0x105"), "breakpoint at 0x00000105"));
  {
    const std::string out = mon.execute("continue");
    CHECK(contains(out, "breakpoint at 0x00000105"));
    CHECK(contains(out, "after 2 insn(s)"));
  }
  /* Still on the breakpoint: continuing again stops immediately. */
  CHECK(contains(mon.execute("continue"), "after 0 insn(s)"));
  /* delete drops it, and the run then goes to the HALT. */
  CHECK(contains(mon.execute("delete"), "dropped"));
  CHECK(contains(mon.execute("continue"), "stopped after 2 insn(s)"));
  /* A halted machine has nothing left to say. */
  CHECK(contains(mon.execute("continue"), "stopped after 0 insn(s)"));

  /* step [N] on a halted machine retires nothing and the PC stays where the
   * HALT left it: past the opcode, at 0x107. */
  CHECK(contains(mon.execute("step 3"), "pc = 0x00000107"));
  CHECK(brain_get_reg(e, brain_cpu_pc_reg(e)) == 0x107);

  /* devices / stats: what is mapped, and the machine's counters. */
  {
    const std::string out = mon.execute("devices");
    CHECK(contains(out, "console @ 0x10000000"));
    CHECK(contains(out, "bytes="));
  }
  {
    const std::string out = mon.execute("stats");
    CHECK(contains(out, "insns:"));
    CHECK(contains(out, "monitor stats:"));
    CHECK(contains(out, "slices="));
  }

  /* reset: the machine goes back to the CPU's reset state (a toy8 starts at
   * PC 0, and the entry a loader set is not part of it), and the breakpoints go
   * with it.  The monitor's `reg` puts the PC back on the program, which is how
   * a user re-runs an image after a reset. */
  CHECK(mon.execute("reset") == "machine reset\n");
  CHECK(brain_get_reg(e, brain_cpu_pc_reg(e)) == 0);
  CHECK(contains(mon.execute("reg 3 0x100"), "r3 = 0x00000100"));   /* toy8: PC is r3 */
  CHECK(contains(mon.execute("continue"), "stopped after 5 insn(s)"));

  /* The stats monitor counted what the runs did, as fields. */
  {
    CHECK(brain_monitor_count(e) == 1);
    CHECK(std::strcmp(brain_monitor_id(e, 0), "stats") == 0);
    /* slices and retired moved with the runs above */
    u64 slices = 0, retired = 0, resets = 0;
    for (u32 f = 0; f < brain_monitor_field_count(e, 0); ++f) {
      const char* name = brain_monitor_field_name(e, 0, f);
      const u64 v = std::strtoull(brain_monitor_field_value(e, 0, f), nullptr, 10);
      if (std::strcmp(name, "slices") == 0) slices = v;
      if (std::strcmp(name, "retired") == 0) retired = v;
      if (std::strcmp(name, "resets") == 0) resets = v;
    }
    CHECK(slices > 0);
    CHECK(retired >= 10);          /* two full runs of the five-instruction program */
    /* a reset is counted, exactly one per brain_reset */
    brain_reset(e);
    u64 resets2 = 0;
    for (u32 f = 0; f < brain_monitor_field_count(e, 0); ++f)
      if (std::strcmp(brain_monitor_field_name(e, 0, f), "resets") == 0)
        resets2 = std::strtoull(brain_monitor_field_value(e, 0, f), nullptr, 10);
    CHECK(resets2 == resets + 1);
  }

  /* quit ends the REPL. */
  CHECK(!mon.quit());
  CHECK(mon.execute("quit") == "bye\n");
  CHECK(mon.quit());

  brain_emu_destroy(e);
  return brainemutest::summary("host/monitor");
}
