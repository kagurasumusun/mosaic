# Generic VM Infrastructure - Status, Gap Analysis, & Real OS Test Pipeline

## 1. Overview & Current Architecture

Mosaic is a modular virtual machine infrastructure in Rust (`framework/` and `catalog/`), strictly utilizing Unicorn Engine (`mosaic-engine-unicorn`) for CPU execution.

The repository follows a strict layer architecture enforced by `cargo xtask check`:
- `framework/`: VM core, contracts (`mmio`, `model`, `cpu-engine`), runtime, ISA layers (`arm`, `aarch64`, `x86-64`, `riscv`), `describe` (IR/schema), `assemble` (machine wiring).
- `catalog/`: Hardware IP models (`x86/pic-8259`, `x86/pit-8254`, `x86/rtc-mc146818`, `pci/pci-host`, `pci/pci-ide`, `virtio/virtio-blk`, `arm/gic-v2`, `arm/pl011`).
- `apps/`: Execution applications and test harnesses (`vm-runner`, `bringup`, `sensorbus`, `multicore`, `stm32f103`, `vic-thermal`).

---

## 2. Real OS Verification Status vs. Synthetic Smoke Harness

> **Note on Verification Status:**
> To maintain absolute honesty, we strictly distinguish between **Real OS Installation Executed**, **Synthetic Volume/Fixture Smoke Test**, **Unexecuted / Missing Firmware**, and **Environment Restricted**.

| OS / Target Category | OS Version / Distribution | Target Architecture | Verification Level | Detailed Status & Requirements |
| --- | --- | --- | --- | --- |
| **Linux (Real ISO)** | TinyCore Linux (`tinycore.iso`) | x86 / x86_64 | **Fixture Boot / Header Parsed** | ISO header & storage attach verified. Full guest OS kernel boot/install sequence requires real BIOS/UEFI firmware execution. |
| **Linux (Target Distros)** | Buildroot, Alpine, DSL, SliTaz, Puppy, Arch, Debian | x86_64 / AArch64 / ARM / RISC-V | **Synthetic ISO Fixture Test** | ISO Volume ID parsing & PCI/IDE/VirtIO storage detection verified via synthetic ISO fixtures. Real OS boot pending real firmware/kernel loading. |
| **Android** | Android 1.6 - 4.4, Android-x86 | x86 / ARM32 / ARM64 | **Synthetic ISO Fixture Test** | Storage attachment & Volume ID detection verified via synthetic fixtures. Real boot requires graphics/framebuffer & Android bootloader integration. |
| **Windows** | Win 95 / 98 / 2000 / XP / 2008 / WinPE / ReactOS / WinCE | x86 / x86_64 | **Synthetic ISO Fixture Test** | PCI Host, IDE Controller & 8259 PIC storage detection verified via synthetic volume descriptors. Real OS installer execution requires x86 BIOS/16-bit real mode execution support or SeaBIOS integration. |
| **RTOS** | FreeRTOS, Zephyr, NuttX | ARM Cortex-M / RISC-V | **Firmware Unit / Bringup Test** | ARM Cortex-M3 (T32) & RV32I firmware executed via `mosaic-apps-stm32f103` & `mosaic-apps-multicore`. |

---

## 3. Implemented Infrastructure Components

1. **CPU Engine Integration (`mosaic-engine-unicorn`)**:
   - Supported ISAs: ARM32 (A32), Cortex-M3 (T32), AArch64 (ARM 64-bit), x86_64, RISC-V (RV32I).
   - Capabilities: MMIO callback routing, register read/write, budget stepping, context save/restore, shared RAM coherence (DMI mirror synchronization).

2. **Peripherals & Buses**:
   - Interrupt Controllers: Intel 8259 PIC, ARM GICv2.
   - Timers & Clocks: Intel 8254 PIT, Motorola MC146818 RTC, ARM SP804.
   - Buses & Storage: PCI Host Bridge, PCI IDE Controller (with ISO9660 PVD parser), VirtIO-Block Controller.
   - Serial: ARM PL011 UART.

---

## 4. Work Required for Full Real OS Boot & Installation

To achieve full end-to-end OS boot, installation, and interaction (without synthetic shortcuts):

1. **Firmware / BIOS Integration**:
   - Integrate SeaBIOS / EDK2 (OVMF) or custom real-mode x86 / ARM firmware blob execution to initialize PC/Virt hardware and trigger MBR/El Torito bootloaders.
2. **Real ISO/Disk Download & Persistent Storage Pipeline**:
   - Fetch real OS installation media (e.g., Alpine Linux ISO, Debian NetInst, ReactOS ISO) and wire persistent disk image files for real partition/installation execution.
3. **Display & Input Devices**:
   - Implement VGA / VBE / VirtIO-GPU framebuffer device and PS/2 keyboard/mouse input to allow installer UI interaction.
4. **Interactive Console & Installer Output Logging**:
   - Route serial console (COM1 / UART) and framebuffer state capture to automated test harnesses to record real OS boot text logs and screenshots.

---

## 5. Automated Check Commands

```sh
# 1. Workspace architecture and lint check
cargo xtask check

# 2. Workspace unit and integration tests
cargo test --workspace --features unicorn,riscv

# 3. Generic VM runner test harness
cargo test -p mosaic-apps-vm-runner --features unicorn
```
