/* brain-emu -- i.MX28 Machine composition
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */

#ifndef BRAINEMU_MACHINE_ARM_IMX28_IMX28_MACHINE_H
#define BRAINEMU_MACHINE_ARM_IMX28_IMX28_MACHINE_H

#include <memory>
#include <string>
#include <vector>

#include "arch/arm/cpu/classic/arm9/arm926ej-s/core/cpu.h"
#include "soc/nxp/imx28/imx28_soc.h"

namespace brain::machine::imx28 {

class Imx28Machine {
 public:
  Imx28Machine();
  ~Imx28Machine() = default;

  bool init(bool use_jit = false, uint32_t ram_size_mb = 128);
  bool load_emmc_file(const std::string& path);
  bool load_emmc_multipart(const std::vector<std::string>& part_paths);

  void reset();
  uint64_t run(uint64_t insns);

  arm926::Cpu& cpu() { return cpu_; }
  soc::imx28::Imx28SoC& soc() { return soc_; }

 private:
  void boot_emmc_image();

  arm926::Cpu cpu_;
  soc::imx28::Imx28SoC soc_;
  std::shared_ptr<soc::imx28::EmmcStorage> storage_;
};

}  // namespace brain::machine::imx28

#endif  /* BRAINEMU_MACHINE_ARM_IMX28_IMX28_MACHINE_H */
