# Guest OS boot and installation validation plan

この文書は、汎用 VM の受け入れ基準を定義します。CPU 命令の smoke test、ISO の解析、ファームウェア経由のブート、OS の操作は別の能力です。下位段階の成功を上位段階の成功として報告してはいけません。

## Capability levels

| Level | Name | Required evidence |
| --- | --- | --- |
| G0 | Image intake | Source URL, upstream version, license/redistribution note, byte length, SHA-256 |
| G1 | Image recognition | Format and boot metadata parsed without out-of-bounds access; malformed images rejected |
| G2 | Device discovery | Guest firmware/bootloader can enumerate the configured CPU, RAM, storage, display, and interrupt controllers |
| G3 | Bootloader entry | Actual guest boot code executes from the image's documented entry path; no synthetic NOP loop |
| G4 | Kernel / OS boot | A stable serial or display milestone proves the kernel or OS loader progressed |
| G5 | Interactive operation | Keyboard/console input produces a guest-observable response; storage and reset behavior are checked |
| G6 | Installation | Installer writes a virtual disk, reboot from that disk succeeds, and the installed system reaches G5 |

Each test result records the highest level actually observed. A parsed ISO volume label is G1 evidence only.

## Test record format

Every image test should record:

- guest family and exact release/build;
- guest architecture and machine profile;
- upstream source and retrieval date;
- SHA-256, image format, size, and license / redistribution constraints;
- firmware type and version (BIOS, UEFI, or platform-specific boot ROM);
- virtual CPU, RAM, storage controller, display, and interrupt-controller configuration;
- expected and observed serial markers, display checkpoints, and input/output actions;
- reached capability level, failure category, logs, and reproducible command line.

Do not commit proprietary or redistribution-restricted OS images. CI should download only legally redistributable test images, verify their pinned SHA-256, cache them outside version control, and fail closed on a digest mismatch. Never silently replace an image when an upstream URL changes.

## Initial implementation sequence

### V0: make the runner honest and testable

- Keep the current synthetic x86-64 and AArch64 instruction smoke tests as engine tests, not OS boot tests.
- Add explicit image input and machine-profile arguments instead of manufacturing a successful report when no guest has run.
- Report image metadata separately from guest execution milestones.
- Return an unsupported-capability error when a required device or boot path is not implemented.
- Add negative tests for truncated images, invalid signatures, unsupported image formats, and address-range overflow.

### V1: x86 PC firmware and boot-media path

- Specify a PC memory map and I/O-port model. The current generic PC layout places devices in an MMIO window; this is not equivalent to legacy x86 port I/O and must not be presented as a PC-compatible chipset.
- Implement a documented firmware path (initially a minimal open-source BIOS implementation or an explicitly scoped test firmware) that can load a boot sector from a virtual disk.
- Implement the required x86 reset state, port I/O, PIC/PIT/RTC behavior, PCI configuration access, and an actual interrupt delivery strategy.
- Support ISO9660 boot catalog / El Torito metadata and a bootable virtual disk path; parsing a Primary Volume Descriptor alone does not boot an ISO.
- Validate with a tiny purpose-built boot image before trying general-purpose distributions.

### V2: reproducible Linux guests

Start with one x86-64 profile and a small, legally redistributable guest image. Require serial evidence and an interactive command before expanding the matrix. Add Buildroot, Alpine, Tiny Core, SliTaz, DSL, Puppy, Arch Linux, and Debian only as independently versioned test cases.

### V3: expand machine and ISA coverage

Add profiles only when the engine ISA mode, reset/exception behavior, memory map, firmware, interrupt routing, and required peripherals have explicit tests. Do not infer that a CPU engine's support for an ISA makes every OS or board using that ISA bootable.

### V4: historical and specialized systems

Android 1.6–4.4, Windows 95/98/2000/XP/Server 2008/WinPE, ReactOS, Windows CE, Windows Validation OS, QTest, QEMU Functional Tests, and RTOS targets each require their own image provenance, firmware, machine profile, and acceptance markers. Keep unavailable, proprietary, or legally restricted images out of CI; use user-provided images locally when permitted.

## Architecture coverage

Track support as a matrix of **guest × CPU architecture × machine profile × firmware × capability level**. A family name alone is not a support claim. For every CPU architecture, require at least:

1. register and instruction smoke tests through Unicorn;
2. memory permission and MMIO callback tests;
3. reset/exception and interrupt delivery tests;
4. a small firmware/boot-image test;
5. a real guest image reaching a recorded G4 or higher milestone.

Unicorn adapters must remain the CPU execution backend; Mosaic's own implementation should focus on machine models, platform semantics, device models, firmware/image loading, orchestration, and tests.

## Current known gap

The current PC runner reports that IDE and VirtIO storage are ready even though it does not establish that a guest booted from either device. Its ISO handling extracts an ISO9660 volume identifier and attaches the bytes as an IDE drive, but it does not implement El Torito boot selection, BIOS execution, x86 port I/O, or an interactive OS boot path. These booleans must not be interpreted as OS compatibility evidence. Replace them with explicit device-attachment and guest-progress fields before publishing compatibility results.
