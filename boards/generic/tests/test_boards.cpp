/* brain-emu -- the board modules' test suite.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A board is a *preset*: a named way to assemble a machine out of modules that
 * already exist.  What there is to test is therefore the whole chain - the preset
 * fills the configuration, the caller's own choices are applied on top, the wires
 * resolve to the ends they name, and a real guest runs on the result and stops
 * itself.  The two end-to-end images here are the machine the `--board timer` and
 * `--board dual-timer` command lines run, assembled by hand and checked on both
 * engines, because a board is exactly where an interrupt path is either right or
 * mysteriously wrong.
 */
#include <brainemu/brainemu.h>

#include "brainemu/bus.h"
#include "brainemu/device.h"
#include "brainemu/machine.h"
#include "brainemu/module.h"
#include "testkit/test_support.h"

#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

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

static const uint32_t kBoardVectors[] = {
    /* reset, undef, svc, pabort, dabort, reserved, irq, fiq */
    0xEA001FFEu, 0xEAFFFFFEu, 0xEAFFFFFEu, 0xEAFFFFFEu,
    0xEAFFFFFEu, 0xEAFFFFFEu, 0xEA002038u, 0xEAFFFFFEu,
};
static const uint32_t kBoardProg[] = {
    0xE3A09201u,  /* mov r9, #0x10000000  */
    0xE2899A01u,  /* add r9, r9, #0x1000  */
    0xE3A00001u,  /* mov r0, #1           */
    0xE5890008u,  /* str r0, [r9, #8]     */
    0xE3A000C8u,  /* mov r0, #200         */
    0xE5890000u,  /* str r0, [r9]         */
    0xE10F0000u,  /* mrs r0, cpsr         */
    0xE3C00080u,  /* bic r0, r0, #0x80    */
    0xE121F000u,  /* msr cpsr_c, r0       */
    0xE3A0B000u,  /* mov r11, #0          */
    0xEAFFFFFEu,  /* b .      (0x8028)    */
};
static const uint32_t kBoardHandler[] = {
    0xE28BB001u,  /* add r11, r11, #1     */
    0xE35B0003u,  /* cmp r11, #3          */
    0x1A000001u,  /* bne 0x8114           */
    0xE3A00003u,  /* mov r0, #3           */
    0xE3A0C201u,  /* mov r12, #0x10000000 */
    0xE28CC010u,  /* add r12, r12, #0x10  */
    0xE58C0000u,  /* str r0, [r12]        */
    0xEAFFFFFFu,  /* b .                  */
    0xE3A08001u,  /* mov r8, #1           */
    0xE5898010u,  /* str r8, [r9, #16]    */
    0xE25EF004u,  /* subs pc, lr, #4      */
};

/* The three segments at the addresses they belong at. */
static std::vector<uint8_t> board_image() {
  std::vector<uint8_t> img(0x9000u, 0);
  auto put = [&](uint32_t at, const uint32_t* w, size_t n) {
    for (size_t i = 0; i < n; ++i) {
      img[at + i * 4 + 0] = (uint8_t)w[i];
      img[at + i * 4 + 1] = (uint8_t)(w[i] >> 8);
      img[at + i * 4 + 2] = (uint8_t)(w[i] >> 16);
      img[at + i * 4 + 3] = (uint8_t)(w[i] >> 24);
    }
  };
  put(0x0000u, kBoardVectors, sizeof kBoardVectors / 4);
  put(0x8000u, kBoardProg, sizeof kBoardProg / 4);
  put(0x8100u, kBoardHandler, sizeof kBoardHandler / 4);
  return img;
}

static void test_board_timer_irq() {
  std::printf("[board: timer -> IRQ -> guest]\n");
  const std::vector<uint8_t> img = board_image();
  for (int jit = 0; jit < 2; ++jit) {
    brain_config_t cfg;
    std::memset(&cfg, 0, sizeof cfg);
    cfg.ram_size = 1u << 20;
    cfg.board_id = "timer";                 /* console + exit + timer, wired */
    cfg.use_jit = jit;
    brain_emu_t* e = brain_emu_create(&cfg);
    CHECK(e != nullptr);
    if (!e) continue;
    CHECK(std::strcmp(brain_board_id(e), "timer") == 0);
    CHECK(brain_wire_count(e) == 1);
    CHECK(brain_add_wire(e, "timer", "irq", "fiq") == 0);   /* a second wire */
    CHECK(brain_wire_count(e) == 2);
    CHECK(brain_wire_level(e, 0) == 0);
    CHECK(brain_wire_level(e, 1) == 0);
    CHECK(brain_load_raw(e, img.data(), img.size(), 0, 0) == 0);

    const uint64_t ran = brain_run_insns(e, 200000);
    check(ran > 100u && ran < 200000u, "the guest stopped the run itself");

    brain_stats_t st;
    brain_get_stats(e, &st);
    check(st.exc[BRAIN_EXC_IRQ] == 3u, "one IRQ per expiry the guest serviced");
    check(st.exc[BRAIN_EXC_DABORT] == 0u && st.exc[BRAIN_EXC_PABORT] == 0u,
          "no abort on the way to the timer's window");

    /* The exit device's own fields: the guest wrote 3 there.  A tool reads a
     * device through exactly this self-description. */
    uint32_t exit_dev = brain_device_count(e);
    for (uint32_t i = 0; i < brain_device_count(e); ++i)
      if (!std::strcmp(brain_device_id(e, i), "exit")) exit_dev = i;
    CHECK(exit_dev < brain_device_count(e));
    if (exit_dev < brain_device_count(e)) {
      CHECK(std::strcmp(brain_device_field_value(e, exit_dev, 0), "0x00000003") == 0);
      CHECK(std::strcmp(brain_device_field_value(e, exit_dev, 2), "1") == 0);
    }

    /* The timer expired on its own clock while the CPU ran (a device only
     * advanced when the run *ends* would show one expiry here, and a machine
     * that never ticked it would show none), and the third interrupt is still
     * pending: the guest left through the exit port from inside the handler,
     * before acknowledging. */
    uint32_t timer_dev = brain_device_count(e);
    for (uint32_t i = 0; i < brain_device_count(e); ++i)
      if (!std::strcmp(brain_device_id(e, i), "timer")) timer_dev = i;
    CHECK(timer_dev < brain_device_count(e));
    if (timer_dev < brain_device_count(e)) {
      CHECK(std::strcmp(brain_device_field_name(e, timer_dev, 1), "ticks") == 0);
      CHECK(std::atoi(brain_device_field_value(e, timer_dev, 1)) >= 3);
      CHECK(std::strcmp(brain_device_field_value(e, timer_dev, 2), "yes") == 0);
    }
    CHECK(brain_wire_level(e, 0) == 1);
    CHECK(brain_wire_level(e, 1) == 1);          /* the caller's wire too */
    brain_emu_destroy(e);
  }
}

/* The same chain a real SoC has, end to end: two interrupt sources behind one
 * controller, which drives the CPU's IRQ input.  Nothing in the core knows what a
 * timer or a controller is - the board is a preset, the three wires are name
 * pairs, and the guest's console log says which source it serviced.
 *
 * The image (hand-assembled words, keystone-verified) is the flat 0x9000-byte ROM
 * image the `--addr 0 --board dual-timer` command line runs:
 *
 *   0x0000  vectors: reset -> 0x8000, IRQ -> 0x8100
 *   0x8000  r7 = console, r9 = t0 (0x10001000), r10 = t1 (0x10001100),
 *           r8 = intc (0x10002000): t0 period 100, t1 period 150, both enabled,
 *           intc MASK = 0b11, CPSR.I cleared, then spin
 *   0x8100  the handler: read intc PENDING; if line 0 is up acknowledge t0 and
 *           print '0'; if line 1 is up acknowledge t1 and print '1'; count the
 *           interrupts and, on the second, write 0x11 through the board's exit
 *           port - the guest stops the run itself, from inside the handler.
 *
 * The log is "0101": each of the two entries found *both* lines pending - the
 * devices are ticked once per run slice, so both timers expire inside the first
 * slice's clock and their lines are up together - and the handler resolves them
 * from PENDING, acknowledging each and printing which one it was.  That the CPU
 * saw two interrupts and not four is the controller doing its job: the guest
 * never looks at a timer's status or at which line fired, only at PENDING. */
static const uint32_t kDualVectors[] = {
    0xEA001FFEu, 0xEAFFFFFEu, 0xEAFFFFFEu, 0xEAFFFFFEu,
    0xEAFFFFFEu, 0xEAFFFFFEu, 0xEA002038u, 0xEAFFFFFEu,
};
static const uint32_t kDualProg[] = {
    0xE3A07201u,  /* mov r7, #0x10000000   (console)      */
    0xE3A09201u,  /* mov r9, #0x10000000                  */
    0xE2899A01u,  /* add r9, r9, #0x1000   (t0)           */
    0xE289AC01u,  /* add r10, r9, #0x100   (t1)           */
    0xE3A08201u,  /* mov r8, #0x10000000                  */
    0xE2888A02u,  /* add r8, r8, #0x2000   (intc)         */
    0xE3A00064u,  /* mov r0, #100                         */
    0xE5890000u,  /* str r0, [r9]          t0 PERIOD      */
    0xE3A00096u,  /* mov r0, #150                         */
    0xE58A0000u,  /* str r0, [r10]         t1 PERIOD      */
    0xE3A00001u,  /* mov r0, #1                           */
    0xE5890008u,  /* str r0, [r9, #8]      t0 enable      */
    0xE3A00001u,  /* mov r0, #1                           */
    0xE58A0008u,  /* str r0, [r10, #8]     t1 enable      */
    0xE3A00003u,  /* mov r0, #3                           */
    0xE5880000u,  /* str r0, [r8]          intc MASK = 3  */
    0xE10F0000u,  /* mrs r0, cpsr                         */
    0xE3C00080u,  /* bic r0, r0, #0x80                    */
    0xE121F000u,  /* msr cpsr_c, r0        unmask IRQ     */
    0xE3A0B000u,  /* mov r11, #0                          */
    0xEAFFFFFEu,  /* b .                                  */
};
static const uint32_t kDualHandler[] = {
    0xE5980008u,  /* ldr r0, [r8, #8]      intc PENDING  */
    0xE3100001u,  /* tst r0, #1                           */
    0x0A000003u,  /* beq 0x811C                           */
    0xE3A01001u,  /* mov r1, #1                           */
    0xE5891010u,  /* str r1, [r9, #0x10]   ack t0        */
    0xE3A01030u,  /* mov r1, #'0'                         */
    0xE5C71000u,  /* strb r1, [r7]         console        */
    0xE3100002u,  /* tst r0, #2                           */
    0x0A000003u,  /* beq 0x8134                           */
    0xE3A01001u,  /* mov r1, #1                           */
    0xE58A1010u,  /* str r1, [r10, #0x10]  ack t1        */
    0xE3A01031u,  /* mov r1, #'1'                         */
    0xE5C71000u,  /* strb r1, [r7]                        */
    0xE28BB001u,  /* add r11, r11, #1                     */
    0xE35B0002u,  /* cmp r11, #2                          */
    0x1A000003u,  /* bne 0x8150                           */
    0xE3A00011u,  /* mov r0, #0x11                        */
    0xE3A0C201u,  /* mov r12, #0x10000000                 */
    0xE28CC010u,  /* add r12, r12, #0x10                  */
    0xE58C0000u,  /* str r0, [r12]         exit(0x11)     */
    0xE25EF004u,  /* subs pc, lr, #4                      */
};

static std::vector<uint8_t> dual_timer_image() {
  std::vector<uint8_t> img(0x9000u, 0);
  auto put = [&](uint32_t at, const uint32_t* w, size_t n) {
    for (size_t i = 0; i < n; ++i)
      for (int b = 0; b < 4; ++b)
        img[at + i * 4 + (size_t)b] = (uint8_t)(w[i] >> (8 * b));
  };
  put(0x0000u, kDualVectors, sizeof kDualVectors / 4);
  put(0x8000u, kDualProg, sizeof kDualProg / 4);
  put(0x8100u, kDualHandler, sizeof kDualHandler / 4);
  return img;
}

static void test_board_dual_timer_irq() {
  std::printf("[board: 2 timers -> controller -> IRQ -> guest]\n");
  const std::vector<uint8_t> img = dual_timer_image();
  for (int jit = 0; jit < 2; ++jit) {
    brain_config_t cfg;
    std::memset(&cfg, 0, sizeof cfg);
    cfg.ram_size = 1u << 20;
    cfg.board_id = "dual-timer";
    cfg.use_jit = jit;
    brain_emu_t* e = brain_emu_create(&cfg);
    CHECK(e != nullptr);
    if (!e) continue;
    CHECK(std::strcmp(brain_board_id(e), "dual-timer") == 0);
    /* The board's own wires: two sources into the controller, one out to the
     * CPU.  The instance names are what tell the two timers apart. */
    CHECK(brain_wire_count(e) == 3);
    CHECK(brain_wire_level(e, 2) == 0);
    CHECK(brain_load_raw(e, img.data(), img.size(), 0, 0) == 0);

    const uint64_t ran = brain_run_insns(e, 400000);
    check(ran > 50u && ran < 400000u, "the guest stopped the run itself");

    brain_stats_t st;
    brain_get_stats(e, &st);
    /* Two interrupts, not three: the first entry found both lines pending and
     * serviced both, which is exactly what an interrupt controller is for. */
    check(st.exc[BRAIN_EXC_IRQ] == 2u, "one IRQ per handler entry, not per source");
    check(st.exc[BRAIN_EXC_DABORT] == 0u && st.exc[BRAIN_EXC_PABORT] == 0u, "no abort");

    uint32_t con = brain_device_count(e), ex = brain_device_count(e), ic = ex;
    for (uint32_t i = 0; i < brain_device_count(e); ++i) {
      if (!std::strcmp(brain_device_id(e, i), "console")) con = i;
      if (!std::strcmp(brain_device_id(e, i), "exit")) ex = i;
      if (!std::strcmp(brain_device_id(e, i), "intc")) ic = i;
    }
    CHECK(con < brain_device_count(e) && ex < brain_device_count(e) &&
          ic < brain_device_count(e));
    if (con < brain_device_count(e))
      check(std::strcmp(brain_device_field_value(e, con, 1), "0101") == 0,
            "each entry serviced both lines, in the order it found them");
    if (ex < brain_device_count(e))
      check(std::strcmp(brain_device_field_value(e, ex, 0), "0x00000011") == 0,
            "the guest left through the exit port with its own code");
    if (ic < brain_device_count(e)) {
      CHECK(std::strcmp(brain_device_field_name(e, ic, 2), "pending") == 0);
      CHECK(std::strcmp(brain_device_field_value(e, ic, 1), "0x3") == 0);
    }
    brain_emu_destroy(e);
  }
}


/* A guest that does nothing but wait, and a timer that wakes it.
 *
 * This is the run loop's hardest case and the reason it is bounded on cycles as
 * well as instructions: between two interrupts the guest retires *nothing* - it
 * sits in the ARM926EJ-S's wait-for-interrupt (`MCR p15, 0, Rd, c7, c0, 4`).
 * An instruction budget alone would never bring control back to the machine, so
 * the device that is supposed to raise the line would never be ticked, and the
 * run would never end.  Both engines have to get through it, and the console
 * has to show exactly one wake-up per timer expiry.
 */
static const uint32_t kIdleProg[] = {
    0xE3A09201u,  /* mov r9, #0x10000000   */
    0xE2899A01u,  /* add r9, r9, #0x1000   -> 0x10001000 (the timer)  */
    0xE3A00001u,  /* mov r0, #1            */
    0xE5890008u,  /* str r0, [r9, #8]      CTRL: enable               */
    0xE3A000C8u,  /* mov r0, #200          */
    0xE5890000u,  /* str r0, [r9]          PERIOD: 200 cycles         */
    0xE10F0000u,  /* mrs r0, cpsr          */
    0xE3C00080u,  /* bic r0, r0, #0x80     clear I: interrupts are on */
    0xE121F000u,  /* msr cpsr_c, r0        */
    0xEE070F90u,  /* mcr p15, 0, r0, c7, c0, 4   wait for interrupt  */
    0xE3A01007u,  /* mov r1, #7            woken: the exit code       */
    0xE3A0C201u,  /* mov r12, #0x10000000  */
    0xE28CC010u,  /* add r12, r12, #0x10   -> 0x10000010 (the exit)   */
    0xE58C1000u,  /* str r1, [r12]         leave with 7               */
    0xEAFFFFFEu,  /* b .                                              */
};
static const uint32_t kIdleVectors[] = {
    /* reset, undef, svc, pabort, dabort, reserved, irq, fiq */
    0xEA001FFEu, 0xEAFFFFFEu, 0xEAFFFFFEu, 0xEAFFFFFEu,
    0xEAFFFFFEu, 0xEAFFFFFEu, 0xEA002033u, 0xEAFFFFFEu,
};
/* The handler acknowledges the timer (write 1 to ACK at +0x10) and returns.
 * Without the acknowledgement the line would stay high and the interrupt would
 * be re-taken the moment the return cleared CPSR's I bit - the classic "handler
 * that never clears its source" livelock. */
static const uint32_t kIdleHandler[] = {
    0xE3A08001u,  /* mov r8, #1           */
    0xE5898010u,  /* str r8, [r9, #16]    ACK: drop the line          */
    0xE25EF004u,  /* subs pc, lr, #4      return to after the wait    */
};
static std::vector<uint8_t> idle_image() {
  std::vector<uint8_t> img(0x9000, 0);
  auto put = [&img](uint32_t addr, const uint32_t* w, size_t n) {
    std::memcpy(img.data() + addr, w, n * 4);
  };
  put(0, kIdleVectors, sizeof kIdleVectors / 4);
  put(0x8000u, kIdleProg, sizeof kIdleProg / 4);
  put(0x8100u, kIdleHandler, sizeof kIdleHandler / 4);
  return img;
}

static void test_board_idle_wait() {
  std::printf("[board: a waiting guest, woken by its timer]\n");
  const std::vector<uint8_t> img = idle_image();
  for (int jit = 0; jit < 2; ++jit) {
    brain_config_t cfg;
    std::memset(&cfg, 0, sizeof cfg);
    cfg.ram_size = 1u << 20;
    cfg.board_id = "timer";
    cfg.use_jit = jit;
    brain_emu_t* e = brain_emu_create(&cfg);
    CHECK(e != nullptr);
    if (!e) continue;
    CHECK(brain_load_raw(e, img.data(), img.size(), 0, 0) == 0);

    /* The budget is in instructions and the guest spends its life retiring
     * none, so this returning at all is the property under test. */
    const uint64_t ran = brain_run_insns(e, 20000);
    check(ran > 10u && ran < 20000u, "the guest left through the exit port");
    brain_stats_t st;
    brain_get_stats(e, &st);
    check(st.exc[BRAIN_EXC_IRQ] == 1u, "one wake-up, one interrupt");
    check(st.cycles > 200u, "and the wait still charged time");

    uint32_t ex = brain_device_count(e), tm = ex;
    for (uint32_t i = 0; i < brain_device_count(e); ++i) {
      if (!std::strcmp(brain_device_id(e, i), "exit")) ex = i;
      if (!std::strcmp(brain_device_id(e, i), "timer")) tm = i;
    }
    if (ex < brain_device_count(e))
      check(std::strcmp(brain_device_field_value(e, ex, 0), "0x00000007") == 0,
            "the woken guest carried on after the wait");
    if (tm < brain_device_count(e))
      check(std::strcmp(brain_device_field_value(e, tm, 2), "no") == 0,
            "and the handler acknowledged the timer");
    brain_emu_destroy(e);
  }
}


int main() {
  std::printf("brain-emu board tests: %s\n", brain_version_string());
  brainemutest::ensure_builtins();
  test_boards_and_wiring();
  test_board_timer_irq();
  test_board_dual_timer_irq();
  test_board_idle_wait();
  return brainemutest::summary("boards");
}
