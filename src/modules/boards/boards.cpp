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
 */
#include <cstring>

#include "brainemu/core/machine.h"
#include "brainemu/core/module.h"

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

}  // namespace

void register_builtin_boards() {
  static const MinimalBoard minimal;
  static const TimerBoard timer;
  static bool done = false;
  if (done) return;
  done = true;
  ModuleRegistry::instance().add_board(&minimal);
  ModuleRegistry::instance().add_board(&timer);
}

}  // namespace brain
