/* brain-emu -- NXP i.MX28 SoC implementation
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */

#include "soc/nxp/imx28/imx28_soc.h"

#include <algorithm>
#include <cstdio>
#include <cstring>

namespace brain::soc::imx28 {

/* ========================================================== EmmcStorage === */

bool EmmcStorage::open_file(const std::string& path) {
  close();
  FILE* fp = std::fopen(path.c_str(), "rb+");
  if (!fp) fp = std::fopen(path.c_str(), "rb");
  if (!fp) return false;

  std::fseek(fp, 0, SEEK_END);
  const long sz = std::ftell(fp);
  std::fseek(fp, 0, SEEK_SET);

  Part p;
  p.path = path;
  p.fp = fp;
  p.size = static_cast<uint64_t>(sz);
  parts_.push_back(p);
  total_size_ = p.size;
  return true;
}

bool EmmcStorage::open_multipart(const std::vector<std::string>& paths) {
  close();
  uint64_t cum_size = 0;
  for (const auto& path : paths) {
    FILE* fp = std::fopen(path.c_str(), "rb+");
    if (!fp) fp = std::fopen(path.c_str(), "rb");
    if (!fp) {
      close();
      return false;
    }
    std::fseek(fp, 0, SEEK_END);
    const long sz = std::ftell(fp);
    std::fseek(fp, 0, SEEK_SET);

    Part p;
    p.path = path;
    p.fp = fp;
    p.size = static_cast<uint64_t>(sz);
    parts_.push_back(p);
    cum_size += p.size;
  }
  total_size_ = cum_size;
  return !parts_.empty();
}

void EmmcStorage::close() {
  for (auto& p : parts_) {
    if (p.fp) {
      std::fclose(p.fp);
      p.fp = nullptr;
    }
  }
  parts_.clear();
  total_size_ = 0;
}

bool EmmcStorage::read_sector(uint64_t sector_idx, uint8_t* out_512) {
  uint64_t byte_off = sector_idx * 512;
  if (byte_off + 512 > total_size_) {
    std::memset(out_512, 0, 512);
    return false;
  }

  for (auto& p : parts_) {
    if (byte_off < p.size) {
      std::fseek(p.fp, static_cast<long>(byte_off), SEEK_SET);
      const size_t n = std::fread(out_512, 1, 512, p.fp);
      if (n < 512) std::memset(out_512 + n, 0, 512 - n);
      return true;
    }
    byte_off -= p.size;
  }
  return false;
}

bool EmmcStorage::write_sector(uint64_t sector_idx, const uint8_t* in_512) {
  uint64_t byte_off = sector_idx * 512;
  if (byte_off + 512 > total_size_) return false;

  for (auto& p : parts_) {
    if (byte_off < p.size) {
      std::fseek(p.fp, static_cast<long>(byte_off), SEEK_SET);
      return std::fwrite(in_512, 1, 512, p.fp) == 512;
    }
    byte_off -= p.size;
  }
  return false;
}

/* ========================================================== IcollDevice === */

IcollDevice::IcollDevice(uint32_t base) : base_(base) {}

void IcollDevice::reset() {
  vector_ = 0;
  stat_ = 0;
  ctrl_ = 0;
  std::memset(raw_status_, 0, sizeof(raw_status_));
  std::memset(enable_, 0, sizeof(enable_));
  std::memset(level_, 0, sizeof(level_));
  update_outputs();
}

void IcollDevice::set_irq_line(uint32_t irq_num, int level) {
  if (irq_num >= 128) return;
  const uint32_t idx = irq_num / 32;
  const uint32_t bit = 1u << (irq_num % 32);
  if (level) raw_status_[idx] |= bit;
  else raw_status_[idx] &= ~bit;
  update_outputs();
}

void IcollDevice::update_outputs() {
  bool pending = false;
  uint32_t highest_irq = 0;
  for (uint32_t i = 0; i < 4; ++i) {
    const uint32_t active = raw_status_[i] & enable_[i];
    if (active) {
      pending = true;
      for (uint32_t b = 0; b < 32; ++b) {
        if (active & (1u << b)) {
          highest_irq = i * 32 + b;
          break;
        }
      }
      break;
    }
  }
  if (pending) {
    vector_ = highest_irq;
    stat_ = 0x80000000u | highest_irq;
    drive_irq(0, 1);
  } else {
    stat_ = 0;
    drive_irq(0, 0);
  }
}

uint32_t IcollDevice::read(uint32_t offset, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) return vector_;
  if (reg == 0x10) return 0;  /* LEVELACK */
  if (reg == 0x20) return ctrl_;
  if (reg == 0x30) return stat_;
  if (reg >= 0xA0 && reg <= 0xD0) {
    const uint32_t idx = (reg - 0xA0) / 0x10;
    if (idx < 4) return raw_status_[idx];
  }
  if (reg >= 0x120 && reg < 0x920) {
    const uint32_t irq_num = (reg - 0x120) / 0x10;
    if (irq_num < 128) {
      const uint32_t idx = irq_num / 32;
      const uint32_t bit = 1u << (irq_num % 32);
      uint32_t val = 0;
      if (enable_[idx] & bit) val |= 0x00000004u;  /* ENABLE bit */
      if (raw_status_[idx] & bit) val |= 0x00000001u;  /* RAWSTATUS bit */
      return val;
    }
  }
  return 0;
}

void IcollDevice::write(uint32_t offset, uint32_t value, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) { reg_write_stc(&vector_, offset, value); }
  else if (reg == 0x20) { reg_write_stc(&ctrl_, offset, value); }
  else if (reg >= 0x120 && reg < 0x920) {
    const uint32_t irq_num = (reg - 0x120) / 0x10;
    if (irq_num < 128) {
      const uint32_t idx = irq_num / 32;
      const uint32_t bit = 1u << (irq_num % 32);
      if (value & 0x04u) enable_[idx] |= bit;
      else enable_[idx] &= ~bit;
    }
  }
  update_outputs();
}

/* ======================================================== ClkctrlDevice === */

ClkctrlDevice::ClkctrlDevice(uint32_t base) : base_(base) {}
void ClkctrlDevice::reset() {
  pll0ctrl0_ = 0x00010000u;
  cpu_ = 0x00000001u;
  hbus_ = 0x00000001u;
  xbus_ = 0x00000001u;
  xtal_ = 0x00000000u;
  ssp0_ = 0x00000001u;
}
uint32_t ClkctrlDevice::read(uint32_t offset, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) return pll0ctrl0_;
  if (reg == 0x20) return cpu_;
  if (reg == 0x30) return hbus_;
  if (reg == 0x40) return xbus_;
  if (reg == 0x50) return xtal_;
  if (reg == 0x100) return ssp0_;
  return 0;
}
void ClkctrlDevice::write(uint32_t offset, uint32_t value, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) reg_write_stc(&pll0ctrl0_, offset, value);
  else if (reg == 0x20) reg_write_stc(&cpu_, offset, value);
  else if (reg == 0x30) reg_write_stc(&hbus_, offset, value);
  else if (reg == 0x40) reg_write_stc(&xbus_, offset, value);
  else if (reg == 0x50) reg_write_stc(&xtal_, offset, value);
  else if (reg == 0x100) reg_write_stc(&ssp0_, offset, value);
}

/* ========================================================= PowerDevice === */

PowerDevice::PowerDevice(uint32_t base) : base_(base) {}
void PowerDevice::reset() {
  ctrl_ = 0;
  v5ctrl_ = 0;
  sts_ = 0x00000008u;  /* V5V_VALID */
}
uint32_t PowerDevice::read(uint32_t offset, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) return ctrl_;
  if (reg == 0x10) return v5ctrl_;
  if (reg == 0xB0) return sts_;
  return 0;
}
void PowerDevice::write(uint32_t offset, uint32_t value, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) reg_write_stc(&ctrl_, offset, value);
  else if (reg == 0x10) reg_write_stc(&v5ctrl_, offset, value);
}

/* ======================================================= PinctrlDevice === */

PinctrlDevice::PinctrlDevice(uint32_t base) : base_(base) {}
void PinctrlDevice::reset() {
  std::memset(din_, 0, sizeof(din_));
  std::memset(dout_, 0, sizeof(dout_));
  std::memset(doe_, 0, sizeof(doe_));
}
uint32_t PinctrlDevice::read(uint32_t offset, uint32_t size) {
  (void)size;
  if (offset >= 0x600 && offset <= 0x640) {
    const uint32_t bank = (offset - 0x600) / 0x10;
    if (bank < 5) return dout_[bank];
  }
  if (offset >= 0x700 && offset <= 0x740) {
    const uint32_t bank = (offset - 0x700) / 0x10;
    if (bank < 5) return din_[bank];
  }
  if (offset >= 0x900 && offset <= 0x940) {
    const uint32_t bank = (offset - 0x900) / 0x10;
    if (bank < 5) return doe_[bank];
  }
  return 0;
}
void PinctrlDevice::write(uint32_t offset, uint32_t value, uint32_t size) {
  (void)size;
  if (offset >= 0x600 && offset <= 0x640) {
    const uint32_t bank = (offset - 0x600) / 0x10;
    if (bank < 5) reg_write_stc(&dout_[bank], offset, value);
  } else if (offset >= 0x900 && offset <= 0x940) {
    const uint32_t bank = (offset - 0x900) / 0x10;
    if (bank < 5) reg_write_stc(&doe_[bank], offset, value);
  }
}

/* ======================================================= DigctlDevice === */

DigctlDevice::DigctlDevice(uint32_t base) : base_(base) {}
void DigctlDevice::reset() {
  ctrl_ = 0;
  microsec_ = 0;
  chip_id_ = 0x28000000u;
}
void DigctlDevice::tick(uint64_t cycles) {
  /* 1 microsecond per 24 cycles (assuming 24 MHz XTAL) */
  microsec_ += cycles / 24;
}
uint32_t DigctlDevice::read(uint32_t offset, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) return ctrl_;
  if (reg == 0xC0) return static_cast<uint32_t>(microsec_);
  if (reg == 0x310) return chip_id_;
  return 0;
}
void DigctlDevice::write(uint32_t offset, uint32_t value, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) reg_write_stc(&ctrl_, offset, value);
}

/* ======================================================= TimrotDevice === */

TimrotDevice::TimrotDevice(uint32_t base) : base_(base) {}
void TimrotDevice::reset() {
  for (int i = 0; i < 4; ++i) {
    tmr_[i].ctrl = 0;
    tmr_[i].count = 0;
    tmr_[i].fixed_count = 0;
  }
}
void TimrotDevice::tick(uint64_t cycles) {
  for (int i = 0; i < 4; ++i) {
    if (tmr_[i].ctrl & 0x4000u) {  /* RELOAD bit */
      if (tmr_[i].count <= cycles) {
        tmr_[i].count = tmr_[i].fixed_count;
        drive_irq(i, 1);
      } else {
        tmr_[i].count -= static_cast<uint32_t>(cycles);
      }
    }
  }
}
uint32_t TimrotDevice::read(uint32_t offset, uint32_t size) {
  (void)size;
  if (offset >= 0x20 && offset <= 0x80) {
    const uint32_t idx = (offset - 0x20) / 0x20;
    if (idx < 4) return tmr_[idx].ctrl;
  }
  if (offset >= 0x30 && offset <= 0x90) {
    const uint32_t idx = (offset - 0x30) / 0x20;
    if (idx < 4) return tmr_[idx].count;
  }
  return 0;
}
void TimrotDevice::write(uint32_t offset, uint32_t value, uint32_t size) {
  (void)size;
  if (offset >= 0x20 && offset <= 0x80) {
    const uint32_t idx = (offset - 0x20) / 0x20;
    if (idx < 4) reg_write_stc(&tmr_[idx].ctrl, offset, value);
  } else if (offset >= 0x30 && offset <= 0x90) {
    const uint32_t idx = (offset - 0x30) / 0x20;
    if (idx < 4) {
      tmr_[idx].fixed_count = value;
      tmr_[idx].count = value;
    }
  }
}

/* ========================================================= DuartDevice === */

DuartDevice::DuartDevice(uint32_t base) : base_(base) {}
void DuartDevice::reset() {
  dr_ = 0;
  fr_ = 0x90;
  cr_ = 0x301;
}
uint32_t DuartDevice::read(uint32_t offset, uint32_t size) {
  (void)size;
  if (offset == 0x00) return dr_;
  if (offset == 0x18) return fr_;
  if (offset == 0x30) return cr_;
  return 0;
}
void DuartDevice::write(uint32_t offset, uint32_t value, uint32_t size) {
  (void)size;
  if (offset == 0x00) {
    dr_ = value & 0xFFu;
    std::putchar(static_cast<char>(dr_));
    std::fflush(stdout);
  } else if (offset == 0x30) {
    cr_ = value;
  }
}

/* =========================================================== SspDevice === */

SspDevice::SspDevice(uint32_t base) : base_(base) {}

void SspDevice::reset() {
  ctrl0_ = 0x80000000u;
  ctrl1_ = 0;
  cmd0_ = 0;
  cmd1_ = 0;
  status_ = 0x00000020u;  /* FIFO_EMPTY */
  sdresp0_ = sdresp1_ = sdresp2_ = sdresp3_ = 0;
  rca_ = 0x00010000u;
  block_len_ = 512;
  buf_pos_ = 0;
  std::memset(sector_buf_, 0, sizeof(sector_buf_));
}

void SspDevice::handle_command(uint32_t cmd_val, uint32_t arg) {
  const uint32_t cmdidx = cmd_val & 0x3Fu;
  sdresp0_ = sdresp1_ = sdresp2_ = sdresp3_ = 0;

  switch (cmdidx) {
    case 0:   /* GO_IDLE_STATE */
      break;
    case 1:   /* SEND_OP_COND (eMMC OCR) */
      sdresp0_ = 0x40FF8000u;  /* Card ready / High capacity */
      break;
    case 2:   /* ALL_SEND_CID */
      sdresp0_ = 0x15010042u;
      sdresp1_ = 0x50573220u;
      sdresp2_ = 0x00000001u;
      sdresp3_ = 0x00000000u;
      break;
    case 3:   /* SET_RELATIVE_ADDR */
      sdresp0_ = rca_;
      break;
    case 7:   /* SELECT_CARD */
      sdresp0_ = 0x00000700u;  /* Card status: TRAN state */
      break;
    case 8:   /* SEND_IF_COND */
      sdresp0_ = arg;          /* Voltage echo pattern */
      break;
    case 9:   /* SEND_CSD */
      sdresp0_ = 0x400E0032u;  /* Standard CSD */
      sdresp1_ = 0x5B5A83B9u;
      sdresp2_ = 0x00000000u;
      sdresp3_ = 0x00000000u;
      break;
    case 12:  /* STOP_TRANSMISSION */
      sdresp0_ = 0x00000B00u;
      break;
    case 13:  /* SEND_STATUS */
      sdresp0_ = 0x00000900u;  /* TRAN state, ready for data */
      break;
    case 16:  /* SET_BLOCKLEN */
      block_len_ = arg ? arg : 512;
      sdresp0_ = 0x00000900u;
      break;
    case 17:  /* READ_SINGLE_BLOCK */
    case 18:  /* READ_MULTIPLE_BLOCK */
      sdresp0_ = 0x00000900u;
      if (storage_) {
        const uint64_t sector = (arg > 0x100000u) ? (arg / 512) : arg;
        storage_->read_sector(sector, sector_buf_);
      }
      buf_pos_ = 0;
      status_ &= ~0x00000020u;  /* FIFO not empty */
      break;
    case 24:  /* WRITE_BLOCK */
    case 25:  /* WRITE_MULTIPLE_BLOCK */
      sdresp0_ = 0x00000900u;
      break;
    case 55:  /* APP_CMD */
      sdresp0_ = 0x00000020u;
      break;
    default:
      sdresp0_ = 0x00000900u;
      break;
  }
}

uint32_t SspDevice::read(uint32_t offset, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) return ctrl0_;
  if (reg == 0x10) return ctrl1_;
  if (reg == 0x20) return cmd0_;
  if (reg == 0x30) return cmd1_;
  if (reg == 0x70) {  /* DATA FIFO */
    if (buf_pos_ < sizeof(sector_buf_)) {
      uint32_t word = 0;
      std::memcpy(&word, sector_buf_ + buf_pos_, 4);
      buf_pos_ += 4;
      if (buf_pos_ >= sizeof(sector_buf_)) {
        status_ |= 0x00000020u;  /* FIFO empty */
      }
      return word;
    }
    return 0;
  }
  if (reg == 0x80) return sdresp0_;
  if (reg == 0x90) return sdresp1_;
  if (reg == 0xA0) return sdresp2_;
  if (reg == 0xB0) return sdresp3_;
  if (reg == 0x100) return status_;
  return 0;
}

void SspDevice::write(uint32_t offset, uint32_t value, uint32_t size) {
  (void)size;
  const uint32_t reg = offset & ~0x0Fu;
  if (reg == 0x00) { reg_write_stc(&ctrl0_, offset, value); }
  else if (reg == 0x10) { reg_write_stc(&ctrl1_, offset, value); }
  else if (reg == 0x20) {
    reg_write_stc(&cmd0_, offset, value);
    handle_command(cmd0_ & 0xFFu, cmd1_);
  }
  else if (reg == 0x30) { reg_write_stc(&cmd1_, offset, value); }
  else if (reg == 0x70) {
    if (buf_pos_ < sizeof(sector_buf_)) {
      std::memcpy(sector_buf_ + buf_pos_, &value, 4);
      buf_pos_ += 4;
    }
  }
}

/* ========================================================== Imx28SoC ====== */

Imx28SoC::Imx28SoC() = default;

bool Imx28SoC::attach_to_bus(Bus& bus) {
  if (!icoll_.attach(bus)) return false;
  if (!pinctrl_.attach(bus)) return false;
  if (!digctl_.attach(bus)) return false;
  if (!clkctrl_.attach(bus)) return false;
  if (!power_.attach(bus)) return false;
  if (!timrot_.attach(bus)) return false;
  if (!duart_.attach(bus)) return false;
  if (!ssp0_.attach(bus)) return false;
  return true;
}

bool Imx28SoC::attach_emmc_storage(std::shared_ptr<EmmcStorage> storage) {
  ssp0_.attach_storage(storage);
  return true;
}

}  // namespace brain::soc::imx28
