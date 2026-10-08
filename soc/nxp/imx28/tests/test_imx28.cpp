/* brain-emu -- NXP i.MX28 SoC test suite
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */

#include <cassert>
#include <cstdio>
#include <cstring>
#include <memory>
#include <vector>

#include "soc/nxp/imx28/imx28_soc.h"
#include "testkit/test_support.h"

using namespace brain::soc::imx28;

static void test_icoll() {
  IcollDevice icoll(0x80000000u);
  icoll.reset();

  /* Enable IRQ 12 */
  icoll.write(0x120 + 12 * 0x10, 0x04, 4);  /* ENABLE bit */
  icoll.set_irq_line(12, 1);

  const uint32_t stat = icoll.read(0x30, 4);
  CHECK(stat == (0x80000000u | 12));
  CHECK(icoll.irq_level(0) == 1);

  icoll.set_irq_line(12, 0);
  CHECK(icoll.irq_level(0) == 0);
}

static void test_clkctrl_power() {
  ClkctrlDevice clk(0x80040000u);
  PowerDevice pwr(0x80044000u);

  clk.reset();
  pwr.reset();

  CHECK(clk.read(0x00, 4) == 0x00010000u);
  CHECK(pwr.read(0xB0, 4) == 0x00000008u);

  /* Test SET/CLR/TOG reg write */
  clk.write(0x04, 0x00000001u, 4);  /* SET PLL0CTRL0 bit 0 */
  CHECK(clk.read(0x00, 4) == 0x00010001u);
  clk.write(0x08, 0x00000001u, 4);  /* CLR PLL0CTRL0 bit 0 */
  CHECK(clk.read(0x00, 4) == 0x00010000u);
}

static void test_ssp_emmc() {
  auto storage = std::make_shared<EmmcStorage>();

  /* Create temporary multi-part dummy image */
  const char* p0 = "build/test_emmc_part0.img";
  const char* p1 = "build/test_emmc_part1.img";
  FILE* f0 = std::fopen(p0, "wb");
  FILE* f1 = std::fopen(p1, "wb");
  uint8_t dummy[512];
  std::memset(dummy, 0xAB, sizeof(dummy));
  std::fwrite(dummy, 1, 512, f0);
  std::memset(dummy, 0xCD, sizeof(dummy));
  std::fwrite(dummy, 1, 512, f1);
  std::fclose(f0);
  std::fclose(f1);

  CHECK(storage->open_multipart({p0, p1}));
  CHECK(storage->size_bytes() == 1024);
  CHECK(storage->sector_count() == 2);

  uint8_t s0[512], s1[512];
  CHECK(storage->read_sector(0, s0));
  CHECK(storage->read_sector(1, s1));
  CHECK(s0[0] == 0xAB);
  CHECK(s1[0] == 0xCD);

  SspDevice ssp(0x80010000u);
  ssp.reset();
  ssp.attach_storage(storage);

  /* Send CMD1 (eMMC SEND_OP_COND) */
  ssp.write(0x30, 0x00000000, 4);  /* CMD1 arg */
  ssp.write(0x20, 1, 4);           /* CMD1 index 1 */
  CHECK(ssp.read(0x80, 4) == 0x40FF8000u);  /* SDRESP0 */

  /* Send CMD17 (READ_SINGLE_BLOCK sector 0) */
  ssp.write(0x30, 0, 4);           /* Sector 0 */
  ssp.write(0x20, 17, 4);          /* CMD17 */
  const uint32_t first_word = ssp.read(0x70, 4);  /* Read from FIFO */
  CHECK((first_word & 0xFF) == 0xAB);
}

int main() {
  test_icoll();
  test_clkctrl_power();
  test_ssp_emmc();
  return brainemutest::summary("imx28");
}
