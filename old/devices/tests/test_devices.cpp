/* brain-emu -- the device modules' test suite.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A device is a window in the address space plus behaviour, and these checks are
 * about the behaviour the core promises it: the bus routes to it, `tick` is what
 * tells it time passed, the field list is how a tool prints its state, and an
 * interrupt output is a level somebody else carries.  The devices tested are the
 * ones this build ships (the console, the timer, the interrupt controller), each
 * driven both on its own bus and through a machine.
 */
#include <brainemu/brainemu.h>

#include "brainemu/bus.h"
#include "brainemu/device.h"
#include "brainemu/machine.h"
#include "brainemu/module.h"
#include "devices/console/console_device.h"
#include "testkit/test_support.h"

#include <cstdio>
#include <cstring>
#include <memory>
#include <string>

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



/* The console: bytes written become text, a read finds nothing waiting, the log
 * keeps its tail instead of refusing to grow, and `reset` is the device's own
 * business (the console keeps what was printed - a machine reset is not a reason
 * to lose the output that explained it). */
void test_console_device() {
  std::printf("[console device]\n");
  Bus bus;
  ConsoleDevice con(0x5000);
  CHECK(con.attach(bus));
  const char* msg = "ok\n";
  for (const char* p = msg; *p; ++p) CHECK(bus.write_byte(0x5000, (u8)*p));
  CHECK(std::strcmp(con.log(), "ok\n") == 0);
  CHECK(con.log_len() == 3u);
  u8 b = 0;
  CHECK(bus.read_byte(0x5000, &b) && b == 0);
  CHECK(std::strcmp(con.field_name(0), "bytes") == 0);
  /* A 32-bit store is four bytes, in order. */
  con.clear();
  con.write(0, 0x44434241u, 4);
  CHECK(std::strcmp(con.log(), "ABCD") == 0);
  /* The log keeps its tail rather than refusing to grow. */
  for (int i = 0; i < 3000; ++i) con.write(0, (u32)('a' + i % 26), 1);
  CHECK(con.log_len() > 0u && con.log_len() < ConsoleDevice::kLogSize);
  CHECK(std::strlen(con.log()) == con.log_len());
  const u32 len = con.log_len();
  con.reset();
  CHECK(con.log_len() == len);
}

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

int main() {
  std::printf("brain-emu device tests: %s\n", brain_version_string());
  brainemutest::ensure_builtins();
  test_console_device();
  test_timer_device();
  test_interrupt_chain();
  return brainemutest::summary("devices");
}
