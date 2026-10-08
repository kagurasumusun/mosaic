/* brain-emu -- what the loaders in this directory share.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A loader is small: an image format is a header, a placement rule and a list of
 * refusals.  The few things two of them would otherwise both write - a
 * little-endian field reader, a hex formatter for error messages, and the "is
 * this whole range writable RAM?" question every loader has to ask *before* it
 * writes a byte - live here, so each loader's own file is only its format.
 */
#ifndef BRAINEMU_LOADERS_IMAGE_H
#define BRAINEMU_LOADERS_IMAGE_H

#include <string>

#include "brainemu/bus.h"
#include "brainemu/types.h"

namespace brain {

inline u32 get_le32(const u8* p) {
  return (u32)p[0] | ((u32)p[1] << 8) | ((u32)p[2] << 16) | ((u32)p[3] << 24);
}
inline u16 get_le16(const u8* p) { return (u16)((u32)p[0] | ((u32)p[1] << 8)); }

inline std::string hex(u32 v) {
  static const char kDig[] = "0123456789abcdef";
  char buf[11];
  buf[0] = '0';
  for (int i = 0; i < 8; ++i) buf[1 + i] = kDig[(v >> (28 - i * 4)) & 0xF];
  buf[9] = 0;
  return std::string(buf);
}

/* Does the bus have writable RAM for every byte of [addr, addr+n)?  A loader has
 * to know *before* it writes anything, or a refused image leaves a half-loaded
 * machine behind. */
inline bool writable_range(Bus& bus, u32 addr, size_t n) {
  return bus.range_writable(addr, n);
}

}  // namespace brain

#endif  /* BRAINEMU_LOADERS_IMAGE_H */
