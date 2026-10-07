/* brain-emu -- throughput benchmark.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Builds its own guest program (a counted add/cmp/branch loop, hand encoded --
 * no assembler is needed to measure dispatch), runs it under the interpreter or
 * the JIT, and reports instruction and cycle rates plus the counters that
 * explain the difference.
 */
#include <brainemu/brainemu.h>

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {

/* The loop, hand encoded for ARM state:
 *   add r1, r1, #1      e2811001
 *   cmp r1, r2          e1510002
 *   bne -8              1afffffc   (target = pc + 8 + -4*4 = the add)
 *   b .                 eafffffe   (so an escaped loop is still a halt)
 * R1 counts up, R2 holds the trip count; the run is also capped by --insns. */
const uint32_t kLoopArm[] = {0xE2811001u, 0xE1510002u, 0x1AFFFFFCu,
                             0xEAFFFFFEu};

/* The same shape in Thumb-1 (three halfwords, the branch's PC is insn+4):
 *   adds r1, #1         3101   00111 iiiiiiii
 *   cmp r1, r2          4291   010000 1010 Rm=010 Rd=001
 *   bne -8              d1fc   1101 0001 imm8=-4 -> start+8-8 = start
 * The trip count and counter registers come from the harness, which keeps the
 * encodings to the three the loop needs. */
const uint32_t kLoopThumb[] = {0x3101u, 0x4291u, 0xD1FCu};

struct Opts {
  uint64_t insns = 50000000ull;
  uint32_t trips = 0;   /* 0 = derive from the instruction budget */
  bool jit = false;
  bool thumb = false;
};

}  // namespace

int main(int argc, char** argv) {
  Opts o;
  for (int i = 1; i < argc; ++i) {
    const std::string a = argv[i];
    if (a == "--jit") o.jit = true;
    else if (a == "--no-jit") o.jit = false;
    else if (a == "--thumb") o.thumb = true;
    else if (a == "--insns" && i + 1 < argc) o.insns = std::strtoull(argv[++i], 0, 0);
    else if (a == "--trips" && i + 1 < argc) o.trips = (uint32_t)std::strtoul(argv[++i], 0, 0);
    else {
      std::printf("usage: %s [--jit] [--thumb] [--insns N] [--trips N]\n",
                  argv[0]);
      return 2;
    }
  }

  std::vector<uint8_t> image;
  if (o.thumb) {
    for (uint32_t w : kLoopThumb) {
      const uint16_t h = (uint16_t)w;
      image.insert(image.end(), (const uint8_t*)&h, (const uint8_t*)&h + 2);
    }
    image.resize(sizeof kLoopThumb);
    std::memcpy(image.data(), kLoopThumb, sizeof kLoopThumb);
  } else {
    image.resize(sizeof kLoopArm);
    std::memcpy(image.data(), kLoopArm, sizeof kLoopArm);
  }

  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof(cfg));
  cfg.ram_size = 4u << 20;
  cfg.use_jit = o.jit ? 1 : 0;
  cfg.cycle_model = 1;
  brain_emu_t* e = brain_emu_create(&cfg);
  if (!e) { std::fprintf(stderr, "bench: %s\n", brain_last_error(e)); return 1; }

  if (brain_load_raw(e, image.data(), image.size(), 0x8000, o.thumb ? 1 : 0)) {
    std::fprintf(stderr, "bench: %s\n", brain_last_error(e));
    return 1;
  }
  /* One loop iteration is 3 instructions, so a trip count of insns/3 walks the
   * counted loop exactly once before the budget runs out. */
  const uint32_t trips = o.trips ? o.trips : (uint32_t)(o.insns / 3 + 1);
  brain_set_reg(e, 2, trips);
  brain_set_reg(e, 1, 0);

  const auto t0 = std::chrono::steady_clock::now();
  const uint64_t retired = brain_run_insns(e, o.insns);
  const auto t1 = std::chrono::steady_clock::now();
  const double secs = std::chrono::duration<double>(t1 - t0).count();

  brain_stats_t s;
  brain_get_stats(e, &s);
  std::printf("%s %s: %llu insns in %.3f s = %.1f MInsn/s, %.2f cycles/insn\n",
              o.thumb ? "thumb" : "arm", o.jit ? "jit" : "interp",
              (unsigned long long)retired, secs,
              secs > 0 ? (double)retired / secs / 1e6 : 0.0,
              retired ? (double)s.cycles / (double)retired : 0.0);
  std::printf("  engine split: interp %llu jit %llu | blocks %llu flushes %llu\n",
              (unsigned long long)s.insns_interp, (unsigned long long)s.insns_jit,
              (unsigned long long)s.blocks, (unsigned long long)s.flushes);
  std::printf("  tlb hits %llu miss %llu | code bytes %llu\n",
              (unsigned long long)s.tlb_hits, (unsigned long long)s.tlb_miss,
              (unsigned long long)s.code_bytes);
  if (s.insns != retired)
    std::printf("  note: counters report %llu retired instructions\n",
                (unsigned long long)s.insns);
  brain_emu_destroy(e);
  return 0;
}
