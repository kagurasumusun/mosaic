/* brain-emu -- the boards this build ships.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A board is a *preset*: a named way to assemble a machine out of modules that
 * already exist.  It writes the same `MachineConfig` a caller would, so there is
 * exactly one code path from "what the machine is" to "the machine runs", and a
 * caller's own choices (a different CPU, more RAM, another device, another wire)
 * are applied on top of it.
 *
 * The two boards here are the CPU-core project's own test benches, not models of
 * any computer: a target is added by writing these three lines for its real
 * peripherals, or by naming a different set on the command line.
 *
 *   --board minimal        console + exit port
 *   --board timer          the same, plus a timer wired to the CPU's IRQ input
 *   --board dual-timer     two timers behind an interrupt controller, on IRQ
 *
 * The last one is the shape a real SoC has, in miniature, and the reason devices
 * have input pins and instances: two sources share one CPU interrupt line, and
 * the CPU is not asked to tell them apart.
 */
#include "boards/boards.h"

#include <cstring>

#include "brainemu/machine.h"
#include "brainemu/module.h"

namespace brain {

namespace {

class MinimalBoard final : public BoardModule {
 public:
  const ModuleInfo& info() const override { return info_; }
  void configure(MachineConfig& cfg) const override {
    cfg.add_device(ModuleSpec("console"));
    cfg.add_device(ModuleSpec("exit"));
  }

 private:
  ModuleInfo info_{"minimal", "CPU + console + exit port", ModuleKind::Board,
                   "the smallest useful machine: a display port and a way to stop"};
};

/* The wiring demonstration: the timer's interrupt output goes to the CPU's
 * `irq` input.  Neither module knows the other exists - the wire is the whole
 * connection, and `--wire` on the command line is the same thing from outside. */
class TimerBoard final : public BoardModule {
 public:
  const ModuleInfo& info() const override { return info_; }
  void configure(MachineConfig& cfg) const override {
    MinimalBoard().configure(cfg);
    cfg.add_device(ModuleSpec("timer"));
    cfg.add_wire("timer", "irq", "irq");
  }

 private:
  ModuleInfo info_{"timer", "minimal + an IRQ-wired timer", ModuleKind::Board,
                   "a timing source on the CPU's interrupt input"};
};

/* Two sources, one CPU line.  `t0` and `t1` are two *instances* of the one timer
 * module (which is why a device needs an instance name at all), each wired to an
 * input of the controller; the controller's single output goes to the CPU.  The
 * chain timer -> intc -> CPU is three wires and no code: a peripheral drives a
 * line, a device with inputs aggregates, and the CPU input is just the last sink
 * in the chain. */
class DualTimerBoard final : public BoardModule {
 public:
  const ModuleInfo& info() const override { return info_; }
  void configure(MachineConfig& cfg) const override {
    MinimalBoard().configure(cfg);
    cfg.add_device(ModuleSpec("timer", 0x10001000u, true, "t0"));
    cfg.add_device(ModuleSpec("timer", 0x10001100u, true, "t1"));
    cfg.add_device(ModuleSpec("intc"));
    cfg.add_wire("t0", "irq", "intc.irq0");
    cfg.add_wire("t1", "irq", "intc.irq1");
    cfg.add_wire("intc", "irq", "irq");
  }

 private:
  ModuleInfo info_{"dual-timer", "minimal + 2 timers behind an interrupt controller",
                   ModuleKind::Board,
                   "two interrupt sources sharing one CPU line, the SoC way"};
};

}  // namespace

void register_builtin_boards() {
  static const MinimalBoard minimal;
  static const TimerBoard timer;
  static const DualTimerBoard dual;
  static bool done = false;
  if (done) return;
  done = true;
  ModuleRegistry::instance().add_board(&minimal);
  ModuleRegistry::instance().add_board(&timer);
  ModuleRegistry::instance().add_board(&dual);
}

}  // namespace brain
