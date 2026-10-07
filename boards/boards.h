/* The boards this build ships.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A board is a *preset*: a named way to assemble a machine out of modules that
 * already exist, which writes the same `MachineConfig` a caller would.  They are
 * kept together in one file because that is all they are - three short
 * `configure()` bodies - and because a preset is only meaningful next to the
 * other presets of the same build.
 */
#ifndef BRAINEMU_BOARDS_BOARDS_H
#define BRAINEMU_BOARDS_BOARDS_H

namespace brain {

/* Adds this module to the registry.  Called once by the build's module list
 * (`builtin/builtin.cpp`) - the only place that decides what a build contains -
 * so a module the list does not name is not linked in at all, and the failure
 * is a linker error naming the module rather than a runtime surprise. */
void register_builtin_boards();

}  // namespace brain

#endif  /* BRAINEMU_BOARDS_BOARDS_H */
