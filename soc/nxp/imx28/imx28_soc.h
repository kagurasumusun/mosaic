/* brain-emu -- NXP i.MX28 SoC integration
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Integrates the i.MX28 SoC peripherals around an ARM926EJ-S CPU core:
 *   - ICOLL: Interrupt Collector (128 channels, vector/stat, IRQ/FIQ outputs)
 *   - CLKCTRL: Clock Control (PLL, CPU/bus clocks, peripheral gating)
 *   - POWER: Power Management
 *   - PINCTRL: Pin Multiplexing and GPIOs
 *   - DIGCTL: Digital Control (microsecond timer, chip ID)
 *   - TIMROT: 32-bit Timers (Rotary & Timers 0..3)
 *   - DUART / AUART: Debug and Application Serial Interfaces
 *   - SSP: Synchronous Serial Port (SD/eMMC controller with block image streaming)
 */

#ifndef BRAINEMU_SOC_NXP_IMX28_SOC_H
#define BRAINEMU_SOC_NXP_IMX28_SOC_H

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include "brainemu/bus.h"
#include "brainemu/device.h"
#include "brainemu/module.h"

namespace brain::soc::imx28 {

/* Common helper for i.MX28 SET/CLR/TOG register writing rules:
 * Reg + 0x0: Write value directly
 * Reg + 0x4: Set bits (Reg |= val)
 * Reg + 0x8: Clear bits (Reg &= ~val)
 * Reg + 0xC: Toggle bits (Reg ^= val)
 */
inline void reg_write_stc(uint32_t* reg, uint32_t offset, uint32_t val) {
  const uint32_t op = offset & 0x0Fu;
  if (op == 0x0) *reg = val;
  else if (op == 0x4) *reg |= val;
  else if (op == 0x8) *reg &= ~val;
  else if (op == 0xC) *reg ^= val;
}

/* ---- eMMC Image Backing Store ------------------------------------------ */

class EmmcStorage {
 public:
  EmmcStorage() = default;
  ~EmmcStorage() { close(); }

  bool open_file(const std::string& path);
  bool open_multipart(const std::vector<std::string>& paths);
  void close();

  uint64_t size_bytes() const { return total_size_; }
  uint32_t sector_count() const { return static_cast<uint32_t>(total_size_ / 512); }

  bool read_sector(uint64_t sector_idx, uint8_t* out_512);
  bool write_sector(uint64_t sector_idx, const uint8_t* in_512);

 private:
  struct Part {
    std::string path;
    FILE* fp = nullptr;
    uint64_t size = 0;
  };
  std::vector<Part> parts_;
  uint64_t total_size_ = 0;
};

/* ---- ICOLL: Interrupt Collector --------------------------------------- */

class IcollDevice final : public Device {
 public:
  explicit IcollDevice(uint32_t base = 0x80000000u);
  void reset() override;
  uint32_t read(uint32_t offset, uint32_t size) override;
  void write(uint32_t offset, uint32_t value, uint32_t size) override;

  const ModuleInfo& info() const override { return info_; }
  uint32_t base() const override { return base_; }
  uint32_t size() const override { return 0x2000u; }

  void set_irq_line(uint32_t irq_num, int level);
  void update_outputs();

 private:
  ModuleInfo info_{"imx28_icoll", "i.MX28 ICOLL", ModuleKind::Device, "Interrupt Collector"};
  uint32_t base_;
  uint32_t vector_ = 0;
  uint32_t stat_ = 0;
  uint32_t ctrl_ = 0;
  uint32_t raw_status_[4] = {0};   /* 128 IRQ channels */
  uint32_t enable_[4] = {0};
  uint32_t level_[4] = {0};
};

/* ---- CLKCTRL: Clock Control ------------------------------------------- */

class ClkctrlDevice final : public Device {
 public:
  explicit ClkctrlDevice(uint32_t base = 0x80040000u);
  void reset() override;
  uint32_t read(uint32_t offset, uint32_t size) override;
  void write(uint32_t offset, uint32_t value, uint32_t size) override;

  const ModuleInfo& info() const override { return info_; }
  uint32_t base() const override { return base_; }
  uint32_t size() const override { return 0x2000u; }

 private:
  ModuleInfo info_{"imx28_clkctrl", "i.MX28 CLKCTRL", ModuleKind::Device, "Clock Control"};
  uint32_t base_;
  uint32_t pll0ctrl0_ = 0x00010000u;
  uint32_t cpu_ = 0x00000001u;
  uint32_t hbus_ = 0x00000001u;
  uint32_t xbus_ = 0x00000001u;
  uint32_t xtal_ = 0x00000000u;
  uint32_t ssp0_ = 0x00000001u;
};

/* ---- POWER: Power Management ----------------------------------------- */

class PowerDevice final : public Device {
 public:
  explicit PowerDevice(uint32_t base = 0x80044000u);
  void reset() override;
  uint32_t read(uint32_t offset, uint32_t size) override;
  void write(uint32_t offset, uint32_t value, uint32_t size) override;

  const ModuleInfo& info() const override { return info_; }
  uint32_t base() const override { return base_; }
  uint32_t size() const override { return 0x2000u; }

 private:
  ModuleInfo info_{"imx28_power", "i.MX28 POWER", ModuleKind::Device, "Power Management"};
  uint32_t base_;
  uint32_t ctrl_ = 0x00000000u;
  uint32_t v5ctrl_ = 0x00000000u;
  uint32_t sts_ = 0x00000008u;   /* V5V_VALID */
};

/* ---- PINCTRL: Pin Control & GPIO -------------------------------------- */

class PinctrlDevice final : public Device {
 public:
  explicit PinctrlDevice(uint32_t base = 0x80018000u);
  void reset() override;
  uint32_t read(uint32_t offset, uint32_t size) override;
  void write(uint32_t offset, uint32_t value, uint32_t size) override;

  const ModuleInfo& info() const override { return info_; }
  uint32_t base() const override { return base_; }
  uint32_t size() const override { return 0x4000u; }

 private:
  ModuleInfo info_{"imx28_pinctrl", "i.MX28 PINCTRL", ModuleKind::Device, "Pin Multiplexing & GPIO"};
  uint32_t base_;
  uint32_t din_[5] = {0};
  uint32_t dout_[5] = {0};
  uint32_t doe_[5] = {0};
};

/* ---- DIGCTL: Digital Control ------------------------------------------ */

class DigctlDevice final : public Device {
 public:
  explicit DigctlDevice(uint32_t base = 0x8001C000u);
  void reset() override;
  uint32_t read(uint32_t offset, uint32_t size) override;
  void write(uint32_t offset, uint32_t value, uint32_t size) override;
  void tick(uint64_t cycles) override;

  const ModuleInfo& info() const override { return info_; }
  uint32_t base() const override { return base_; }
  uint32_t size() const override { return 0x2000u; }

 private:
  ModuleInfo info_{"imx28_digctl", "i.MX28 DIGCTL", ModuleKind::Device, "Digital Control & System Timer"};
  uint32_t base_;
  uint32_t ctrl_ = 0;
  uint64_t microsec_ = 0;
  uint32_t chip_id_ = 0x28000000u;   /* i.MX28 Chip ID */
};

/* ---- TIMROT: Timers ---------------------------------------------------- */

class TimrotDevice final : public Device {
 public:
  explicit TimrotDevice(uint32_t base = 0x80068000u);
  void reset() override;
  uint32_t read(uint32_t offset, uint32_t size) override;
  void write(uint32_t offset, uint32_t value, uint32_t size) override;
  void tick(uint64_t cycles) override;

  const ModuleInfo& info() const override { return info_; }
  uint32_t base() const override { return base_; }
  uint32_t size() const override { return 0x2000u; }

 private:
  ModuleInfo info_{"imx28_timrot", "i.MX28 TIMROT", ModuleKind::Device, "32-bit Timers"};
  uint32_t base_;
  struct Timer {
    uint32_t ctrl = 0;
    uint32_t count = 0;
    uint32_t fixed_count = 0;
  } tmr_[4];
};

/* ---- DUART / Serial --------------------------------------------------- */

class DuartDevice final : public Device {
 public:
  explicit DuartDevice(uint32_t base = 0x80070000u);
  void reset() override;
  uint32_t read(uint32_t offset, uint32_t size) override;
  void write(uint32_t offset, uint32_t value, uint32_t size) override;

  const ModuleInfo& info() const override { return info_; }
  uint32_t base() const override { return base_; }
  uint32_t size() const override { return 0x2000u; }

 private:
  ModuleInfo info_{"imx28_duart", "i.MX28 DUART", ModuleKind::Device, "Debug UART"};
  uint32_t base_;
  uint32_t dr_ = 0;
  uint32_t fr_ = 0x90;  /* TXFE | RXFE */
  uint32_t cr_ = 0x301;
};

/* ---- SSP: SD/eMMC Controller ------------------------------------------ */

class SspDevice final : public Device {
 public:
  explicit SspDevice(uint32_t base = 0x80010000u);
  ~SspDevice() override = default;

  void reset() override;
  uint32_t read(uint32_t offset, uint32_t size) override;
  void write(uint32_t offset, uint32_t value, uint32_t size) override;

  const ModuleInfo& info() const override { return info_; }
  uint32_t base() const override { return base_; }
  uint32_t size() const override { return 0x2000u; }

  void attach_storage(std::shared_ptr<EmmcStorage> storage) { storage_ = storage; }

 private:
  void handle_command(uint32_t cmd_val, uint32_t arg);

  ModuleInfo info_{"imx28_ssp", "i.MX28 SSP", ModuleKind::Device, "SSP eMMC/SD Controller"};
  uint32_t base_;
  uint32_t ctrl0_ = 0x80000000u;
  uint32_t ctrl1_ = 0;
  uint32_t cmd0_ = 0;
  uint32_t cmd1_ = 0;
  uint32_t status_ = 0x00000020u;   /* BUSY=0, FIFO_EMPTY=1 */
  uint32_t sdresp0_ = 0, sdresp1_ = 0, sdresp2_ = 0, sdresp3_ = 0;

  std::shared_ptr<EmmcStorage> storage_;
  uint32_t rca_ = 0x00010000u;
  uint32_t block_len_ = 512;
  uint8_t sector_buf_[512] = {0};
  uint32_t buf_pos_ = 0;
};

/* ---- Full i.MX28 SoC Assembly ---------------------------------------- */

class Imx28SoC {
 public:
  Imx28SoC();
  ~Imx28SoC() = default;

  bool attach_to_bus(Bus& bus);
  bool attach_emmc_storage(std::shared_ptr<EmmcStorage> storage);

  IcollDevice& icoll() { return icoll_; }
  SspDevice& ssp0() { return ssp0_; }

 private:
  IcollDevice icoll_{0x80000000u};
  PinctrlDevice pinctrl_{0x80018000u};
  DigctlDevice digctl_{0x8001C000u};
  ClkctrlDevice clkctrl_{0x80040000u};
  PowerDevice power_{0x80044000u};
  TimrotDevice timrot_{0x80068000u};
  DuartDevice duart_{0x80070000u};
  SspDevice ssp0_{0x80010000u};
};

}  // namespace brain::soc::imx28

#endif  /* BRAINEMU_SOC_NXP_IMX28_SOC_H */
