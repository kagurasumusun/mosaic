/* brain-emu -- the flat-binary loader.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A raw image carries no header, so nothing about it is guessable: it is what a
 * boot ROM has when the design decided the load address and the image together,
 * and the caller states the address (`Machine::load_raw`, `--addr`,
 * `brain_load_raw`).  It is therefore never auto-detected - `probe` says no -
 * and the loader exists so that "place these bytes there" is a module like any
 * other rather than a special case in the machine.
 */
#include "loaders/raw/raw.h"

#include <string>

#include "brainemu/bus.h"
#include "brainemu/loader.h"
#include "brainemu/module.h"
#include "loaders/image.h"

namespace brain {
namespace {

class RawLoader final : public Loader {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo kMod = {"raw", "flat binary", ModuleKind::Loader,
                                    "an image with no header, placed at the "
                                    "address the caller gives"};
    return kMod;
  }
  /* A flat image has nothing to recognise: it is what a boot ROM has when the
   * design decided load address and image together, so it is never
   * auto-detected - the caller asks for it (Machine::load_raw). */
  bool probe(const void*, u64) const override { return false; }
  int load(Bus& bus, const void* data, u64 size, ImageInfo* out,
           std::string& err) const override {
    /* Without an address a flat image cannot be placed; the machine passes one
     * through load_raw, and this path exists so the interface is total. */
    ImageInfo info;
    info.format = "raw";
    info.entry = 0;
    info.segments = 1;
    info.max = (u32)size;
    if (size && !writable_range(bus, 0, (size_t)size)) {
      err = "a raw image needs a load address (see --addr)";
      return -1;
    }
    if (size && !bus.write_bytes(0, data, (size_t)size)) {
      err = "raw image does not fit in writable memory";
      return -1;
    }
    if (out) *out = info;
    return 0;
  }
};

}  // namespace

const Loader& loader_raw() {
  static const RawLoader kLoader;
  return kLoader;
}

int raw_load(Bus& bus, const void* image, size_t size, u32 addr,
             std::string& err) {
  if (!image || !size) {
    err = "empty image";
    return -1;
  }
  if (!writable_range(bus, addr, size)) {
    err = "image does not fit in writable memory";
    return -1;
  }
  if (!bus.write_bytes(addr, image, size)) {
    err = "image does not fit in mapped memory";
    return -1;
  }
  return 0;
}

void register_raw_loader() {
  static bool done = false;
  if (done) return;
  done = true;
  ModuleRegistry::instance().add_loader(&loader_raw());
}

}  // namespace brain
