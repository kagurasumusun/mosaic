/* brain-emu -- unit tests for the API surface around the core.
 * SPDX-file-note: see LICENSE.
 *
 * These cover the parts a golden-instruction corpus cannot: the image loaders,
 * the device bus, register access with the pipeline aliasing, and the counters.
 * Instruction semantics live in tests/test_instr.cpp (corpus-driven), so a
 * check here that *looks* like an instruction test probably belongs there.
 */
#include <brainemu/brainemu.h>

#include "core/ops.h"   /* shift_arm(), the primitive both engines share */
#include "core/cpu.h"
#include "jit/cache.h"
#include "jit/jit.h"   /* BRAIN_JIT_BACKEND (must be 0 on this host) */

#include <cstdio>
#include <cstring>
#include <vector>

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

/* Hand-encoded programs; the corpus owns the semantics, these only need to
 * retire cleanly and touch the registers the test then reads back. */
const uint32_t kProgAdd[] = {
    0xE3A00005u,  /* mov  r0, #5   */
    0xE2800001u,  /* add  r0, r0, #1 */
    0xEAFFFFFEu,  /* b    .        */
};

const uint16_t kThumbProg[] = {
    0x2003,  /* movs r0, #3 */
    0x3002,  /* adds r0, #2 */
    0xE7FE,  /* b    .      */
};

/* A minimal ELF32 header + one PT_LOAD, little-endian, EM_ARM. */
std::vector<uint8_t> make_elf(uint32_t load_addr, const uint8_t* code,
                              size_t code_len, uint32_t entry,
                              uint32_t memsz_extra = 0) {
  const uint32_t ehsize = 52, phentsize = 32, phnum = 1;
  const uint32_t total = (uint32_t)(ehsize + phentsize * phnum + code_len);
  std::vector<uint8_t> b(total, 0);
  auto put16 = [&](size_t at, uint32_t v) {
    b[at] = (uint8_t)v;
    b[at + 1] = (uint8_t)(v >> 8);
  };
  auto put32 = [&](size_t at, uint32_t v) {
    b[at] = (uint8_t)v;
    b[at + 1] = (uint8_t)(v >> 8);
    b[at + 2] = (uint8_t)(v >> 16);
    b[at + 3] = (uint8_t)(v >> 24);
  };
  b[0] = 0x7f; b[1] = 'E'; b[2] = 'L'; b[3] = 'F';
  b[4] = 1;      /* ELFCLASS32 */
  b[5] = 1;      /* ELFDATA2LSB */
  b[6] = 1;      /* EV_CURRENT */
  put16(16, 2);      /* ET_EXEC */
  put16(18, 40);     /* EM_ARM */
  put32(20, 1);      /* e_version */
  put32(24, entry);  /* e_entry */
  put32(28, ehsize); /* e_phoff */
  put32(36, 0x5000200); /* e_flags: hard-float ABI tag, ignored here */
  put16(40, ehsize);
  put16(42, phentsize);
  put16(44, phnum);
  /* program header */
  const size_t ph = ehsize;
  put32(ph + 0, 1);          /* PT_LOAD */
  put32(ph + 4, total - code_len);  /* p_offset */
  put32(ph + 8, load_addr);  /* p_vaddr */
  put32(ph + 12, load_addr); /* p_paddr */
  put32(ph + 16, (uint32_t)code_len);   /* p_filesz */
  put32(ph + 20, (uint32_t)code_len + memsz_extra); /* p_memsz */
  put32(ph + 24, 7);         /* RWX */
  put32(ph + 28, 4096);
  std::memcpy(&b[total - code_len], code, code_len);
  return b;
}

/* ---- device ------------------------------------------------------------ */

struct Dev {
  uint32_t last_write;
  int writes, reads;
  brain_emu_t* stop;
};
Dev g_dev;

uint32_t dev_read(void*, uint32_t addr, uint32_t size) {
  g_dev.reads++;
  return 0xA5000000u | ((addr >> 2) & 3u) | (uint32_t)size;
}
void dev_write(void*, uint32_t addr, uint32_t value, uint32_t size) {
  (void)size;
  g_dev.writes++;
  g_dev.last_write = value ^ addr;
  /* The same trick the command line tool uses: a device write ends the run. */
  if (g_dev.stop) brain_stop(g_dev.stop);
}

void test_basic_run() {
  std::printf("[basic run]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;

  CHECK(brain_load_raw(e, kProgAdd, sizeof kProgAdd, 0x8000, 0) == 0);
  CHECK(brain_get_reg(e, 15) == 0x8000);
  CHECK((brain_get_reg(e, BRAIN_REG_CPSR) & 0x20) == 0);
  CHECK(brain_run_insns(e, 2) == 2);
  CHECK(brain_get_reg(e, 0) == 6);
  CHECK(brain_pending_exception(e) == 0);

  brain_stats_t s;
  brain_get_stats(e, &s);
  CHECK(s.insns == 2);
  CHECK(s.insns_interp == 2);
  CHECK(s.cycles >= 2);
  for (int i = 0; i < BRAIN_EXC_COUNT; ++i) CHECK(s.exc[i] == 0);

  /* A register write and a re-read have to agree even for R15, whose value in
   * the register file is the pipeline's +8, not the address you asked for. */
  CHECK(brain_set_reg(e, 15, 0x8008) == 0);
  CHECK(brain_get_reg(e, 15) == 0x8008);
  CHECK(brain_set_reg(e, 5, 0xdeadbeefu) == 0);
  CHECK(brain_get_reg(e, 5) == 0xdeadbeefu);

  /* Memory peek/poke is physical and must not take exceptions. */
  brain_poke32(e, 0x9000, 0x11223344u);
  CHECK(brain_peek32(e, 0x9000) == 0x11223344u);
  uint8_t bytes[4] = {0};
  CHECK(brain_read_mem(e, 0x9000, bytes, 4) == 0);
  CHECK(bytes[0] == 0x44 && bytes[3] == 0x11);
  const uint8_t patch[4] = {1, 2, 3, 4};
  CHECK(brain_write_mem(e, 0x9000, patch, 4) == 0);
  CHECK(brain_peek32(e, 0x9000) == 0x04030201u);

  /* Out of range registers are refused, not wrapped. */
  CHECK(brain_set_reg(e, 18, 0) != 0);
  CHECK(brain_get_reg(e, -1) == 0);

  brain_emu_destroy(e);
}

void test_thumb_entry() {
  std::printf("[thumb entry]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;
  CHECK(brain_load_raw(e, kThumbProg, sizeof kThumbProg, 0x8000, 1) == 0);
  CHECK((brain_get_reg(e, BRAIN_REG_CPSR) & 0x20) != 0);
  CHECK(brain_run_insns(e, 2) == 2);
  CHECK(brain_get_reg(e, 0) == 5);
  /* The T bit survives execution: a Thumb program that has not branched to an
   * even address stays in Thumb state. */
  CHECK((brain_get_reg(e, BRAIN_REG_CPSR) & 0x20) != 0);
  brain_emu_destroy(e);
}

void test_reset_clears() {
  std::printf("[reset]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;
  brain_set_reg(e, 0, 0x12345678u);
  brain_run_insns(e, 10);
  brain_reset(e);
  CHECK(brain_get_reg(e, 0) == 0);
  brain_stats_t s;
  brain_get_stats(e, &s);
  /* reset() puts the counters back too -- the numbers a caller sees are always
   * "since the last reset", which is what a test harness wants. */
  CHECK(s.insns == 0);
  CHECK(s.cycles == 0);
  brain_emu_destroy(e);
}

void test_elf_loader() {
  std::printf("[elf loader]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;

  const std::vector<uint8_t> img =
      make_elf(0x8000, (const uint8_t*)kProgAdd, sizeof kProgAdd, 0x8000, 16);
  uint32_t entry = 0;
  CHECK(brain_load_elf32(e, img.data(), img.size(), &entry) == 0);
  CHECK(entry == 0x8000);
  CHECK(brain_peek32(e, 0x8000) == kProgAdd[0]);
  CHECK(brain_peek32(e, 0x8004) == kProgAdd[1]);
  /* p_memsz - p_filesz of zero fill. */
  CHECK(brain_peek32(e, 0x8000 + sizeof kProgAdd) == 0);
  CHECK(brain_get_reg(e, 15) == 0x8000);
  /* A stack was handed to us at the top of RAM, out of the image's way. */
  const uint32_t sp = brain_get_reg(e, 13);
  CHECK(sp != 0 && sp >= 0x8000 + sizeof kProgAdd && sp <= cfg.ram_size);
  CHECK((sp & 7u) == 0);
  CHECK(brain_run_insns(e, 2) == 2);
  CHECK(brain_get_reg(e, 0) == 6);

  /* A Thumb entry is marked by bit 0 and has to carry through to CPSR.T. */
  const std::vector<uint8_t> timg =
      make_elf(0x8000, (const uint8_t*)kThumbProg, sizeof kThumbProg, 0x8001);
  CHECK(brain_load_elf32(e, timg.data(), timg.size(), &entry) == 0);
  CHECK(entry == 0x8001);
  CHECK((brain_get_reg(e, BRAIN_REG_CPSR) & 0x20) != 0);

  /* Rejections: wrong class, wrong machine, PIC, truncated, no segments. */
  std::vector<uint8_t> bad = img;
  bad[4] = 2;  /* ELFCLASS64 */
  CHECK(brain_load_elf32(e, bad.data(), bad.size(), nullptr) != 0);
  bad = img;
  bad[5] = 2;  /* ELFDATA2MSB */
  CHECK(brain_load_elf32(e, bad.data(), bad.size(), nullptr) != 0);
  bad = img;
  bad[18] = 3;  /* e_machine = 0x0303, not EM_ARM */
  CHECK(brain_load_elf32(e, bad.data(), bad.size(), nullptr) != 0);
  bad = img;
  bad[16] = 3;  /* ET_DYN */
  CHECK(brain_load_elf32(e, bad.data(), bad.size(), nullptr) != 0);
  CHECK(std::strlen(brain_last_error(e)) > 0);
  CHECK(brain_load_elf32(e, img.data(), 40, nullptr) != 0);
  const std::vector<uint8_t> junk(64, 0);
  CHECK(brain_load_elf32(e, junk.data(), junk.size(), nullptr) != 0);

  /* A segment outside the mapped window is an error, not a silent drop. */
  const std::vector<uint8_t> far =
      make_elf(0xF0000000, (const uint8_t*)kProgAdd, sizeof kProgAdd,
               0xF0000000);
  CHECK(brain_load_elf32(e, far.data(), far.size(), nullptr) != 0);

  /* Raw loading keeps working after a rejected image, and the error text is
   * cleared by the next successful call. */
  CHECK(brain_load_raw(e, kProgAdd, sizeof kProgAdd, 0x8000, 0) == 0);
  brain_emu_destroy(e);

  /* An image that does not fit the RAM window is refused *before* anything is
   * written, so a rejected load cannot leave a half-loaded machine behind. */
  brain_config_t small;
  std::memset(&small, 0, sizeof small);
  small.ram_size = 0x1000;   /* one page, at 0 */
  brain_emu_t* tiny = brain_emu_create(&small);
  CHECK(tiny != nullptr);
  if (tiny) {
    CHECK(brain_load_raw(tiny, kProgAdd, sizeof kProgAdd, 0x8000, 0) != 0);
    CHECK(std::strlen(brain_last_error(tiny)) > 0);
    /* The same image, placed inside the window, is fine. */
    CHECK(brain_load_raw(tiny, kProgAdd, sizeof kProgAdd, 0, 0) == 0);
    CHECK(brain_peek32(tiny, 0) == kProgAdd[0]);
    brain_emu_destroy(tiny);
  }
}

void test_device_and_stop() {
  std::printf("[device, stop]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;
  CHECK(brain_map_device(e, 0x100000, 4096, dev_read, dev_write, nullptr,
                         "test") == 0);
  g_dev = Dev{};
  g_dev.stop = e;
  /* One bus-level 32-bit read must be exactly one hook call: a device with a
   * read side effect (a FIFO, a status register) cannot tolerate being polled
   * four times per access. */
  CHECK(brain_read32(e, 0x100000) == 0xA5000004u);
  CHECK(g_dev.reads == 1);
  CHECK(g_dev.writes == 0);
  /* And a RAM-only peek must not touch the device at all. */
  g_dev = Dev{};
  g_dev.stop = e;
  (void)brain_peek32(e, 0x100000);
  CHECK(g_dev.reads == 0);
  g_dev = Dev{};
  g_dev.stop = e;
  brain_write32(e, 0x100008, 0x11111111u);
  CHECK(g_dev.writes == 1);
  CHECK(g_dev.last_write == (0x11111111u ^ 0x100008u));

  /* Device accesses have to work the way the *guest* reaches them: LDR retires
   * the value the read hook returned, and STR both writes the hook and asks the
   * run loop to stop. */
  const uint32_t prog[] = {0xE5912000u,  /* ldr r2, [r1] */
                           0xE5810000u,  /* str r0, [r1] */
                           0xEAFFFFFEu};
  CHECK(brain_load_raw(e, prog, sizeof prog, 0x8000, 0) == 0);
  brain_set_reg(e, 1, 0x100000);
  brain_set_reg(e, 0, 0x42);
  g_dev = Dev{};
  g_dev.stop = e;
  const uint64_t n = brain_run_insns(e, 1000);
  CHECK(n == 2);                       /* the device write ended the run      */
  CHECK(brain_get_reg(e, 2) == 0xA5000004u);
  CHECK(g_dev.writes == 1);            /* one hook call for the guest's STR   */
  CHECK(g_dev.reads == 1);             /* one for its LDR                     */

  /* MMIO above the 32MB mark has to be reachable with the MMU off -- that is
   * where the i.MX28 puts its peripherals, and boot code pokes them long before
   * any page table exists.  A core that clears MVA[31:24] when the MMU is off
   * silently turns these accesses into RAM accesses at 0x00######. */
  CHECK(brain_map_device(e, 0x80000000, 4096, dev_read, dev_write, nullptr,
                         "hi") == 0);
  const uint32_t hi[] = {0xE3A01008u,  /* mov r1, #8          */
                         0xE1A01E01u,  /* mov r1, r1, lsl #28 -> 0x80000000  */
                         0xE3A0002Au,  /* mov r0, #0x2a       */
                         0xE5810000u}; /* str r0, [r1]        */
  CHECK(brain_load_raw(e, hi, sizeof hi, 0x8000, 0) == 0);
  g_dev = Dev{};
  g_dev.stop = e;
  CHECK(brain_run_insns(e, 8) == 4);
  CHECK(g_dev.last_write == (0x2Au ^ 0x80000000u));

  /* Overlapping the RAM window is refused. */
  CHECK(brain_map_device(e, 0x0, 4096, dev_read, dev_write, nullptr, "clash") !=
        0);
  brain_emu_destroy(e);
}

void test_be32_and_flags() {
  std::printf("[flags]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;
  /* ADDS R0, R0, R1 with R0 = 0x7fffffff, R1 = 1 -> N and V set. */
  CHECK(brain_set_reg(e, 0, 0x7FFFFFFFu) == 0);
  CHECK(brain_set_reg(e, 1, 1) == 0);
  const uint32_t adds = 0xE0900001u;
  CHECK(brain_load_raw(e, &adds, 4, 0x8000, 0) == 0);
  brain_run_insns(e, 1);
  const uint32_t cpsr = brain_get_reg(e, BRAIN_REG_CPSR);
  CHECK(brain_get_reg(e, 0) == 0x80000000u);
  CHECK((cpsr & (1u << 31)) != 0);  /* N */
  CHECK((cpsr & (1u << 28)) != 0);  /* V */
  CHECK((cpsr & (1u << 30)) == 0);  /* Z clear */
  CHECK((cpsr & (1u << 29)) == 0);  /* C clear */
  brain_emu_destroy(e);
}

}  // namespace


/* Exception entry, the register bank, and the interrupt masks.
 *
 * R0-R7 (and R8-R12 outside FIQ mode) name the same physical registers in
 * every mode, so taking an exception must leave them alone, while R13/R14 are
 * banked per mode.  The handler therefore hops through System mode - which has
 * its own R13/R14 but shares everything else - and back.  It also records the
 * CPSR it saw while in System mode, which proves the mode change really
 * happened, and the test pins the documented entry rules: taking an exception
 * sets CPSR.I, leaves CPSR.F alone except for a FIQ or reset, and returns with
 * the flags restored from the SPSR (DDI 0100I A2.6, "Summary of exception
 * entry").  All encodings verified with keystone. */
void test_exception_banking() {
  std::printf("[exception banking]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;

  const uint32_t prog[] = {
      0xE3A000A0u,  /* mov r0, #0xa0 */   0xE3A010A1u,  /* mov r1, #0xa1 */
      0xE3A020A2u,  /* mov r2, #0xa2 */   0xE3A030A3u,  /* mov r3, #0xa3 */
      0xE3A040A4u,  /* mov r4, #0xa4 */   0xE3A050A5u,  /* mov r5, #0xa5 */
      0xE3A060A6u,  /* mov r6, #0xa6 */   0xE3A070A7u,  /* mov r7, #0xa7 */
      0xE3A0801Fu,  /* mov r8, #0x1f  - the System-mode CPSR value            */
      0xE3A09013u,  /* mov r9, #0x13  - the Supervisor-mode CPSR value        */
      0xEF000000u,  /* swi 0                                                   */
      0xE2800001u,  /* add r0, r0, #1  - where the handler must return         */
      0xEAFFFFFEu,  /* b .                                                     */
  };
  const uint32_t handler[] = {
      0xE121F008u,  /* MSR CPSR_c, r8   -> System mode (own R13/R14)          */
      0xE3A0005Au,  /* MOV R0, #0x5a    - shared, so it must survive the return */
      0xE3A0DC12u,  /* MOV R13, #0x1200 - the *System* stack pointer           */
      0xE10F3000u,  /* MRS R3, CPSR     - records that the System detour ran      */
      0xE121F009u,  /* MSR CPSR_c, r9   -> back to Supervisor                  */
      0xE1B0F00Eu,  /* MOVS PC, R14     - exception return: CPSR <- SPSR_svc   */
  };
  CHECK(brain_load_raw(e, prog, sizeof prog, 0x8000u, 0) == 0);
  for (size_t i = 0; i < sizeof handler / sizeof handler[0]; ++i)
    brain_poke32(e, 0x40u + (uint32_t)(i * 4u), handler[i]);
  brain_poke32(e, 0x08u, 0xEA00000Cu);          /* the SWI vector: b 0x40     */
  CHECK(brain_set_reg(e, 13, 0x1FF000u) == 0);  /* the Supervisor stack        */

  CHECK(brain_run_insns(e, 24) == 24);
  /* R1-R7 are untouched by the exception: no bank may copy over them. */
  for (int r = 1; r <= 7; ++r) {
    if (r == 3) continue;   /* the handler uses R3 to record the detour      */
    CHECK(brain_get_reg(e, r) == (uint32_t)(0xA0 + r));
  }
  CHECK(brain_get_reg(e, 0) == 0x5Bu);         /* the handler's 0x5a, then +1  */
  /* MRS in System mode: the control byte the handler wrote, I and F included. */
  CHECK(brain_get_reg(e, 3) == 0x1Fu);
  CHECK(brain_get_reg(e, 8) == 0x1Fu);
  CHECK(brain_get_reg(e, 9) == 0x13u);
  CHECK(brain_get_reg(e, 13) == 0x1FF000u);    /* the Supervisor stack survived */
  CHECK(brain_get_reg(e, 14) == 0x802Cu);      /* R14_svc = address of the SWI+4 */
  CHECK(brain_get_reg(e, BRAIN_REG_CPSR) == 0xD3u);   /* SVC, ARM, I|F as before */
  brain_emu_destroy(e);
}

/* A faulting fetch must still retire an instruction and burn a cycle.  Without
 * that, a guest whose abort vector itself faults - the classic "MMU on before
 * the table maps the vectors" bootloader bug - never makes progress and every
 * bounded run of it hangs. */
void test_fetch_abort_terminates() {
  std::printf("[fetch abort liveness]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;

  /* Turn the MMU on with a table of all-zeroes, so every fetch - including the
   * one of the abort vector - takes a prefetch abort for ever. */
  const uint32_t prog[] = {0xE3A01000u, 0xEE021F10u, 0xEE010F10u};
  CHECK(brain_load_raw(e, prog, sizeof prog, 0x8000u, 0) == 0);
  CHECK(brain_set_reg(e, 0, 1u) == 0);         /* c1 = M                        */
  CHECK(brain_set_reg(e, 1, 0x10000u) == 0);   /* c2 = the empty table          */
  CHECK(brain_set_reg(e, 2, 0u) == 0);         /* c3 = every domain: no access  */
  const uint64_t n = brain_run_insns(e, 500);
  CHECK(n == 500u);
  brain_stats_t st;
  brain_get_stats(e, &st);
  CHECK(st.cycles >= 500u);
  CHECK(st.exc[BRAIN_EXC_PABORT] > 1u);        /* it kept trying to vector      */
  CHECK((brain_get_reg(e, BRAIN_REG_CPSR) & 0x1Fu) == 0x17u);   /* Abort mode  */
  brain_emu_destroy(e);
}

/* Loads that change the PC, and the state they leave behind.
 *
 * DDI 0198E Table 2-11 gives CP15 c1[15] (L4) a specific meaning: with L4 = 0
 * (the reset value) "loads to PC set the T bit", with L4 = 1 they do not
 * (ARMv4 behaviour).  So `LDR PC,[Rn]` to an odd address really does enter Thumb
 * state on this core - and only *loads* get that treatment: a `MOV PC, Rn` with
 * an odd value ignores bits[1:0] on ARMv5, while ARMv6+ made every PC write
 * behave like BX, which is why such encodings stay out of the golden corpus.
 * The last phase checks the BLX-immediate link: bit[0] of R14 has to stay clear,
 * because the link is what the callee returns with, and the caller is in ARM
 * state.  Encodings verified with keystone. */
void test_load_pc_and_l4() {
  std::printf("[load to PC, L4]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;

  const uint32_t ldr_pc[] = {0xE3A00000u, 0xEE014F10u, 0xE591F000u,
                             0xE2800C01u, 0xEAFFFFFEu};
  const uint32_t mov_pc[] = {0xE3A00000u, 0xE1A0F002u, 0xE2800002u, 0xEAFFFFFEu};
  /* BLX #imm is the ARMv5 encoding cond 1111 101 H imm23 1, whose offset is
   * SignExtend(imm24 : H : 0) over insn[23:0].  Both words below carry
   * imm24 = 0x11 from 0x8004 (PC = 0x800c), so the offset is 0x44 + 2*H:
   *   H = 1 -> 0x46 -> 0x8052, the `MOV R2, R14` thunk   (link reaches R2)
   *   H = 0 -> 0x44 -> 0x8050, the `b .` in front of it  (R2 keeps its seed)
   * A core that scales the immediate by 4 from insn[23:1] instead reaches
   * 0x802E, so the pair pins both the scaling and H.  Hand-assembled because
   * an ARM-mode line list cannot express a Thumb callee for the corpus. */
  const uint32_t blx_imm[] = {0xE3A00000u, 0xFB000011u, 0xE2800C01u,
                              0xEAFFFFFEu};
  const uint32_t blx_h0[] = {0xE3A00000u, 0xFA000011u, 0xE2800C01u,
                             0xEAFFFFFEu};
  const uint32_t kL4 = 1u << 15;

  for (int phase = 0; phase < 5; ++phase) {
    /* A Thumb callee at 0x8040 (ADDS R0, #1 then b .), an ARM marker at 0x8044
     * (ADD R0, R0, #2), and a Thumb thunk at 0x8050 (MOV R2, R14 then b .). */
    brain_poke32(e, 0x8040u, 0xE7FE3001u);
    brain_poke32(e, 0x8044u, 0xE2800002u);
    brain_poke32(e, 0x8048u, 0xEAFFFFFEu);
    brain_poke32(e, 0x8050u, 0x4672E7FEu);   /* 0x8052: MOV R2, R14          */
    const uint32_t* prog = phase == 2 ? mov_pc
                       : phase == 3 ? blx_imm
                       : phase == 4 ? blx_h0 : ldr_pc;
    const size_t len = phase == 2 ? sizeof mov_pc
                       : phase == 3 ? sizeof blx_imm
                       : phase == 4 ? sizeof blx_h0 : sizeof ldr_pc;
    CHECK(brain_load_raw(e, prog, len, 0x8000u, 0) == 0);
    CHECK(brain_set_reg(e, 1, 0x9000u) == 0);
    CHECK(brain_set_reg(e, 4, phase == 1 ? kL4 : 0u) == 0);
    CHECK(brain_set_reg(e, 2, 0x8009u) == 0);
    if (phase == 0) brain_poke32(e, 0x9000u, 0x8041u);   /* odd: enter Thumb    */
    if (phase == 1) brain_poke32(e, 0x9000u, 0x8046u);   /* L4 = 1: stay ARM     */
    CHECK(brain_run_insns(e, 12) == 12);
    const uint32_t cpsr = brain_get_reg(e, BRAIN_REG_CPSR);
    switch (phase) {
      case 0:                                    /* L4 = 0, LDR PC -> Thumb      */
        CHECK(brain_get_reg(e, 0) == 1u);        /* the ADDS at 0x8040 ran       */
        CHECK((cpsr & 0x20u) != 0u);             /* bit[0] selected Thumb        */
        CHECK(brain_get_reg(e, 1) == 0x9000u);
        break;
      case 1:                                    /* L4 = 1, ARMv4: no T update  */
        CHECK(brain_get_reg(e, 0) == 2u);        /* the ARM ADD at 0x8044 ran     */
        CHECK((cpsr & 0x20u) == 0u);
        break;
      case 2:                                    /* MOV PC: not a load          */
        CHECK(brain_get_reg(e, 0) == 2u);        /* bit[0] of 0x8009 ignored      */
        CHECK((cpsr & 0x20u) == 0u);
        break;
      case 3: {                                  /* BLX #imm, H = 1             */
        const uint32_t r2 = brain_get_reg(e, 2);
        CHECK(brain_get_reg(e, 0) == 0u);        /* the fall-through was skipped   */
        CHECK((cpsr & 0x20u) != 0u);             /* the immediate entered Thumb    */
        CHECK(r2 == 0x8008u);                    /* MOV R2, R14 at 0x8052          */
        break;
      }
      case 4: {                                  /* same, H = 0: two bytes back   */
        const uint32_t r2 = brain_get_reg(e, 2);
        CHECK(brain_get_reg(e, 0) == 0u);        /* still no fall-through          */
        CHECK((cpsr & 0x20u) != 0u);             /* landed on the `b .` in Thumb   */
        CHECK(r2 == 0x8009u);                    /* the thunk was never reached    */
        break;
      }
      default:
        break;
    }
    CHECK((cpsr & 0x1Fu) == 0x13u);              /* mode never changes here      */
  }
  brain_emu_destroy(e);
}

/* A Thumb epilogue that returns through the PC.
 *
 * This is the part of a PUSH/POP prologue the generated corpus cannot express.
 * Stack cases need a stack pointer, and a banked register cannot be *handed* to
 * either model: the oracle reloads the bank its mode caches when the PC moves, so
 * every corpus stack sequence has to give itself an SP with `MOV SP, R4` and ends
 * with the link register reading 0.  That pins the register round trip but not the
 * state the return selects, which is the interesting half.  Here LR is seeded
 * directly, so both directions are checked: with L4 clear, as at reset, bit[0] of
 * the value popped into the PC sets T, and the address used has that bit masked off
 * (DDI 0198E Table 2-11, and the same rule `test_load_pc_and_l4` pins for LDR).
 * Encodings from keystone. */
void test_push_pop_return() {
  std::printf("[push/pop epilogue return]\n");
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  CHECK(e != nullptr);
  if (!e) return;

  /* 0x8000 push {r4, r5, lr}   0x8002 movs r4, r0   0x8004 adds r5, r0, #2
     0x8006 pop  {r4, r5, pc}   0x8008 b .                <- the return slot */
  const uint16_t prog[] = {0xB530u, 0x0004u, 0x1C85u, 0xBD30u, 0xE7FEu};
  for (int phase = 0; phase < 2; ++phase) {
    const uint32_t lr = 0x8008u | (phase ? 0u : 1u);   /* odd: stay Thumb      */
    CHECK(brain_load_raw(e, prog, sizeof prog, 0x8000u, 1) == 0);
    CHECK(brain_set_reg(e, 0, 0x11u) == 0);
    CHECK(brain_set_reg(e, 4, 0x4444u) == 0);
    CHECK(brain_set_reg(e, 5, 0x5555u) == 0);
    CHECK(brain_set_reg(e, 13, 0x9000u) == 0);
    CHECK(brain_set_reg(e, 14, lr) == 0);
    CHECK(brain_run_insns(e, 4) == 4);
    CHECK(brain_get_reg(e, 0) == 0x11u);        /* the body did run            */
    CHECK(brain_get_reg(e, 4) == 0x4444u);      /* ... and was undone by POP   */
    CHECK(brain_get_reg(e, 5) == 0x5555u);
    CHECK(brain_get_reg(e, 13) == 0x9000u);
    /* The model reports the address of the instruction that would run next, with
     * bit[0] set in Thumb state, so the two phases read differently and neither
     * says 0x8008|1 in ARM: `b .` at 0x8008 is the same slot either way. */
    CHECK(brain_get_reg(e, 15) == (phase ? 0x8008u : 0x8009u));
    const uint32_t t = brain_get_reg(e, BRAIN_REG_CPSR) & 0x20u;
    CHECK(t == (phase ? 0u : 0x20u));           /* bit[0] picked the state     */
    CHECK((brain_get_reg(e, BRAIN_REG_CPSR) & 0x1Fu) == 0x13u);
  }
  brain_emu_destroy(e);
}

/* Exception entry: which vector, which mode, what the link and SPSR hold.
 *
 * Nothing external can be an oracle for this - Unicorn takes exceptions on QEMU's
 * own terms and cannot be asked to fault a chosen fetch - so this pins the rule
 * recorded in docs/ARCHITECTURE.md, from DDI 0198E 4.1.1: on entry the core saves
 * the whole CPSR (including the mirrored T bit, which is how a return gets back to
 * Thumb) into SPSR_<mode>, puts `address of the instruction that raised the
 * exception + a per-kind adjustment` into R14_<mode>, changes to the mode, forces
 * ARM state, sets I, and leaves F alone because only FIQ touches it.  The
 * adjustment is 4 for ARM and 2 for Thumb on an undefined instruction and on SWI,
 * so that `MOVS PC, R14` resumes after the instruction, and 8 on a data abort on
 * both states, so that `SUBS PC, R14, #8` re-executes the faulting access.
 *
 * The vector is proved by running something: each vector slot gets a marker and
 * the row's gets `MOV R12, #<exception number>`, so a core that enters the right
 * state at the wrong address, or the right address with the wrong link, is caught.
 * A prefetch abort is deliberately not a row here: a faulting fetch retires
 * without executing (see `test_fetch_abort_terminates`), which makes the marker
 * dance ambiguous, and the mapping behaviour it depends on belongs to
 * tests/test_mmu.cpp.  IRQ and FIQ cannot be raised through the public API at all.
 * Encodings verified with keystone: CDP is undefined on this core (no
 * coprocessor), `IT` is an ARMv6T2 encoding an ARM926EJ-S does not have, and an
 * LDR from outside the RAM window is an external data abort.
 */
static void test_interrupt_inputs() {
  /* Level inputs on nIRQ/nFIQ, exercised through the exact retirement order
   * `Cpu::step_one` samples them at.  ARM rows: the program is `mov r0,#0x13`,
   * `msr cpsr_c,r0` (0xE121F000 - Capstone-verified field encoding), then a spin
   * `b .` in slot [2] and two more spins - 0xEAFFFFFE x3.  The load defers the
   * first instruction into the step where the line is sampled, so with the line
   * high from the start, slot [0] retires *masked* (phase 1) and the `msr` in
   * slot [1] commits as it retires; the line is then taken at the boundary in
   * front of slot [2] at 0x8008, which is the deferred instruction - R14 must
   * equal 0x800C (`SUBS PC,R14,#4` returns to the deferred spin exactly).
   *
   * Thumb rows reach Thumb through a *taken* BX - the case the deferral interacts
   * with: [0] `mov r0,#0x13`, [1] `add r1,pc,#1`, [2] `bx r1`, [3]/[4] halfword
   * pairs (0x2007 `movs r0,#7` + 0xE7FE `b .`).  With the line low the `add` and
   * the BX retire (CPSR 0xb3: T set by the taken branch, I still masked by the
   * seed); the test then pokes the masks open through the debugger register API -
   * which is the `msr` equivalent at a *known* boundary - and the entry happens in
   * front of the halfword at 0x8010: R14 = 0x8012, the ARMv5 "R14 = old-style PC
   * + 4" for a Thumb IRQ, and SPSR carries T so a `BX`/`MOVS` return restores
   * Thumb.  FIQ rows assert the 0x1C vector, the F mask, and R14_FIQ - all in
   * both states.  Nothing lowers the line automatically: dropping it is what lets
   * the handler's own spin run instead of re-entering, which the last rows pin. */
  static const uint32_t arm_prog[5] = {0xE3A00013u, 0xE121F000u, 0xEAFFFFFEu,
                                      0xEAFFFFFEu, 0xEAFFFFFEu};
  static const uint32_t th_prog[5] = {0xE3A00013u, 0xE28F1001u, 0xE12FFF11u,
                                     0xE7FE2007u, 0xE7FE2007u};
  for (int fiq = 0; fiq < 2; ++fiq) {
    for (int thumb = 0; thumb < 2; ++thumb) {
      const uint32_t vec = fiq ? 0x1Cu : 0x18u;
      const int mode = fiq ? 0x11 : 0x12;
      const uint32_t marker = 0x22u;
      char nm[64];
      std::snprintf(nm, sizeof nm, "%s entry from %s state", fiq ? "FIQ" : "IRQ",
                    thumb ? "thumb" : "arm");
      brain_emu_t* e = brain_emu_create(nullptr);
      CHECK(e != nullptr);
      if (!e) continue;
      /* All nine vector slots: FIQ lives at 0x1C, and poking only through 0x18
       * would leave the FIQ slot holding whatever the IRQ entry installed. */
      for (int i = 0; i < 9; ++i) brain_poke32(e, (uint32_t)(i * 4u), 0xEAFFFFFEu);
      /* `mov r4,r14` then `mov r12,#marker`: R14 is sampled *inside* the handler
       * because it is banked; both instructions are flag-free, so no state the
       * assertions compare against is perturbed by the handler itself. */
      brain_poke32(e, vec, 0xE1A0400Eu);
      brain_poke32(e, vec + 4u, 0xE3A0C000u + marker);

      /* SVC with I (and F for FIQ) masked; the load only forces T, and T=0 here -
       * both programs start in ARM and the Thumb rows *arrive* in Thumb. */
      CHECK(brain_set_reg(e, BRAIN_REG_CPSR, 0x00000093u | (fiq ? 0x40u : 0u)) == 0);
      CHECK(brain_load_raw(e, thumb ? (const void*)th_prog : (const void*)arm_prog,
                           20, 0x8000u, 0) == 0);

      /* Phase 1: the line is high while the masks hold it - the deferred first
       * instruction retires and the core parks, no entry taken. */
      if (fiq) brain_set_fiq(e, 1); else brain_set_irq(e, 1);
      if (thumb) {
        /* [0] `mov`, [1] `add r1,pc,#1`, [2] `bx r1`, [3] the deferred halfword
         * all retire while masked - the line cannot be taken while I (and F for
         * FIQ) hide it; the BX's own T-setting is what the poke below mimics. */
        CHECK(brain_run_insns(e, 4) == 4u);
        CHECK(brain_get_reg(e, 12) == 0u);    /* masked: no entry yet                  */
        CHECK(brain_get_reg(e, BRAIN_REG_CPSR)
              == (uint32_t)(0x93u | 0x20u | (fiq ? 0x40u : 0u)));  /* T from the BX   */
        CHECK(brain_set_reg(e, BRAIN_REG_CPSR, 0x00000033u) == 0);
      } else {
        CHECK(brain_run_insns(e, 2) == 2u);   /* [0] masked, [1] `msr` commits unmask  */
      }
      check(brain_get_reg(e, 12) == 0u, "no entry while the interrupt is masked");

      /* The entry at the next step boundary retires nothing itself; the two handler
       * instructions follow.  Both states fit the same budget here. */
      CHECK(brain_run_insns(e, 2) == 2u);              /* entry + `mov r4,r14`         */
      if (brain_get_reg(e, 12) != marker) CHECK(brain_run_insns(e, 2) <= 2u);  /* `mov r12,#0x22` */
      const uint32_t in = brain_get_reg(e, BRAIN_REG_CPSR);
      const uint32_t spsr = brain_get_reg(e, BRAIN_REG_SPSR);
      check(brain_get_reg(e, 12) == marker, "the handler ran");
      check((in & 0x1Fu) == (uint32_t)mode, nm);
      CHECK(in & 0x80u);                      /* I is always set on entry              */
      check((in & 0x40u) == (uint32_t)(fiq ? 0x40u : 0u), "F is set by FIQ entry only");
      CHECK((in & 0x20u) == 0u);               /* entry forces ARM state               */
      if (thumb) {
        check(spsr == 0x00000033u, "SPSR keeps T so the handler can return to Thumb");
        check(brain_get_reg(e, 4) == 0x8012u,
              "R14 = old-style PC + 4 for a Thumb IRQ (deferred halfword + 2)");
      } else {
        check(spsr == 0x00000013u, "SPSR_<mode> gets the whole pre-entry CPSR");
        check(brain_get_reg(e, 4) == 0x800Cu,
              "R14 = the deferred instruction + 4 in ARM state");
      }
      brain_stats_t st;
      brain_get_stats(e, &st);
      CHECK(st.exc[fiq ? BRAIN_EXC_FIQ : BRAIN_EXC_IRQ] >= 1u);

      /* Level semantics: with the line dropped, the handler's spin runs instead of
       * re-entering; raising it again re-enters on the next boundary.  */
      if (fiq) brain_set_fiq(e, 0); else brain_set_irq(e, 0);
      CHECK(brain_run_insns(e, 1) == 1u);
      check((brain_get_reg(e, BRAIN_REG_CPSR) & 0x1Fu) == (uint32_t)mode,
            "no re-entry while the line is low");
      brain_emu_destroy(e);
    }
  }
}

void test_exception_entry() {
  std::printf("[exception entry]\n");
  struct Row { int exc; uint32_t off; int mode; uint32_t adj_arm, adj_thumb;
               uint32_t trig_arm; uint16_t trig_thumb; };
  const Row rows[] = {
      {BRAIN_EXC_UNDEF, 0x04u, 0x1B, 4, 2, 0xED000A10u, 0xBFE8u},
      /* LDC/STC to any coprocessor - p15 included, whose interface is MRC/MCR
       * only (ARM926EJ-S TRM 2.2; the SAM9G15 datasheet 8.4.8 spells out "other
       * instructions like CDP, LDC, STC can cause an undefined instruction
       * exception").  All six words are Keystone-assembled; the Thumb triggers
       * are the T2 high halves (0xED94/0xED84), which this core never pairs -
       * 32-bit Thumb is out of the ISA by design, so they stand undefined here,
       * exactly as the ARM926EJ-S decodes them. */
      {BRAIN_EXC_UNDEF, 0x04u, 0x1B, 4, 2, 0xED900F00u, 0xBF00u},
      {BRAIN_EXC_UNDEF, 0x04u, 0x1B, 4, 2, 0xED800F00u, 0xBF00u},
      {BRAIN_EXC_UNDEF, 0x04u, 0x1B, 4, 2, 0xED921001u, 0xBF00u},
      {BRAIN_EXC_SWI,   0x08u, 0x13, 4, 2, 0xEF000000u, 0xDF00u},
      {BRAIN_EXC_DABORT, 0x10u, 0x17, 8, 8, 0xE5912000u, 0x680Au},
  };

  for (size_t r = 0; r < sizeof rows / sizeof rows[0]; ++r) {
    const Row& w = rows[r];
    for (int thumb = 0; thumb < 2; ++thumb) {
      for (int high = 0; high < 2; ++high) {
        /* c1[13] (V) picks 0xFFFF0000 at reset, so the vectors move - and with
         * them the RAM window, which has to reach them for the marker to exist. */
        brain_config_t cfg;
        std::memset(&cfg, 0, sizeof cfg);
        cfg.ram_size = high ? (2u << 20) : (1u << 20);
        if (high) cfg.ram_base = 0xFFE00000u;
        cfg.high_vectors = high;
        brain_emu_t* e = brain_emu_create(&cfg);
        CHECK(e != nullptr);
        if (!e) continue;
        const uint32_t code = high ? 0xFFE08000u : 0x8000u;
        const uint32_t vbase = high ? 0xFFFF0000u : 0u;

        for (int i = 0; i < 8; ++i)
          brain_poke32(e, vbase + (uint32_t)(i * 4u), 0xEAFFFFFEu);
        brain_poke32(e, vbase + w.off, 0xE3A0C000u + (uint32_t)w.exc);

        /* One preamble instruction so the trigger sits at a known address.  It
         * deliberately does *not* touch the flags (an ARM `MOV`, and in Thumb the
         * hi-register `MOV R8, R0`, neither of which writes CPSR), because the
         * seeded flags are what SPSR has to carry into the handler. */
        const uint32_t prog_arm[] = {0xE3A00000u, w.trig_arm, 0xEAFFFFFEu};
        const uint16_t prog_th[] = {0x4680u, w.trig_thumb, 0xE7FEu};
        const uint32_t trig = code + (thumb ? 2u : 4u);
        CHECK(brain_set_reg(e, BRAIN_REG_CPSR, 0x40000010u) == 0);   /* Z set, USR */
        CHECK(brain_set_reg(e, 1, 0x40000000u) == 0);  /* outside either window    */
        CHECK(brain_load_raw(e, thumb ? (const void*)prog_th : (const void*)prog_arm,
                             thumb ? sizeof prog_th : sizeof prog_arm, code,
                             thumb) == 0);
        const uint32_t pre = brain_get_reg(e, BRAIN_REG_CPSR);

        /* the preamble, the trigger, then the marker the vector holds. */
        CHECK(brain_run_insns(e, 3) == 3);

        brain_stats_t st;
        brain_get_stats(e, &st);
        const uint32_t cpsr = brain_get_reg(e, BRAIN_REG_CPSR);
        const uint32_t spsr = brain_get_reg(e, BRAIN_REG_SPSR);
        CHECK(st.exc[w.exc] == 1u);
        for (int o = 0; o < BRAIN_EXC_COUNT; ++o)
          if (o != w.exc) CHECK(st.exc[o] == 0u);
        CHECK(brain_get_reg(e, 12) == (uint32_t)w.exc);          /* the vector ran */
        CHECK((cpsr & 0x1Fu) == (uint32_t)w.mode);               /* the mode       */
        CHECK((cpsr & 0x80u) != 0u);                             /* I set          */
        CHECK((cpsr & 0x40u) == 0u);                              /* F only for FIQ */
        CHECK((cpsr & 0x20u) == 0u);                       /* ARM at the vector    */
        CHECK((cpsr & 0xF0000000u) == (pre & 0xF0000000u));      /* N Z C V carried */
        CHECK(spsr == pre);                                       /* the whole thing  */
        CHECK(brain_get_reg(e, 14) == trig + (thumb ? w.adj_thumb : w.adj_arm));
        brain_emu_destroy(e);
      }
    }
  }
}

/* shift_arm(), the barrel-shifter primitive that the interpreter and the JIT front
 * end share (src/core/ops.h).  Pinned here as a table because the two places that
 * would otherwise catch a regression can miss it: the corpus contains no instruction
 * using the re-encoded shift spellings, and tests/test_shifter.py can only compare
 * against a model when unicorn is installed.  The expected values are ARM DDI 0084D
 * s4.5.2 / DDI 0406 A1.8.1 read directly: a right-ward shift by 32 leaves C from bit
 * 31 and by more than 32 gives zero with C cleared, ASR saturates to the sign, a
 * rotate folds as (n - 1) MOD 32 + 1, and a zero amount leaves the carry alone.
 * `0x80000001` is the discriminating seed: bit 0 and bit 31 differ from the middle
 * bits, so a rule that reads the wrong one shows up. */
/* The ARMv5 reading of the Thumb T1 shift group: `000 imm5 Rm Rd` with imm5 == 0 is
 * MOV (register) and leaves the flags alone, while `LSRS/ASRS` with the *ARM* field
 * value 0 mean a shift by 32.  A decoder that re-encodes the Thumb imm5 the same way
 * as the ARM shift field turns `MOV R1, R2` into `LSL #32` and clears a register. */
/* The Thumb T1 shift group `000 imm5(10:6) Rm(5:3) Rd(2:0)`, whose imm5 is a *real*
 * five-bit amount: imm5 = 0 is MOV (register) and leaves the flags alone (from ARMv6
 * the same encoding is `LSLS #0`, which does update them - the core is an ARM926EJ-S,
 * so the ARMv5 reading is the one implemented).  In particular the encoding cannot
 * say LSR #32 / ASR #32 / RRX at all: those belong to the ARM encoding and to the
 * 32-bit Thumb one, and a decoder that re-encodes the Thumb field like the ARM one
 * turns `MOV R1, R2` into a shift by 32 and zeroes the register. */
static void test_thumb_shift_zero() {
  struct Row { uint32_t hw, r1, cpsr; const char* what; };
  /* R2 = 0x80000001, CPSR = N Z C V I F T mode from the seed, so each row below is
   * taken from the ARM926 behaviour the model agrees with on every row but the
   * first: `LSLS Rn, Rm, #0` is MOV (register) on ARMv5 and leaves the flags alone,
   * while QEMU (and so unicorn) implements the ARMv6 reading where it updates N/Z.
   * The row is pinned the ARMv5 way, and tests/test_oracle.py skips that one shape
   * for the same reason. */
  static const Row rows[] = {
    {0x0011u, 0x80000001u, 0x20000030u, "000 00000 = MOV R1, R2 (value)"},
    {0x0051u, 0x00000002u, 0x20000030u, "LSL #1"},
    {0x0091u, 0x00000004u, 0x00000030u, "LSL #2"},
    {0x00d1u, 0x00000008u, 0x00000030u, "LSL #3"},
    {0x0211u, 0x00000100u, 0x00000030u, "LSL #8"},
    {0x0411u, 0x00010000u, 0x00000030u, "LSL #16"},
    {0x0611u, 0x01000000u, 0x00000030u, "LSL #24"},
    {0x07d1u, 0x80000000u, 0x80000030u, "LSL #31 (C from bit1)"},
    {0x4089u, 0x00000000u, 0x60000030u, "LSLS R1, R1, R2 (S=1)"},
    {0x4091u, 0x00000000u, 0x40000030u, "LSLS R1, R2, R1 with R1 = 0"},
  };
  for (unsigned k = 0; k < sizeof(rows) / sizeof(rows[0]); ++k) {
    brain_emu_t* e = brain_emu_create(nullptr);
    if (!e) { check(false, "thumb shift: emu created"); return; }
    brain_load_raw(e, (void*)&rows[k].hw, 2, 0x8000u, 1);
    brain_set_reg(e, BRAIN_REG_CPSR, 0x20000010u | 0x20u);   /* T, USR, Z seed */
    brain_set_reg(e, 0, 0u);
    brain_set_reg(e, 1, 0u);
    brain_set_reg(e, 2, 0x80000001u);
    brain_run_insns(e, 1);
    check(brain_get_reg(e, 1) == rows[k].r1, rows[k].what);
    char lab[128];
    std::snprintf(lab, sizeof lab, "%s [flags]", rows[k].what);
    check((brain_get_reg(e, BRAIN_REG_CPSR) & 0xF00000F0u) == rows[k].cpsr, lab);
    brain_emu_destroy(e);
  }
}

static void test_shifter_primitive() {
  struct Row {
    uint32_t x, type, amt, cin, val, carry;
  };
  static const Row kRows[] = {
      {0x80000001, brain::SH_LSL,   0, 0, 0x80000001, 0},
      {0x80000001, brain::SH_LSL,   0, 1, 0x80000001, 1},
      {0x80000001, brain::SH_LSL,   1, 1, 0x00000002, 1},
      {0x80000001, brain::SH_LSL,   2, 1, 0x00000004, 0},
      {0x80000001, brain::SH_LSL,  15, 1, 0x00008000, 0},
      {0x80000001, brain::SH_LSL,  16, 1, 0x00010000, 0},
      {0x80000001, brain::SH_LSL,  30, 1, 0x40000000, 0},
      {0x80000001, brain::SH_LSL,  31, 1, 0x80000000, 0},
      {0x80000001, brain::SH_LSL,  32, 1, 0x00000000, 1},
      {0x80000001, brain::SH_LSL,  33, 1, 0x00000000, 0},
      {0x80000001, brain::SH_LSL,  64, 1, 0x00000000, 0},
      {0x80000001, brain::SH_LSL, 255, 1, 0x00000000, 0},
      {0x80000001, brain::SH_LSR,   0, 0, 0x80000001, 0},
      {0x80000001, brain::SH_LSR,   0, 1, 0x80000001, 1},
      {0x80000001, brain::SH_LSR,   1, 1, 0x40000000, 1},
      {0x80000001, brain::SH_LSR,   2, 1, 0x20000000, 0},
      {0x80000001, brain::SH_LSR,  15, 1, 0x00010000, 0},
      {0x80000001, brain::SH_LSR,  16, 1, 0x00008000, 0},
      {0x80000001, brain::SH_LSR,  30, 1, 0x00000002, 0},
      {0x80000001, brain::SH_LSR,  31, 1, 0x00000001, 0},
      {0x80000001, brain::SH_LSR,  32, 1, 0x00000000, 1},
      {0x80000001, brain::SH_LSR,  33, 1, 0x00000000, 0},
      {0x80000001, brain::SH_LSR,  64, 1, 0x00000000, 0},
      {0x80000001, brain::SH_LSR, 255, 1, 0x00000000, 0},
      {0x80000001, brain::SH_ASR,   0, 0, 0x80000001, 0},
      {0x80000001, brain::SH_ASR,   0, 1, 0x80000001, 1},
      {0x80000001, brain::SH_ASR,   1, 1, 0xC0000000, 1},
      {0x80000001, brain::SH_ASR,   2, 1, 0xE0000000, 0},
      {0x80000001, brain::SH_ASR,  15, 1, 0xFFFF0000, 0},
      {0x80000001, brain::SH_ASR,  16, 1, 0xFFFF8000, 0},
      {0x80000001, brain::SH_ASR,  30, 1, 0xFFFFFFFE, 0},
      {0x80000001, brain::SH_ASR,  31, 1, 0xFFFFFFFF, 0},
      {0x80000001, brain::SH_ASR,  32, 1, 0xFFFFFFFF, 1},
      {0x80000001, brain::SH_ASR,  33, 1, 0xFFFFFFFF, 1},
      {0x80000001, brain::SH_ASR,  64, 1, 0xFFFFFFFF, 1},
      {0x80000001, brain::SH_ASR, 255, 1, 0xFFFFFFFF, 1},
      {0x80000001, brain::SH_ROR,   0, 0, 0x80000001, 0},
      {0x80000001, brain::SH_ROR,   0, 1, 0x80000001, 1},
      {0x80000001, brain::SH_ROR,   1, 1, 0xC0000000, 1},
      {0x80000001, brain::SH_ROR,   2, 1, 0x60000000, 0},
      {0x80000001, brain::SH_ROR,  15, 1, 0x00030000, 0},
      {0x80000001, brain::SH_ROR,  16, 1, 0x00018000, 0},
      {0x80000001, brain::SH_ROR,  30, 1, 0x00000006, 0},
      {0x80000001, brain::SH_ROR,  31, 1, 0x00000003, 0},
      {0x80000001, brain::SH_ROR,  32, 1, 0x80000001, 1},
      {0x80000001, brain::SH_ROR,  33, 1, 0xC0000000, 1},
      {0x80000001, brain::SH_ROR,  64, 1, 0x80000001, 1},
      {0x80000001, brain::SH_ROR, 255, 1, 0x00000003, 0},
      {0x80000001, brain::SH_RRX,   0, 0, 0x40000000, 1},
      {0x80000001, brain::SH_RRX,   0, 1, 0xC0000000, 1},
      {0x12345678, brain::SH_LSL,   0, 0, 0x12345678, 0},
      {0x12345678, brain::SH_LSL,   0, 1, 0x12345678, 1},
      {0x12345678, brain::SH_LSL,   1, 1, 0x2468ACF0, 0},
      {0x12345678, brain::SH_LSL,   2, 1, 0x48D159E0, 0},
      {0x12345678, brain::SH_LSL,  15, 1, 0x2B3C0000, 0},
      {0x12345678, brain::SH_LSL,  16, 1, 0x56780000, 0},
      {0x12345678, brain::SH_LSL,  30, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSL,  31, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSL,  32, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSL,  33, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSL,  64, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSL, 255, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSR,   0, 0, 0x12345678, 0},
      {0x12345678, brain::SH_LSR,   0, 1, 0x12345678, 1},
      {0x12345678, brain::SH_LSR,   1, 1, 0x091A2B3C, 0},
      {0x12345678, brain::SH_LSR,   2, 1, 0x048D159E, 0},
      {0x12345678, brain::SH_LSR,  15, 1, 0x00002468, 1},
      {0x12345678, brain::SH_LSR,  16, 1, 0x00001234, 0},
      {0x12345678, brain::SH_LSR,  30, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSR,  31, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSR,  32, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSR,  33, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSR,  64, 1, 0x00000000, 0},
      {0x12345678, brain::SH_LSR, 255, 1, 0x00000000, 0},
      {0x12345678, brain::SH_ASR,   0, 0, 0x12345678, 0},
      {0x12345678, brain::SH_ASR,   0, 1, 0x12345678, 1},
      {0x12345678, brain::SH_ASR,   1, 1, 0x091A2B3C, 0},
      {0x12345678, brain::SH_ASR,   2, 1, 0x048D159E, 0},
      {0x12345678, brain::SH_ASR,  15, 1, 0x00002468, 1},
      {0x12345678, brain::SH_ASR,  16, 1, 0x00001234, 0},
      {0x12345678, brain::SH_ASR,  30, 1, 0x00000000, 0},
      {0x12345678, brain::SH_ASR,  31, 1, 0x00000000, 0},
      {0x12345678, brain::SH_ASR,  32, 1, 0x00000000, 0},
      {0x12345678, brain::SH_ASR,  33, 1, 0x00000000, 0},
      {0x12345678, brain::SH_ASR,  64, 1, 0x00000000, 0},
      {0x12345678, brain::SH_ASR, 255, 1, 0x00000000, 0},
      {0x12345678, brain::SH_ROR,   0, 0, 0x12345678, 0},
      {0x12345678, brain::SH_ROR,   0, 1, 0x12345678, 1},
      {0x12345678, brain::SH_ROR,   1, 1, 0x091A2B3C, 0},
      {0x12345678, brain::SH_ROR,   2, 1, 0x048D159E, 0},
      {0x12345678, brain::SH_ROR,  15, 1, 0xACF02468, 1},
      {0x12345678, brain::SH_ROR,  16, 1, 0x56781234, 0},
      {0x12345678, brain::SH_ROR,  30, 1, 0x48D159E0, 0},
      {0x12345678, brain::SH_ROR,  31, 1, 0x2468ACF0, 0},
      {0x12345678, brain::SH_ROR,  32, 1, 0x12345678, 0},
      {0x12345678, brain::SH_ROR,  33, 1, 0x091A2B3C, 0},
      {0x12345678, brain::SH_ROR,  64, 1, 0x12345678, 0},
      {0x12345678, brain::SH_ROR, 255, 1, 0x2468ACF0, 0},
      {0x12345678, brain::SH_RRX,   0, 0, 0x091A2B3C, 0},
      {0x12345678, brain::SH_RRX,   0, 1, 0x891A2B3C, 0},
  };
  for (size_t i = 0; i < sizeof kRows / sizeof kRows[0]; ++i) {
    const Row& r = kRows[i];
    const brain::Shifted g = brain::shift_arm(r.x, r.type, r.amt, r.cin);
    char nm[96];
    std::snprintf(nm, sizeof nm,
                  "shift_arm(%08x, type %u, amt %u, cin %u) -> %08x c=%u", r.x,
                  r.type, r.amt, r.cin, r.val, r.carry);
    check(g.val == r.val && g.carry == r.carry, nm);
  }
}


/* ---- block cache: bookkeeping + the dispatcher, with no host code executed.
 *
 * The one step a real backend owns - calling into the arena - is replaced by
 * `jit_cache_exec_hook`, which lets this test drive translate/enter/retire on
 * a host without AArch64 execution.  What the generated code itself computes
 * is the business of tests/test_jit_arm64.py (AArch64 model); here we pin the
 * contract around it: what a block is, when it may run, when it must not, and
 * exactly what state crosses the shadow-register boundary. */
static int g_hook_calls = 0;

static void jit_cache_test_hook(uint32_t* s) {
  ++g_hook_calls;
  /* Play both block insns honestly: ADDS r0,r1,r2 then MOV r3,#5. */
  s[0] = s[1] + s[2];
  s[3] = 5;
}

static void test_jit_cache() {
  std::printf("[jit cache]\n");
  CHECK(BRAIN_JIT_BACKEND == 0);            /* no arena execution here        */

  brain::Cpu cpu;
  brain::CpuConfig cfg;
  cfg.ram_size = 1u << 20;
  CHECK(cpu.init(cfg));

  /* ADDS r0,r1,r2 ; MOV r3,#5 ; B .  - the branch must end the block. */
  /* ADDS ; MOV ; B . then at 0x8008 a `B` back to 0x8000 - a loop with two
   * translatable insns and an interpreted branch, so the dispatcher sees both
   * kinds of boundary; the branch retires through the interpreter. */
  const uint32_t prog[3] = {0xE0910002u, 0xE3A03005u, 3942645756u};
  for (uint32_t i = 0; i < 3; ++i) cpu.phys_write(0x8000 + i * 4, prog[i], 4);
  cpu.npc = 0x8000; cpu.cur = 0x8000; cpu.regs[15] = 0x8008;
  cpu.regs[1] = 1; cpu.regs[2] = 2;

  brain::JitCache* jc = brain::jit_cache_create(&cpu, 1);
  CHECK(jc != nullptr);
  if (!jc) return;

  CHECK(brain::jit_cache_lookup(jc, 0x8000) == nullptr);
  brain::JitBlock* b = nullptr;
  CHECK(brain::jit_cache_translate(jc, 0x8000, &b));
  CHECK(b != nullptr);
  if (!b) { brain::jit_cache_destroy(jc); return; }
  CHECK(b->pc == 0x8000);
  CHECK(b->end_pc == 0x8008);               /* stops before the B              */
  CHECK(b->insns == 2 && b->cycles == 2);   /* flat kIssue; cache.cpp why-note */
  CHECK(b->gen == cpu.trans_gen);
  CHECK(brain::jit_cache_lookup(jc, 0x8000) == b);

  /* enter(): copy-in, "run", copy-back, rebuild the pipeline view, retire. */
  g_hook_calls = 0;
  const uint64_t i0 = cpu.insns, c0 = cpu.cycles;
  brain::jit_cache_exec_hook = jit_cache_test_hook;
  brain::jit_cache_enter(jc, b);
  CHECK(g_hook_calls == 1);
  CHECK(cpu.regs[0] == 3u && cpu.regs[3] == 5u);
  CHECK(cpu.npc == 0x8008 && cpu.cur == 0x8004 && cpu.regs[15] == 0x800C);
  CHECK(cpu.insns == i0 + 2 && cpu.cycles == c0 + 2);
  CHECK(cpu.stats.insns_jit == 2 && cpu.stats.cycles_jit == 2);
  CHECK(cpu.stats.blocks == 1 && cpu.stats.code_bytes > 0);

  /* The dispatcher loop: block, block, ... with the untranslatable `B .`
   * interpreted between them; budgets cut across blocks, never inside. */
  brain::jit_cache_run_insns(jc, 4);         /* B, block, B: two interpreted + */
  CHECK(cpu.insns == i0 + 6);                /* two retired from one entry     */
  CHECK(cpu.stats.insns_jit == 4);
  CHECK(cpu.npc == 0x8000);                  /* the last interpreted B landed  */
  const uint64_t j1 = cpu.stats.insns_jit;
  brain::jit_cache_run_insns(jc, 1);         /* block would over-run: one DP   */
  CHECK(cpu.insns == i0 + 7 && cpu.stats.insns_jit == j1);
  brain::jit_cache_run_insns(jc, 3);         /* 1-instruction block at 0x8004, */
  CHECK(cpu.insns == i0 + 10 &&               /* then B, then a single DP too  */
        cpu.stats.insns_jit == j1 + 1);

  /* A stale block never runs: the boundary flushes instead, and the next
   * lookups see a freshly translated, current-generation block. */
  ++cpu.trans_gen;                            /* as flush_all/exceptions do      */
  const uint64_t j2 = cpu.stats.insns_jit, i2 = cpu.insns;
  brain::jit_cache_run_insns(jc, 2);
  /* The block at 0x8004 is stale: the boundary flushes the whole cache and
   * the budget is spent under the interpreter (MOV, then B). */
  CHECK(cpu.insns == i2 + 2 && cpu.stats.insns_jit == j2);
  CHECK(brain::jit_cache_lookup(jc, 0x8004) == nullptr);

  /* invalidation: overlap strictly, touching ends alone do not. */
  brain::JitBlock* b3 = nullptr;
  CHECK(brain::jit_cache_translate(jc, 0x8000, &b3) && b3 != nullptr);
  brain::jit_cache_invalidate(jc, 0x7FFC, 4); /* ends exactly at pc: keeps     */
  CHECK(brain::jit_cache_lookup(jc, 0x8000) == b3);
  brain::jit_cache_invalidate(jc, 0x8008, 4); /* starts at end_pc: keeps       */
  CHECK(brain::jit_cache_lookup(jc, 0x8000) == b3);
  brain::jit_cache_invalidate(jc, 0x8004, 4); /* inside: drops                 */
  CHECK(brain::jit_cache_lookup(jc, 0x8000) == nullptr);

  /* The core's SMC plumbing (`write_mem -> invalidate_code -> Jit::invalidate_
   * range`) runs only when the control plane owns a Jit, which on this host
   * `Jit::init()` refuses by design; the cache-side contract it calls into is
   * the invalidate() already pinned above, and `test_instr --jit` on aarch64
   * exercises the whole path under the real backend. */

  brain::jit_cache_exec_hook = nullptr;
  brain::jit_cache_destroy(jc);
}

int main() {
  std::printf("brain-emu unit tests: %s\n", brain_version_string());
  test_basic_run();
  test_thumb_entry();
  test_reset_clears();
  test_elf_loader();
  test_device_and_stop();
  test_be32_and_flags();
  test_exception_banking();
  test_fetch_abort_terminates();
  test_load_pc_and_l4();
  test_push_pop_return();
  test_exception_entry();
  test_jit_cache();
  test_interrupt_inputs();
  test_shifter_primitive();
  test_thumb_shift_zero();
  std::printf("%s: %d/%d checks passed\n", g_failed ? "FAILED" : "PASS",
              g_checks - g_failed, g_checks);
  return g_failed ? 1 : 0;
}
