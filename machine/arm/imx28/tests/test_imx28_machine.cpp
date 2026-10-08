/* brain-emu -- i.MX28 Machine integration test
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */

#include <cstdio>
#include <cstring>

#include "machine/arm/imx28/imx28_machine.h"
#include "testkit/test_support.h"

using namespace brain::machine::imx28;

static void test_machine_init() {
  Imx28Machine mac;
  CHECK(mac.init(false, 64));

  /* Verify RAM identity window and ICOLL access via CPU bus */
  mac.cpu().phys_write(0x40000000u, 0x12345678u, 4);
  CHECK(mac.cpu().phys_read(0x40000000u, 4) == 0x12345678u);

  /* Verify ICOLL vector read via physical memory bus */
  CHECK(mac.cpu().phys_read(0x80000000u, 4) == 0x00000000u);
}

int main() {
  test_machine_init();
  return brainemutest::summary("imx28_machine");
}
