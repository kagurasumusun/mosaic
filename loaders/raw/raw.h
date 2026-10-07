/* The raw (flat binary) loader: registration, and nothing else.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#ifndef BRAINEMU_LOADERS_RAW_H
#define BRAINEMU_LOADERS_RAW_H

namespace brain {

/* Adds the loader to the registry (see register_elf32_loader). */
void register_raw_loader();

}  // namespace brain

#endif  /* BRAINEMU_LOADERS_RAW_H */
