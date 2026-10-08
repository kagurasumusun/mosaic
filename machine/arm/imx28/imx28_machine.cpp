/* brain-emu -- i.MX28 Machine implementation
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */

#include "machine/arm/imx28/imx28_machine.h"

#include <cstdio>
#include <cstring>

namespace brain::machine::imx28 {

Imx28Machine::Imx28Machine() {
  storage_ = std::make_shared<soc::imx28::EmmcStorage>();
}

bool Imx28Machine::init(bool use_jit, uint32_t ram_size_mb) {
  arm926::CpuConfig cfg;
  cfg.ram_base = 0x40000000u;  /* i.MX28 DRAM Base */
  cfg.ram_size = ram_size_mb << 20;
  cfg.use_jit = use_jit;

  if (!cpu_.init(cfg)) return false;
  if (!soc_.attach_to_bus(*cpu_.bus)) return false;
  soc_.attach_emmc_storage(storage_);
  return true;
}

bool Imx28Machine::load_emmc_file(const std::string& path) {
  if (!storage_->open_file(path)) return false;
  boot_emmc_image();
  return true;
}

bool Imx28Machine::load_emmc_multipart(const std::vector<std::string>& part_paths) {
  if (!storage_->open_multipart(part_paths)) return false;
  boot_emmc_image();
  return true;
}

void Imx28Machine::boot_emmc_image() {
  if (!storage_ || storage_->size_bytes() < 512) return;

  /* Read MBR (Sector 0) and load into RAM at 0x40000000 */
  uint8_t mbr[512] = {0};
  storage_->read_sector(0, mbr);
  cpu_.load_image(0x40000000u, mbr, 512);

  /* Read NK kernel image from offset 0x120000 (Sector 2304 = 0x900) into RAM at 0x40000200 */
  const uint64_t nk_sector_start = 0x120000 / 512;
  const uint32_t load_sectors = 8192;  /* Load first 4MB of NK kernel into RAM */
  uint8_t sec_buf[512];
  for (uint32_t i = 0; i < load_sectors; ++i) {
    if (storage_->read_sector(nk_sector_start + i, sec_buf)) {
      cpu_.load_image(0x40000200u + i * 512, sec_buf, 512);
    }
  }

  /* Set PC to DRAM entry point 0x40000000 (r15) */
  cpu_.regs[15] = 0x40000000u;
}

void Imx28Machine::reset() {
  cpu_.reset();
  boot_emmc_image();
}

uint64_t Imx28Machine::run(uint64_t insns) {
  const uint64_t start_insns = cpu_.stats.insns_interp + cpu_.stats.insns_jit;
  cpu_.run_until(insns, 0);
  return (cpu_.stats.insns_interp + cpu_.stats.insns_jit) - start_insns;
}

}  // namespace brain::machine::imx28
