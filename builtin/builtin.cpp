/* brain-emu -- the modules this build contains.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A module registers *itself*, and this is the one place that says which modules
 * exist.  It has to be a function rather than a static initialiser because the
 * library is a static archive: an object file nothing references is not linked
 * in, so a module that only had a static registration object would silently
 * vanish from the build.  A build that drops a module drops a line here, and
 * gets a linker error naming the module instead of a runtime "no CPU modules"
 * surprise.
 *
 * That is also the whole contract between the tree and this file: every
 * component directory exports exactly one `register_*()` (the function its own
 * header declares), and this file includes those headers and calls them.  It
 * knows nothing else about any of them - not a class, not a register map, not an
 * opcode - so a machine, a device, a loader or a board is added by writing a
 * directory and adding two lines here:
 *
 *   #include "devices/uart/uart.h"
 *   register_uart_module();
 *
 * The layout itself is documented in docs/ARCHITECTURE.md and the README; the
 * rule in one line: `core/` is the foundation, and everything else is a
 * component that plugs into it and names itself here.
 */
#include "builtin/builtin.h"

#include "boards/generic/boards.h"
#include "brainemu/module.h"
#include "devices/console/console.h"
#include "devices/system/exit/exit.h"
#include "devices/interrupt/intc/intc.h"
#include "devices/timer/timer.h"
#include "loaders/elf/elf32/elf32.h"
#include "loaders/raw/raw.h"
#include "arch/arm/cpu/classic/arm9/arm926ej-s/arm926.h"
#include "arch/experimental/toy8/toy8.h"

namespace brain {

void register_builtin_modules() {
  static bool done = false;
  if (done) return;
  done = true;

  /* Machines: one CPU module each.  The first one registered is the default, so
   * the tree's primary target comes first. */
  register_arm926_module();      /* ARM926EJ-S (ARMv5TEJ): the primary target */
  register_toy8_module();        /* a five-register CPU: the worked example  */

  /* Loaders: image formats the machine can place. */
  register_elf32_loader();
  register_raw_loader();

  /* Devices: what is mapped into the address space. */
  register_exit_module();
  register_console_module();
  register_timer_module();       /* an interrupt output                      */
  register_intc_module();        /* interrupt inputs and one output          */

  /* Boards: named presets of the above. */
  register_builtin_boards();
}

}  // namespace brain
