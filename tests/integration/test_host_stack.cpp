/* brain-emu -- the integration suite: the host stack working together.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This is the suite for the seams *between* components, which no component's
 * own suite can see: a guest program running on a CPU, printing through two
 * devices, one of them raising an interrupt over a wire into the CPU; a gdb
 * stub and a command monitor driving a machine through the C API; and the host
 * services (log, filesystems) underneath all of it.
 *
 * Part 1 builds a machine the way a board does (CPU + console + UART, wired)
 * and runs a hand-assembled ARM program that prints "ok\n" to the console,
 * sends 'U' out of the UART, and enables the UART's receive interrupt - which
 * then fires over the wire into the CPU's IRQ input.
 *
 * Part 2 drives a second machine (a toy8, through the C API) with the gdb stub
 * over a memory transport and with the command monitor, which is the same
 * machine the CLI drives.
 *
 * Part 3 is the host services: a log capture, and both filesystems.
 */
#include <brainemu/brainemu.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <unistd.h>

#include <sys/stat.h>

#include "arch/arm/cpu/classic/arm9/arm926ej-s/core/state.h"
#include "brainemu/bus.h"
#include "brainemu/device.h"
#include "brainemu/machine.h"
#include "brainemu/module.h"
#include "debugger/gdb/gdbstub.h"
#include "debugger/gdb/transport.h"
#include "devices/uart/uart_device.h"
#include "host/fs/fs.h"
#include "host/log/log.h"
#include "host/monitor/command_monitor.h"
#include "testkit/test_support.h"

namespace {

/* ---- part 1: the guest ------------------------------------------------------ */

/* Hand-assembled ARM (ARMv5TEJ) program.  Vectors first (the reset entry
 * branches over them), then the code, then a branch-to-self the code never
 * falls past, then the literals the code loads:
 *
 *   prints "ok\n" to the console (0x10000000)
 *   writes 'U' to the UART's THR (0x10004000)
 *   writes 1 to the UART's IER (receive interrupt enable)
 *   unmasks IRQ in the CPSR (a reset core comes up with the I bit set)
 *   loops
 *
 * The IRQ vector holds `subs pc, lr, #4` - the canonical return - so the
 * interrupt the UART raises over the wire is taken and returned from, again
 * and again (the line is a level and the FIFO is never drained).
 */
const u32 kGuest[] = {
    0xEA000006u,  /* 0x00: b 0x20 (reset: over the vectors)          */
    0x00000000u,  /* 0x04: undef                                     */
    0x00000000u,  /* 0x08: swi                                       */
    0x00000000u,  /* 0x0c: pabort                                    */
    0x00000000u,  /* 0x10: dabort                                    */
    0x00000000u,  /* 0x14: reserved                                  */
    0xE25EF004u,  /* 0x18: subs pc, lr, #4 (IRQ: return)             */
    0x00000000u,  /* 0x1c: fiq                                       */
    0xE59F0038u,  /* 0x20: ldr r0, [pc, #0x38]  -> 0x60: console     */
    0xE59F2038u,  /* 0x24: ldr r2, [pc, #0x38]  -> 0x64: uart        */
    0xE3A0106Fu,  /* 0x28: mov r1, #'o'                              */
    0xE5C01000u,  /* 0x2c: strb r1, [r0]       (console)             */
    0xE3A0106Bu,  /* 0x30: mov r1, #'k'                              */
    0xE5C01000u,  /* 0x34: strb r1, [r0]       (console)             */
    0xE3A0100Au,  /* 0x38: mov r1, #'\n'                             */
    0xE5C01000u,  /* 0x3c: strb r1, [r0]       (console)             */
    0xE3A01055u,  /* 0x40: mov r1, #'U'                              */
    0xE5C21000u,  /* 0x44: strb r1, [r2]       (UART THR)            */
    0xE3A01001u,  /* 0x48: mov r1, #1                                */
    0xE5C21001u,  /* 0x4c: strb r1, [r2, #1]   (UART IER)            */
    0xE10F0000u,  /* 0x50: mrs r0, cpsr                              */
    0xE3C00080u,  /* 0x54: bic r0, r0, #0x80   (I bit: IRQs on)      */
    0xE121F000u,  /* 0x58: msr cpsr_c, r0                            */
    0xEAFFFFFEu,  /* 0x5c: b . (loop)                                */
    0x10000000u,  /* 0x60: console base                              */
    0x10004000u,  /* 0x64: uart base                                 */
};

std::string g_uart_tx;
void tx_hook(void*, u8 b) { g_uart_tx.push_back((char)b); }

struct RxFeed {
  std::string data;
  size_t pos = 0;
};
int rx_hook(void* user) {
  RxFeed* f = static_cast<RxFeed*>(user);
  if (f->pos >= f->data.size()) return -1;
  return (u8)f->data[f->pos++];
}

/* ---- part 2: the scripted debugger session ---------------------------------- */

const unsigned char kToyProg[] = {0x01, 0x07, 0x07, 0x07, 0x0D, 0x00, 0x1F};

std::string frame(const std::string& data) {
  unsigned sum = 0;
  for (const char c : data) sum += (unsigned char)c;
  char cs[4];
  std::snprintf(cs, sizeof(cs), "%02x", sum & 0xFFu);
  return "$" + data + "#" + cs;
}

brain_emu_t* make_toy_machine() {
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof(cfg));
  cfg.cpu_id = "toy8";
  cfg.ram_size = 1u << 16;
  brain_emu_t* e = brain_emu_create(&cfg);
  if (!e) return nullptr;
  if (brain_load_raw(e, kToyProg, sizeof kToyProg, 0x100, 0) != 0) {
    brain_emu_destroy(e);
    return nullptr;
  }
  return e;
}

bool contains(const std::string& hay, const char* needle) {
  return hay.find(needle) != std::string::npos;
}

/* ---- part 3: the log capture ------------------------------------------------- */

struct LogCapture {
  std::string text;
};
void log_sink(void* user, brain::host::LogLevel lvl, const char* msg) {
  (void)lvl;
  static_cast<LogCapture*>(user)->text += msg;
  static_cast<LogCapture*>(user)->text += "\n";
}

}  // namespace

int main() {
  brainemutest::ensure_builtins();

  /* ---- part 1: a machine the way a board builds it -------------------------- */
  std::printf("[machine: guest, devices, wire, interrupt]\n");
  {
    /* A caller-owned UART (the test keeps it, to install the host's hooks) and
     * a module-built console.  The UART is declared before the machine so it
     * outlives it: the machine never owns a caller's device. */
    brain::UartDevice uart(0x10004000u);
    RxFeed feed{"R"};
    uart.set_tx_hook(tx_hook, nullptr);
    uart.set_rx_hook(rx_hook, &feed);

    brain::Machine m;
    brain::MachineConfig c;
    c.cpu_id = "arm926";
    c.ram_size = 1u << 16;
    CHECK(m.init(c));
    CHECK(std::strcmp(m.cpu_info().id, "arm926") == 0);
    CHECK(m.add_device(uart));
    CHECK(m.add_device_by_id("console", 0, false) != nullptr);
    /* The wire: the UART's interrupt output into the CPU's IRQ input. */
    CHECK(m.add_wire("uart", "irq", "irq"));
    CHECK(m.wire_count() == 1);
    CHECK(std::strcmp(m.wire(0).device, "uart") == 0);
    CHECK(std::strcmp(m.wire(0).output, "irq") == 0);
    CHECK(std::strcmp(m.wire(0).input, "irq") == 0);

    CHECK(m.load_raw(kGuest, sizeof kGuest, 0) == 0);
    g_uart_tx.clear();
    const u64 retired = m.run_insns(10000);
    CHECK(retired == 10000);            /* the loop runs until the budget ends */

    /* The guest's outputs: the console collected "ok\n" (read through the
     * field interface, the way a tool reads any device), the UART sent 'U'. */
    const brain::Device* con = m.device_by_id("console");
    CHECK(con != nullptr);
    if (con) {
      std::string log;
      for (u32 f = 0; f < con->field_count(); ++f)
        if (std::strcmp(con->field_name(f), "log") == 0)
          log = con->field_value(f);
      CHECK(log == "ok\n");
    }
    CHECK(g_uart_tx == "U");
    CHECK(uart.tx_bytes() == 1);

    /* The UART received the host's byte (polled by tick, no guest involved)
     * and raised its interrupt: the device's line is high and the wire is
     * carrying it. */
    CHECK(uart.rx_fifo_level() == 1);
    CHECK(uart.irq_level(0) == 1);
    CHECK(m.wire_level(0) == 1);

    /* And the CPU took it: the IRQ exception was entered (and returned from,
     * over and over - the line is a level and the FIFO is never drained). */
    CHECK(m.cpu().stats().exc[BRAIN_EXC_IRQ] > 0);

    /* The fields a tool reads: the UART reports what it did. */
    CHECK(std::strcmp(uart.field_name(0), "tx_bytes") == 0);
    CHECK(std::strcmp(uart.field_value(0), "1") == 0);
  }

  /* ---- part 2: the host services over the C API ----------------------------- */
  std::printf("[gdb stub and command monitor over the C API]\n");
  {
    brain_emu_t* e = make_toy_machine();
    CHECK(e != nullptr);
    if (e) {
      /* The gdb stub: a scripted session over a memory transport - negotiate,
       * read the registers and memory, set a breakpoint, run to it, step, and
       * kill the session. */
      brain::gdb::MemTransport t;
      brain::gdb::GdbStub stub(e, &t);
      std::string script;
      script += frame("qSupported");
      script += frame("g");
      script += frame("m100,7");
      script += frame("Z0,105,1");
      script += frame("c");              /* runs to the breakpoint at 0x105 */
      script += frame("s");              /* steps the NOP */
      script += frame("k");
      t.feed(script);
      stub.serve();
      CHECK(stub.detached());
      const std::string out = t.out();
      CHECK(contains(out, "PacketSize=4000"));
      /* the g reply: six words, the PC (r3) at 0x100 -> "00010000" */
      CHECK(contains(out, "0000000000000000ff000000000100000000000000000000"));
      /* the m reply: the program bytes */
      CHECK(contains(out, "010707070d001f"));
      /* the continue stopped on the breakpoint, the step retired the NOP */
      CHECK(contains(out, "$S05#"));
      /* and the machine is where the session left it: A = 15 (LDA #7, ADD #7,
       * INC A), PC = 0x106 (past the NOP the step retired) */
      CHECK(brain_get_reg(e, 0) == 15);
      CHECK(brain_get_reg(e, 3) == 0x106);

      /* The command monitor over the same machine: the same handle the CLI
       * holds, driven one command at a time. */
      brain::host::CommandMonitor mon(e);
      CHECK(contains(mon.execute("info"), "toy8"));
      CHECK(contains(mon.execute("regs"), "r0"));
      CHECK(contains(mon.execute("reg 0"), "r0 = 0x0000000f"));
      CHECK(contains(mon.execute("mem 0x100 1"), "07070701"));
      CHECK(contains(mon.execute("stats"), "insns:"));
      CHECK(mon.execute("quit") == "bye\n");
      CHECK(mon.quit());

      brain_emu_destroy(e);
    }
  }

  /* ---- part 3: the host services -------------------------------------------- */
  std::printf("[host services: log and filesystems]\n");
  {
    /* The log: a capture sink, a level filter, and the default restored. */
    LogCapture cap;
    {
      brain::host::LogSinkScope scope(log_sink, &cap);
      brain::host::log_set_level(brain::host::LogLevel::kDebug);
      log_info("hello %d", 42);
      log_debug("hidden after the level is raised");
      brain::host::log_set_level(brain::host::LogLevel::kWarn);
      log_debug("dropped");
      log_warn("kept");
    }
    CHECK(contains(cap.text, "hello 42"));
    CHECK(contains(cap.text, "hidden after the level is raised"));
    CHECK(!contains(cap.text, "dropped"));
    CHECK(contains(cap.text, "kept"));
    /* the scope put the default sink back: this line goes to stderr, not the
     * capture */
    log_info("after the scope");
    CHECK(!contains(cap.text, "after the scope"));

    /* HostFs in a scratch directory under /tmp. */
    char tmpl[] = "/tmp/brainemu_host_stack_XXXXXX";
    const char* root = ::mkdtemp(tmpl);
    CHECK(root != nullptr);
    if (root) {
      brain::host::HostFs fs(root);
      u32 h = 0;
      CHECK(fs.open("note.txt", brain::host::kFsWrite | brain::host::kFsCreate,
                    &h) == 0);
      u32 n = 0;
      CHECK(fs.write(h, "from the host", 13, &n) == 0 && n == 13);
      CHECK(fs.close(h) == 0);
      CHECK(fs.open("note.txt", brain::host::kFsRead, &h) == 0);
      char buf[32] = {0};
      CHECK(fs.read(h, buf, 31, &n) == 0 && n == 13);
      CHECK(std::strcmp(buf, "from the host") == 0);
      CHECK(fs.close(h) == 0);
      /* the root is a sandbox */
      CHECK(fs.open("../escape", brain::host::kFsRead, &h) == -EACCES);
      ::unlink((std::string(root) + "/note.txt").c_str());
      ::rmdir(root);
    }

    /* MemFs: the same contract, no disk. */
    brain::host::MemFs mem;
    CHECK(mem.put("blob", "abc", 3) == 0);
    u32 h = 0;
    CHECK(mem.open("blob", brain::host::kFsRead, &h) == 0);
    char buf[8] = {0};
    u32 n = 0;
    CHECK(mem.read(h, buf, 8, &n) == 0 && n == 3);
    CHECK(std::memcmp(buf, "abc", 3) == 0);
    CHECK(mem.close(h) == 0);
  }

  return brainemutest::summary("integration/host-stack");
}
