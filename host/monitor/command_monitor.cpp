/* brain-emu -- the command monitor: one command per function, all of them a
 * C API call.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The command set is the small debugger a bare-metal image needs: look at the
 * registers and memory, poke either, stop at an address, step, run, reset, and
 * print what the machine and its devices have been doing.  `continue` is the
 * one command with a loop in it: with breakpoints set it steps and checks the
 * PC after every instruction (a breakpoint is a stop, not a filter), without
 * them it runs whole slices and reports when the machine stops on its own -
 * an exit device, a fault - which `brain_run_insns` returning 0 is how the
 * caller learns.
 */
#include "host/monitor/command_monitor.h"

#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

namespace brain::host {

namespace {

std::vector<std::string> split(const std::string& line) {
  std::vector<std::string> out;
  size_t i = 0;
  while (i < line.size()) {
    while (i < line.size() && (line[i] == ' ' || line[i] == '\t')) ++i;
    const size_t start = i;
    while (i < line.size() && line[i] != ' ' && line[i] != '\t') ++i;
    if (i > start) out.push_back(line.substr(start, i - start));
  }
  return out;
}

void appendf(std::string* out, const char* fmt, ...)
    __attribute__((format(printf, 2, 3)));
void appendf(std::string* out, const char* fmt, ...) {
  char buf[512];
  va_list ap;
  va_start(ap, fmt);
  std::vsnprintf(buf, sizeof(buf), fmt, ap);
  va_end(ap);
  *out += buf;
}

}  // namespace

bool CommandMonitor::parse_u32(const std::string& s, u32* out) {
  u64 v;
  if (!parse_u64(s, &v) || v > 0xFFFFFFFFull) return false;
  *out = (u32)v;
  return true;
}

bool CommandMonitor::parse_u64(const std::string& s, u64* out) {
  if (s.empty()) return false;
  int base = 10;
  size_t i = 0;
  if (s.size() > 2 && s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) {
    base = 16;
    i = 2;
  }
  if (i >= s.size()) return false;
  u64 v = 0;
  for (; i < s.size(); ++i) {
    const char c = s[i];
    int d;
    if (c >= '0' && c <= '9') d = c - '0';
    else if (base == 16 && c >= 'a' && c <= 'f') d = c - 'a' + 10;
    else if (base == 16 && c >= 'A' && c <= 'F') d = c - 'A' + 10;
    else return false;
    v = v * (u64)base + (u64)d;
  }
  *out = v;
  return true;
}

bool CommandMonitor::hit_break(u32 pc) const {
  for (u32 b : breaks_)
    if (b == pc) return true;
  return false;
}

std::string CommandMonitor::execute(const std::string& line) {
  const std::vector<std::string> args = split(line);
  if (args.empty()) return "";
  const std::string& cmd = args[0];
  const std::vector<std::string> rest(args.begin() + 1, args.end());

  if (cmd == "help" || cmd == "h" || cmd == "?") return cmd_help();
  if (cmd == "info") return cmd_info();
  if (cmd == "regs" || cmd == "registers") return cmd_regs();
  if (cmd == "devices" || cmd == "dev") return cmd_devices();
  if (cmd == "stats") return cmd_stats();
  if (cmd == "mem" || cmd == "x" || cmd.rfind("x/", 0) == 0)
    return cmd_mem(line);   /* the x/Nf prefix rides on the command word */
  if (cmd == "set") return cmd_set(rest);
  if (cmd == "reg") return cmd_reg(rest);
  if (cmd == "break" || cmd == "b") return cmd_break(rest);
  if (cmd == "delete" || cmd == "d") return cmd_delete(rest);
  if (cmd == "step" || cmd == "s") return cmd_step(rest);
  if (cmd == "continue" || cmd == "c") return cmd_cont(rest);
  if (cmd == "reset") return cmd_reset();
  if (cmd == "quit" || cmd == "q") {
    quit_ = true;
    return "bye\n";
  }
  std::string out = "unknown command: ";
  out += cmd;
  out += " (try 'help')\n";
  return out;
}

std::string CommandMonitor::cmd_help() {
  return "commands:\n"
         "  help                 this list\n"
         "  info                 what machine this is (cpu, isa, registers)\n"
         "  regs                 all registers\n"
         "  reg [i [value]]      read register i, or set it\n"
         "  mem ADDR [N]         N words at ADDR (default 4), as the guest reads\n"
         "  x[/N][x|d|c] ADDR    gdb-style: x/4xw 0x1000\n"
         "  set ADDR VALUE       write a word\n"
         "  break ADDR           stop when the PC reaches ADDR\n"
         "  delete [ADDR]        drop one breakpoint, or all of them\n"
         "  step [N]             retire N instructions (default 1)\n"
         "  continue [N]         run until a breakpoint, a stop, or N insns\n"
         "  devices              the devices mapped, and what they report\n"
         "  stats                the machine's counters\n"
         "  reset                machine reset\n"
         "  quit                 leave the monitor\n";
}

std::string CommandMonitor::cmd_info() {
  std::string out;
  appendf(&out, "cpu:      %s (%s)\n", brain_cpu_id(e_), brain_cpu_name(e_));
  appendf(&out, "isa:      %s\n", brain_cpu_isa(e_));
  appendf(&out, "version:  %s\n", brain_version_string());
  appendf(&out, "regs:     %u general, pc=r%d, cpsr=r%d, spsr=r%d, sp=r%d%s\n",
          brain_cpu_reg_count(e_), brain_cpu_pc_reg(e_), brain_cpu_status_reg(e_),
          brain_cpu_saved_status_reg(e_), brain_cpu_sp_reg(e_),
          brain_cpu_alternate_state(e_) ? ", thumb selectable" : "");
  const int exc = brain_pending_exception(e_);
  if (exc != BRAIN_EXC_NONE)
    appendf(&out, "pending:  %s\n", brain_exception_name(exc));
  return out;
}

std::string CommandMonitor::cmd_regs() {
  std::string out;
  const u32 n = brain_cpu_reg_count(e_);
  const int pc = brain_cpu_pc_reg(e_);
  const int cpsr = brain_cpu_status_reg(e_);
  const int spsr = brain_cpu_saved_status_reg(e_);
  for (u32 i = 0; i < n; ++i) {
    appendf(&out, "r%-2u = 0x%08x%s", i, brain_get_reg(e_, (int)i),
            (int)i == pc ? "  (pc)" : "");
    out += ((i % 4) == 3 || i + 1 == n) ? "\n" : "   ";
  }
  if (cpsr >= 0) appendf(&out, "cpsr = 0x%08x\n", brain_get_reg(e_, cpsr));
  if (spsr >= 0) appendf(&out, "spsr = 0x%08x\n", brain_get_reg(e_, spsr));
  return out;
}

std::string CommandMonitor::cmd_devices() {
  std::string out;
  const u32 n = brain_device_count(e_);
  if (n == 0) return "(no devices)\n";
  for (u32 i = 0; i < n; ++i) {
    appendf(&out, "%s @ 0x%08x", brain_device_instance(e_, i),
            brain_device_base(e_, i));
    const u32 nf = brain_device_field_count(e_, i);
    for (u32 f = 0; f < nf; ++f) {
      appendf(&out, "%s%s=%s", f ? ", " : "  ", brain_device_field_name(e_, i, f),
              brain_device_field_value(e_, i, f));
    }
    out += "\n";
  }
  return out;
}

std::string CommandMonitor::cmd_stats() {
  brain_stats_t st;
  brain_get_stats(e_, &st);
  std::string out;
  appendf(&out, "insns:    %llu  (interp %llu, jit %llu)\n",
          (unsigned long long)st.insns, (unsigned long long)st.insns_interp,
          (unsigned long long)st.insns_jit);
  appendf(&out, "cycles:   %llu\n", (unsigned long long)st.cycles);
  appendf(&out, "blocks:   %llu  flushes %llu (smc %llu)\n",
          (unsigned long long)st.blocks, (unsigned long long)st.flushes,
          (unsigned long long)st.smc_flushes);
  appendf(&out, "tlb:      hits %llu, misses %llu, walks %llu, faults %llu\n",
          (unsigned long long)st.tlb_hits, (unsigned long long)st.tlb_miss,
          (unsigned long long)st.walks, (unsigned long long)st.page_faults);
  appendf(&out, "branches: taken %llu, not %llu\n",
          (unsigned long long)st.branch_taken,
          (unsigned long long)st.branch_not);
  for (int i = 1; i < BRAIN_EXC_COUNT; ++i)
    if (st.exc[i]) appendf(&out, "exc %-4s %llu\n", brain_exception_name(i),
                          (unsigned long long)st.exc[i]);
  const u32 nm = brain_monitor_count(e_);
  for (u32 m = 0; m < nm; ++m) {
    appendf(&out, "monitor %s:", brain_monitor_id(e_, m));
    const u32 nf = brain_monitor_field_count(e_, m);
    for (u32 f = 0; f < nf; ++f)
      appendf(&out, " %s=%s", brain_monitor_field_name(e_, m, f),
              brain_monitor_field_value(e_, m, f));
    out += "\n";
  }
  return out;
}

std::string CommandMonitor::cmd_mem(const std::string& line) {
  /* Two spellings: `mem ADDR [N]`, and the gdb one `x[/N][x|d|c] ADDR` where
   * the count and format ride on the command word itself. */
  const std::vector<std::string> tok = split(line);
  if (tok.size() < 2) return "usage: mem ADDR [N]  |  x[/N][x|d|c] ADDR\n";
  u32 count = 4;
  char fmt = 'x';
  size_t ai = 1;
  if (tok[0] == "x" || tok[0] == "mem") {
    /* plain command word: the address is the next token */
  } else if (tok[0].size() > 2 && tok[0][0] == 'x' && tok[0][1] == '/') {
    const std::string spec = tok[0].substr(2);
    size_t digits = 0;
    while (digits < spec.size() && spec[digits] >= '0' && spec[digits] <= '9')
      ++digits;
    if (digits > 0) {
      u64 c;
      if (parse_u64(spec.substr(0, digits), &c) && c > 0 && c <= 1024)
        count = (u32)c;
    }
    if (digits < spec.size()) fmt = spec[digits];
  } else {
    return "usage: mem ADDR [N]  |  x[/N][x|d|c] ADDR\n";
  }
  u32 addr = 0;
  if (ai >= tok.size() || !parse_u32(tok[ai], &addr))
    return "usage: mem ADDR [N]  |  x[/N][x|d|c] ADDR\n";
  if (ai + 1 < tok.size()) {
    u64 c;
    if (parse_u64(tok[ai + 1], &c) && c > 0 && c <= 1024) count = (u32)c;
  }

  std::string out;
  for (u32 i = 0; i < count; ++i) {
    const u32 a = addr + i * 4;
    const u32 v = brain_read32(e_, a);
    if ((i % 4) == 0) appendf(&out, "0x%08x:", a);
    if (fmt == 'd') appendf(&out, " %10u", v);
    else if (fmt == 'c') {
      appendf(&out, "  ");
      for (int b = 0; b < 4; ++b) {
        const u8 ch = (u8)(v >> (8 * b));
        out += (ch >= 0x20 && ch < 0x7F) ? (char)ch : '.';
      }
    } else {
      appendf(&out, " %08x", v);
    }
    if ((i % 4) == 3 || i + 1 == count) out += "\n";
  }
  return out;
}

std::string CommandMonitor::cmd_set(const std::vector<std::string>& args) {
  u32 addr, value;
  if (args.size() < 2 || !parse_u32(args[0], &addr) ||
      !parse_u32(args[1], &value))
    return "usage: set ADDR VALUE\n";
  brain_write32(e_, addr, value);
  std::string out;
  appendf(&out, "0x%08x = 0x%08x\n", addr, brain_read32(e_, addr));
  return out;
}

std::string CommandMonitor::cmd_reg(const std::vector<std::string>& args) {
  if (args.empty()) return cmd_regs();
  u32 i;
  if (!parse_u32(args[0], &i)) return "usage: reg [i [value]]\n";
  if (args.size() >= 2) {
    u32 v;
    if (!parse_u32(args[1], &v)) return "usage: reg [i [value]]\n";
    if (brain_set_reg(e_, (int)i, v) != 0) return "no such register\n";
  }
  std::string out;
  appendf(&out, "r%u = 0x%08x\n", i, brain_get_reg(e_, (int)i));
  return out;
}

std::string CommandMonitor::cmd_break(const std::vector<std::string>& args) {
  u32 addr;
  if (args.size() < 1 || !parse_u32(args[0], &addr))
    return "usage: break ADDR\n";
  breaks_.push_back(addr);
  std::string out;
  appendf(&out, "breakpoint at 0x%08x (%u set)\n", addr, (u32)breaks_.size());
  return out;
}

std::string CommandMonitor::cmd_delete(const std::vector<std::string>& args) {
  if (args.empty()) {
    const u32 n = (u32)breaks_.size();
    breaks_.clear();
    std::string out;
    appendf(&out, "dropped %u breakpoint(s)\n", n);
    return out;
  }
  u32 addr;
  if (!parse_u32(args[0], &addr)) return "usage: delete [ADDR]\n";
  for (size_t i = 0; i < breaks_.size(); ++i)
    if (breaks_[i] == addr) {
      breaks_.erase(breaks_.begin() + i);
      std::string out;
      appendf(&out, "dropped breakpoint at 0x%08x\n", addr);
      return out;
    }
  return "no breakpoint at that address\n";
}

std::string CommandMonitor::cmd_step(const std::vector<std::string>& args) {
  u64 n = 1;
  if (!args.empty() && !parse_u64(args[0], &n)) return "usage: step [N]\n";
  if (n == 0) n = 1;
  for (u64 i = 0; i < n; ++i) {
    if (brain_step(e_) != 0) break;
    if (quit_) break;
  }
  std::string out;
  const int pc = brain_cpu_pc_reg(e_);
  appendf(&out, "pc = 0x%08x\n", pc >= 0 ? brain_get_reg(e_, pc) : 0u);
  return out;
}

std::string CommandMonitor::cmd_cont(const std::vector<std::string>& args) {
  u64 want = 0;   /* 0 = no limit */
  if (!args.empty() && !parse_u64(args[0], &want)) return "usage: continue [N]\n";
  std::string out;
  if (breaks_.empty()) {
    /* No breakpoints: run slices and stop when the machine stops on its own
     * (an exit device, a fault - `brain_run_insns` returning 0 is how the
     * caller learns) or when the instruction budget is spent. */
    u64 total = 0;
    for (;;) {
      const u64 slice = want ? (want - total < 1024 ? want - total : 1024) : 1024;
      if (want && total >= want) break;
      const u64 r = brain_run_insns(e_, slice);
      total += r;
      if (r == 0) {
        appendf(&out, "stopped after %llu insn(s)\n", (unsigned long long)total);
        break;
      }
      if (quit_) {
        appendf(&out, "interrupted after %llu insn(s)\n", (unsigned long long)total);
        break;
      }
    }
  } else {
    /* Breakpoints set: step and check the PC after every instruction - a
     * breakpoint is a stop, not a filter, so this is the only honest way to
     * honour one at a memory address the run loop does not know about. */
    const int pci = brain_cpu_pc_reg(e_);
    u64 steps = 0;
    for (;;) {
      if (want && steps >= want) {
        appendf(&out, "ran %llu insn(s)\n", (unsigned long long)steps);
        break;
      }
      if (pci >= 0 && hit_break(brain_get_reg(e_, pci))) {
        appendf(&out, "breakpoint at 0x%08x after %llu insn(s)\n",
                brain_get_reg(e_, pci), (unsigned long long)steps);
        break;
      }
      if (brain_step(e_) != 0) {
        appendf(&out, "stopped after %llu insn(s)\n", (unsigned long long)steps);
        break;
      }
      ++steps;
      if (quit_) break;
    }
  }
  return out;
}

std::string CommandMonitor::cmd_reset() {
  brain_reset(e_);
  breaks_.clear();
  return "machine reset\n";
}

void CommandMonitor::run() {
  std::printf("brain-emu monitor; 'help' for commands, 'quit' to leave\n");
  std::string line;
  for (;;) {
    std::printf("(brain) ");
    std::fflush(stdout);
    if (!std::getline(std::cin, line)) break;   /* EOF: leave the monitor */
    /* strip the newline getline keeps */
    while (!line.empty() && (line.back() == '\n' || line.back() == '\r'))
      line.pop_back();
    const std::string out = execute(line);
    if (!out.empty()) std::fputs(out.c_str(), stdout);
    if (quit_) break;
  }
}

}  // namespace brain::host
