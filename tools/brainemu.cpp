/* brain-emu -- command line runner for the emulator core.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This is the smallest thing that is still useful: load an image, run it for a
 * bounded amount of time, look at the machine afterwards.  It speaks only the
 * public API (brainemu/brainemu.h), so it doubles as the example of how to
 * embed the core - including how to pick a CPU module with `--cpu` and how to
 * drive one whose registers it has never heard of.
 *
 * The default CPU is this build's primary one (the ARM926EJ-S); `--list-cpus`
 * prints what the build contains.
 */
#include <brainemu/brainemu.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {

struct Options {
  std::string file;
  std::string cpu;          /* --cpu: a CPU module id, empty = default       */
  std::string cpu_opts;     /* --cpu-opt K=V, joined with ';' for the core    */
  bool elf = false;
  bool raw = true;
  bool thumb = false;
  bool be32 = false;
  bool jit = false;
  bool trace = false;
  bool show_regs = false;
  bool show_stats = false;
  uint32_t addr = 0x8000;
  uint32_t entry = 0;
  bool have_entry = false;
  uint32_t sp = 0;
  bool have_sp = false;
  uint64_t steps = 1000000;
  uint64_t cycles = 0;
  uint32_t ram_size = 32u << 20;
  uint32_t exit_port = 0;
  bool have_exit_port = false;
  uint32_t dump_addr = 0, dump_len = 0;
  bool have_dump = false;
};

/* The exit device: a write to it stops the machine and becomes the process
 * exit status.  A guest that wants to report success ends with
 * `STR Rn, [port]`, which is all a functional model needs to terminate. */
brain_emu_t* g_emu = nullptr;
uint32_t g_exit_status = 0;

uint32_t exit_read(void*, uint32_t, uint32_t) { return g_exit_status; }
void exit_write(void*, uint32_t, uint32_t value, uint32_t) {
  g_exit_status = value;
  if (getenv("BRAINEMU_DEBUG"))
    std::fprintf(stderr, "[tool] guest wrote 0x%08x, stop=%d\n", value, g_emu != nullptr);
  if (g_emu) brain_stop(g_emu);
}

uint32_t parse_num(const char* s) {
  char* end = nullptr;
  unsigned long long v = std::strtoull(s, &end, 0);
  if (!end || *end) {
    std::fprintf(stderr, "brainemu: not a number: %s\n", s);
    std::exit(2);
  }
  return (uint32_t)v;
}

uint64_t parse_num64(const char* s) {
  char* end = nullptr;
  unsigned long long v = std::strtoull(s, &end, 0);
  if (!end || *end) {
    std::fprintf(stderr, "brainemu: not a number: %s\n", s);
    std::exit(2);
  }
  return v;
}

/* One line per module this build contains, so a user can see what a build is
 * before choosing a CPU for it. */
void list_modules() {
  std::printf("modules in this build (default CPU: %s)\n", brain_default_cpu_id());
  for (uint32_t i = 0; i < brain_module_count(); ++i) {
    std::printf("  %-8s %-8s %-12s %s\n", brain_module_id(i),
                brain_module_kind(i), brain_module_name(i),
                brain_module_description(i));
  }
}

void usage(const char* argv0) {
  std::printf(
      "usage: %s [options] FILE\n"
      "\n"
      "Load and run an image on this build's default CPU (an ARM926EJ-S,\n"
      "ARMv5TEJ); --cpu picks another module, --list-cpus lists them.\n"
      "\n"
      "  --cpu ID         CPU module to run on (default: %s)\n"
      "  --cpu-opt K=V    hand an option to the CPU module (repeatable)\n"
      "  --list-cpus      list the modules this build contains and exit\n"
      "  --elf            FILE is an ELF32 little-endian ARM executable\n"
      "  --raw            FILE is a flat binary (default)\n"
      "  --addr N         load address for a raw image (default 0x8000)\n"
      "  --entry N        start address (default: load address / e_entry)\n"
      "  --thumb          start in Thumb state\n"
      "  --be32           raw image words are big-endian: swap on load\n"
      "  --sp N           initialise R13\n"
      "  --steps N        retire at most N instructions (default 1000000)\n"
      "  --cycles N       stop after N cycles instead of at --steps\n"
      "  --ram-mb N       size of the RAM window (default 32)\n"
      "  --exit-port A    map a device at A; a write there stops the run and\n"
      "                   its value becomes this process' exit status\n"
      "  --dump A:N       after the run, print N bytes at address A\n"
      "  --regs           after the run, print R0-R15, CPSR and the flags\n"
      "  --stats          after the run, print the counters\n"
      "  --jit            enable the JIT (no host backend -> interpreter)\n"
      "  --trace          trace every instruction the core retires\n",
      argv0, brain_default_cpu_id());
}

bool parse_args(int argc, char** argv, Options* o) {
  for (int i = 1; i < argc; ++i) {
    const std::string a = argv[i];
    const char* nxt = (i + 1 < argc) ? argv[i + 1] : nullptr;
    auto take = [&](const char* name) {
      if (!nxt) {
        std::fprintf(stderr, "brainemu: %s needs a value\n", name);
        std::exit(2);
      }
      return nxt;
    };
    if (a == "--elf") { o->elf = true; o->raw = false; }
    else if (a == "--raw") { o->raw = true; o->elf = false; }
    else if (a == "--thumb") o->thumb = true;
    else if (a == "--be32") o->be32 = true;
    else if (a == "--jit") o->jit = true;
    else if (a == "--trace") o->trace = true;
    else if (a == "--regs") o->show_regs = true;
    else if (a == "--stats") o->show_stats = true;
    else if (a == "--addr") { o->addr = parse_num(take("--addr")); ++i; }
    else if (a == "--entry") { o->entry = parse_num(take("--entry")); o->have_entry = true; ++i; }
    else if (a == "--sp") { o->sp = parse_num(take("--sp")); o->have_sp = true; ++i; }
    else if (a == "--steps") { o->steps = parse_num64(take("--steps")); ++i; }
    else if (a == "--cycles") { o->cycles = parse_num64(take("--cycles")); ++i; }
    else if (a == "--ram-mb") { o->ram_size = parse_num(take("--ram-mb")) << 20; ++i; }
    else if (a == "--exit-port") { o->exit_port = parse_num(take("--exit-port")); o->have_exit_port = true; ++i; }
    else if (a == "--dump") {
      const char* s = take("--dump");
      const char* colon = std::strchr(s, ':');
      if (!colon) { std::fprintf(stderr, "brainemu: --dump wants A:N\n"); return false; }
      o->dump_addr = parse_num(std::string(s, colon).c_str());
      o->dump_len = parse_num(colon + 1);
      o->have_dump = true;
      ++i;
    }
    else if (a == "--cpu") { o->cpu = take("--cpu"); ++i; }
    else if (a == "--cpu-opt") {
      const char* v = take("--cpu-opt");
      if (!o->cpu_opts.empty()) o->cpu_opts += ';';
      o->cpu_opts += v;
      ++i;
    }
    else if (a == "--list-cpus" || a == "--list-modules") {
      list_modules();
      std::exit(0);
    }
    else if (a == "-h" || a == "--help") { usage(argv[0]); std::exit(0); }
    else if (!a.empty() && a[0] == '-') {
      std::fprintf(stderr, "brainemu: unknown option %s\n", a.c_str());
      return false;
    }
    else if (o->file.empty()) o->file = a;
    else { std::fprintf(stderr, "brainemu: two files given\n"); return false; }
  }
  if (o->file.empty()) return false;
  return true;
}

std::vector<uint8_t> read_file(const std::string& path, size_t max = 64u << 20) {
  FILE* f = std::fopen(path.c_str(), "rb");
  if (!f) return {};
  std::fseek(f, 0, SEEK_END);
  const long n = std::ftell(f);
  std::fseek(f, 0, SEEK_SET);
  if (n <= 0 || (size_t)n > max) { std::fclose(f); return {}; }
  std::vector<uint8_t> v((size_t)n);
  const size_t got = std::fread(v.data(), 1, (size_t)n, f);
  std::fclose(f);
  v.resize(got);
  return v;
}

/* Registers, named the way the CPU module describes them.  The ARM926 has the
 * names everyone knows (R0-R15 with SP/LR/PC, CPSR, SPSR) and gets the flag and
 * mode breakdown too; any other module gets its numbering and no assumptions. */
void print_regs(brain_emu_t* e) {
  const int pc_reg = brain_cpu_pc_reg(e);
  const int sp_reg = brain_cpu_sp_reg(e);
  const int st_reg = brain_cpu_status_reg(e);
  const uint32_t count = brain_cpu_reg_count(e);
  const bool arm = brain_cpu_saved_status_reg(e) == BRAIN_REG_SPSR &&
                   st_reg == BRAIN_REG_CPSR;
  static const char* kArm[16] = {"R0","R1","R2","R3","R4","R5","R6","R7",
                                 "R8","R9","R10","R11","R12","SP","LR","PC"};
  for (uint32_t i = 0; i < count; ++i) {
    const char* name;
    char buf[16];
    if (arm && i < 16) name = kArm[i];
    else {
      std::snprintf(buf, sizeof buf, "r%u", i);
      name = buf;
    }
    std::printf("  %-3s = 0x%08x", name, brain_get_reg(e, (int)i));
    if ((i & 1) == 1) std::printf("\n");
    (void)pc_reg; (void)sp_reg;
  }
  if (count & 1) std::printf("\n");
  if (st_reg >= 0) {
    const uint32_t v = brain_get_reg(e, st_reg);
    if (arm) {
      static const char* kMode[8] = {"USR", "FIQ", "IRQ", "SVC", "ABT", "UND",
                                     "reserved", "SYS"};
      const uint32_t mode = v & 0x1Fu;
      std::printf("  CPSR = 0x%08x   %c%c%c%c%c  %s state, %s mode\n", v,
                  (v & (1u << 31)) ? 'N' : '-', (v & (1u << 30)) ? 'Z' : '-',
                  (v & (1u << 29)) ? 'C' : '-', (v & (1u << 28)) ? 'V' : '-',
                  (v & (1u << 27)) ? 'Q' : '-',
                  (v & (1u << 5)) ? brain_cpu_alternate_state(e) ? "Thumb" : "?"
                                  : "ARM",
                  (mode & 0x10u) ? kMode[mode & 7u] : kMode[0]);
    } else {
      std::printf("  flags = 0x%08x\n", v);
    }
  }
  const int ss = brain_cpu_saved_status_reg(e);
  if (ss >= 0) std::printf("  SPSR = 0x%08x\n", brain_get_reg(e, ss));
}

}  // namespace

int main(int argc, char** argv) {
  Options o;
  if (!parse_args(argc, argv, &o)) {
    usage(argv[0]);
    return 2;
  }

  std::vector<uint8_t> image = read_file(o.file);
  if (image.empty()) {
    std::fprintf(stderr, "brainemu: cannot read %s\n", o.file.c_str());
    return 2;
  }
  if (o.be32 && o.raw) {
    for (size_t i = 0; i + 3 < image.size(); i += 4) {
      std::swap(image[i], image[i + 3]);
      std::swap(image[i + 1], image[i + 2]);
    }
  }

  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof(cfg));
  cfg.ram_size = o.ram_size;
  cfg.cpu_id = o.cpu.empty() ? nullptr : o.cpu.c_str();
  cfg.cpu_options = o.cpu_opts.empty() ? nullptr : o.cpu_opts.c_str();
  cfg.use_jit = o.jit ? 1 : 0;
  cfg.trace = o.trace ? 1 : 0;
  brain_emu_t* e = brain_emu_create(&cfg);
  if (!e) {
    std::fprintf(stderr, "brainemu: %s\n", brain_last_error(e));
    return 2;
  }
  g_emu = e;
  std::printf("cpu: %s (%s), %s\n", brain_cpu_id(e), brain_cpu_name(e),
              brain_cpu_isa(e));

  uint32_t entry = 0;
  if (o.elf) {
    if (brain_load_elf32(e, image.data(), image.size(), &entry)) {
      std::fprintf(stderr, "brainemu: %s\n", brain_last_error(e));
      brain_emu_destroy(e);
      return 2;
    }
    o.thumb = (entry & 1u) != 0;
    std::printf("loaded elf: entry 0x%08x%s\n", entry, o.thumb ? " (Thumb)" : "");
  } else {
    if (brain_load_raw(e, image.data(), image.size(), o.addr, o.thumb ? 1 : 0)) {
      std::fprintf(stderr, "brainemu: %s\n", brain_last_error(e));
      brain_emu_destroy(e);
      return 2;
    }
    entry = o.addr | (o.thumb ? 1u : 0u);
    if (brain_cpu_alternate_state(e))
      std::printf("loaded raw: %zu bytes at 0x%08x, %s state\n", image.size(),
                  o.addr, o.thumb ? "Thumb" : "ARM");
    else
      std::printf("loaded raw: %zu bytes at 0x%08x\n", image.size(), o.addr);
  }
  /* The entry and stack go through the register indices the CPU module
   * reports, so this works for a CPU whose PC is not register 15. */
  const int pc_reg = brain_cpu_pc_reg(e);
  const int sp_reg = brain_cpu_sp_reg(e);
  if (o.have_entry && pc_reg >= 0)
    brain_set_reg(e, pc_reg, o.entry | (o.thumb ? 1u : 0u));
  if (o.have_sp && sp_reg >= 0) brain_set_reg(e, sp_reg, o.sp);
  if (o.have_exit_port) {
    if (brain_map_device(e, o.exit_port, 4096, exit_read, exit_write, nullptr,
                         "exit")) {
      std::fprintf(stderr, "brainemu: %s\n", brain_last_error(e));
      brain_emu_destroy(e);
      return 2;
    }
    std::printf("exit port at 0x%08x\n", o.exit_port);
  }

  const uint64_t retired =
      o.cycles ? brain_run_cycles(e, o.cycles) : brain_run_insns(e, o.steps);
  std::printf("ran %llu %s\n", (unsigned long long)retired,
              o.cycles ? "cycles-worth of instructions" : "instructions");
  const int exc = brain_pending_exception(e);
  if (exc) std::printf("pending exception: %s\n", brain_exception_name(exc));

  if (o.show_regs) { std::printf("registers:\n"); print_regs(e); }
  if (o.have_dump) {
    std::printf("memory at 0x%08x:\n", o.dump_addr);
    for (uint32_t i = 0; i < o.dump_len; i += 16) {
      std::printf("  %08x:", o.dump_addr + i);
      for (uint32_t j = 0; j < 16 && i + j < o.dump_len; ++j)
        std::printf(" %02x", brain_peek8(e, o.dump_addr + i + j) & 0xFF);
      std::printf("\n");
    }
  }
  if (o.show_stats) {
    brain_stats_t s;
    brain_get_stats(e, &s);
    std::printf("stats:\n");
    std::printf("  insns        %llu  (interp %llu, jit %llu)\n",
                (unsigned long long)s.insns, (unsigned long long)s.insns_interp,
                (unsigned long long)s.insns_jit);
    std::printf("  cycles       %llu\n", (unsigned long long)s.cycles);
    std::printf("  blocks       %llu  flushes %llu (smc %llu)\n",
                (unsigned long long)s.blocks,
                (unsigned long long)s.flushes, (unsigned long long)s.smc_flushes);
    std::printf("  tlb          hits %llu miss %llu walks %llu faults %llu\n",
                (unsigned long long)s.tlb_hits, (unsigned long long)s.tlb_miss,
                (unsigned long long)s.walks, (unsigned long long)s.page_faults);
    std::printf("  branches     taken %llu not %llu\n",
                (unsigned long long)s.branch_taken,
                (unsigned long long)s.branch_not);
    for (int i = 1; i < BRAIN_EXC_COUNT; ++i)
      if (s.exc[i])
        std::printf("  exc %-8s %llu\n", brain_exception_name(i),
                    (unsigned long long)s.exc[i]);
  }

  const int status = o.have_exit_port ? (int)g_exit_status : 0;
  brain_emu_destroy(e);
  return status;
}
