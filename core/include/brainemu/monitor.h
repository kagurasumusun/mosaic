/* brain-emu -- monitors: observers of a running machine.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A monitor is the third kind of module after a CPU and a device, and it is the
 * one that is easiest to get wrong by making it part of the core: a tracer, a
 * profiler, a coverage collector, a watchpoint layer or a gdb stub all want to
 * watch a machine without being a machine.  So the run loop tells them what
 * happened, at exactly the boundaries the CPU module is called at - a slice
 * ended, the machine was reset - and they decide what to do about it.
 *
 * The hooks are called by the thread that runs the machine, never re-entrantly,
 * with the machine in a consistent state (the CPU has returned and the devices
 * are ticked), so a monitor can read registers, memory and counters.  A hook
 * may stop the machine (`Machine::stop`): that is how a watchpoint or a
 * breakpoint is expressed, and the stop takes effect at the next slice
 * boundary like any other.
 */
#ifndef BRAINEMU_CORE_MONITOR_H
#define BRAINEMU_CORE_MONITOR_H

#include "brainemu/module.h"
#include "brainemu/types.h"

namespace brain {

class Machine;

class Monitor {
 public:
  virtual ~Monitor() = default;
  virtual const ModuleInfo& info() const = 0;

  /* One slice of execution finished: `retired` instructions were retired by it
   * (zero is normal - a step over a stalled CPU, a slice that ran out of
   * budget).  Called after the devices were ticked. */
  virtual void on_slice(Machine& m, u64 retired) {
    (void)m;
    (void)retired;
  }

  /* The machine was reset: registers, counters and devices went back to their
   * starting state.  Called last, after the CPU and the devices. */
  virtual void on_reset(Machine& m) { (void)m; }

  /* Self-description, for tools: whatever this observer wants a report to show
   * (a slice count, a histogram's top entry, a hit or miss tally).  Same
   * contract as `Device::field_*` - name/value text the core never interprets. */
  virtual u32 field_count() const { return 0; }
  virtual const char* field_name(u32 i) const { (void)i; return ""; }
  virtual const char* field_value(u32 i) const { (void)i; return ""; }
};

}  // namespace brain

#endif  /* BRAINEMU_CORE_MONITOR_H */
