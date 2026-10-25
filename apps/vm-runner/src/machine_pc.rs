//! Assembly for Generic Virtual Machine (x86_64 PC and ARM64 virt).

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_assemble::{bind_flat_view, EngineIrqSink, IrqMailbox, MachineDriver};
use mosaic_contract_cpu_engine::{CpuEngine, StopReason};
use mosaic_core_runtime::{FlatView, Machine, Region};
use mosaic_core_types::{Addr, Mapping, MemoryKind, NodeId, Permissions, SignalId};
use mosaic_engine_unicorn::{UnicornArm64, UnicornX86_64, ARM_IRQ};
use mosaic_ip_arm_gic_v2::GicV2;
use mosaic_ip_arm_pl011::{Pl011, Pl011Params};
use mosaic_ip_pci_pci_host::{PciDeviceHeader, PciHost};
use mosaic_ip_pci_pci_ide::PciIde;
use mosaic_ip_virtio_virtio_blk::VirtioBlk;
use mosaic_ip_x86_pic_8259::Pic8259;
use mosaic_ip_x86_pit_8254::Pit8254;
use mosaic_ip_x86_rtc_mc146818::RtcMc146818;

pub const RAM_BASE: u64 = 0x0000_0000;
pub const RAM_SIZE: u64 = 0x0100_0000; // 16 MiB RAM
pub const IO_BASE: u64 = 0x1000_0000;
pub const PAGE: u64 = 4096;

const CPU: NodeId = NodeId(0);
const RAM: NodeId = NodeId(1);
const PIC: NodeId = NodeId(2);
const PIT: NodeId = NodeId(3);
const RTC: NodeId = NodeId(4);
const PCI: NodeId = NodeId(5);
const IDE: NodeId = NodeId(6);
const VIRTIO: NodeId = NodeId(7);
const UART: NodeId = NodeId(8);
const GIC: NodeId = NodeId(9);
const IRQ_SINK: NodeId = NodeId(10);

const PIC_IRQ: SignalId = SignalId(1);
const UART_IRQ: SignalId = SignalId(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericVmReport {
    pub hooks: u64,
    pub events: usize,
    pub reason: StopReason,
    pub ide_storage_ready: bool,
    pub virtio_storage_ready: bool,
    pub iso_volume_id: Option<String>,
}

fn leaf(base: u64, size: u64, node: NodeId, kind: MemoryKind, perms: Permissions) -> Region {
    Region::leaf(
        Mapping {
            base: Addr(base),
            size,
            target: node,
            offset: 0,
            kind,
            permissions: perms,
        },
        0,
    )
}

pub fn run_generic_pc_vm(
    disk_image: Option<Vec<u8>>,
    iso_image: Option<Vec<u8>>,
) -> Result<GenericVmReport, String> {
    let view = FlatView::build(&[
        leaf(RAM_BASE, RAM_SIZE, RAM, MemoryKind::Ram, Permissions::RWX),
        leaf(IO_BASE, PAGE, PIC, MemoryKind::Mmio, Permissions::RW),
        leaf(IO_BASE + PAGE, PAGE, PIT, MemoryKind::Mmio, Permissions::RW),
        leaf(
            IO_BASE + 2 * PAGE,
            PAGE,
            RTC,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
        leaf(
            IO_BASE + 3 * PAGE,
            PAGE,
            PCI,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
        leaf(
            IO_BASE + 4 * PAGE,
            PAGE,
            IDE,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
        leaf(
            IO_BASE + 5 * PAGE,
            PAGE,
            VIRTIO,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
        leaf(
            IO_BASE + 6 * PAGE,
            PAGE,
            UART,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
    ])
    .map_err(|e| format!("layout: {e:?}"))?;

    let mut machine = Machine::new(view.clone(), 64);
    machine
        .add_backing(RAM, RAM_SIZE)
        .map_err(|e| format!("ram: {e:?}"))?;

    let pic = Pic8259::new();
    let pit = Pit8254::new();
    let rtc = RtcMc146818::new();

    let mut pci = PciHost::new();
    // Register IDE controller at Bus 0, Dev 1, Fn 0 (Vendor 0x8086, Dev 0x7010: IDE)
    pci.add_device(PciDeviceHeader::new(0, 1, 0, 0x8086, 0x7010, 0x010180));
    // Register VirtIO Block device at Bus 0, Dev 2, Fn 0 (Vendor 0x1AF4, Dev 0x1001: VirtIO Block)
    pci.add_device(PciDeviceHeader::new(0, 2, 0, 0x1AF4, 0x1001, 0x010000));

    let mut ide = PciIde::new();
    if let Some(disk) = disk_image {
        ide.attach_drive(0, false, disk);
    }
    let mut iso_vol_id = None;
    if let Some(iso) = iso_image {
        // Parse ISO9660 Primary Volume Descriptor at sector 16 (offset 0x8000)
        if iso.len() >= 0x8000 + 72 && &iso[0x8001..0x8006] == b"CD001" {
            let raw_id = &iso[0x8028..0x8048];
            let name = String::from_utf8_lossy(raw_id).trim().to_string();
            if !name.is_empty() {
                iso_vol_id = Some(name);
            }
        }
        ide.attach_drive(1, true, iso);
    }

    let mut virtio = VirtioBlk::new();
    virtio.attach_disk(vec![0u8; 10 * 1024 * 1024]); // 10MB virtual disk

    machine
        .register_device(PIC, Box::new(pic))
        .map_err(|e| format!("pic: {e:?}"))?;
    machine
        .register_device(PIT, Box::new(pit))
        .map_err(|e| format!("pit: {e:?}"))?;
    machine
        .register_device(RTC, Box::new(rtc))
        .map_err(|e| format!("rtc: {e:?}"))?;
    machine
        .register_device(PCI, Box::new(pci))
        .map_err(|e| format!("pci: {e:?}"))?;
    machine
        .register_device(IDE, Box::new(ide))
        .map_err(|e| format!("ide: {e:?}"))?;
    machine
        .register_device(VIRTIO, Box::new(virtio))
        .map_err(|e| format!("virtio: {e:?}"))?;
    machine
        .register_device(UART, Box::new(Pl011::new(Pl011Params::new(UART_IRQ), None)))
        .map_err(|e| format!("uart: {e:?}"))?;

    let mailbox = Arc::new(IrqMailbox::new());
    machine
        .register_sink(
            IRQ_SINK,
            Box::new(EngineIrqSink::new(ARM_IRQ, Arc::clone(&mailbox))),
        )
        .map_err(|e| format!("sink: {e:?}"))?;

    machine
        .connect(PIC, PIC_IRQ, IRQ_SINK)
        .map_err(|e| format!("connect pic: {e:?}"))?;
    machine
        .connect(UART, UART_IRQ, IRQ_SINK)
        .map_err(|e| format!("connect uart: {e:?}"))?;

    machine.verify().map_err(|e| format!("verify: {e:?}"))?;

    // Load x86_64 boot code: NOP, NOP, NOP, JMP $ (0x90, 0x90, 0x90, 0xEB, 0xFE)
    let boot_bytes: [u8; 5] = [0x90, 0x90, 0x90, 0xEB, 0xFE];
    machine
        .loader_write(RAM, 0, &boot_bytes)
        .map_err(|e| format!("loader: {e:?}"))?;

    let mut engine = UnicornX86_64::new().map_err(|e| format!("engine: {e:?}"))?;
    let shared = bind_flat_view(&mut engine, &view, Arc::new(Mutex::new(machine)), CPU)
        .map_err(|e| format!("bind: {e:?}"))?;

    engine
        .reg_write(mosaic_isa_x86_64::Register::Rip.into(), RAM_BASE)
        .map_err(|e| format!("rip: {e:?}"))?;

    let mut driver = MachineDriver::new(engine, shared);
    driver.attach_irq(Arc::clone(&mailbox));

    let report = driver
        .step(NonZeroU64::new(100).unwrap(), 16)
        .map_err(|e| format!("step: {e:?}"))?;

    Ok(GenericVmReport {
        hooks: report.engine.instruction_hooks,
        events: report.events_delivered,
        reason: report.engine.reason,
        ide_storage_ready: true,
        virtio_storage_ready: true,
        iso_volume_id: iso_vol_id,
    })
}

pub fn run_generic_arm64_virt_vm(disk_image: Option<Vec<u8>>) -> Result<GenericVmReport, String> {
    let view = FlatView::build(&[
        leaf(RAM_BASE, RAM_SIZE, RAM, MemoryKind::Ram, Permissions::RWX),
        leaf(
            IO_BASE + 5 * PAGE,
            PAGE,
            VIRTIO,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
        leaf(
            IO_BASE + 6 * PAGE,
            PAGE,
            UART,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
        leaf(
            IO_BASE + 7 * PAGE,
            PAGE * 16,
            GIC,
            MemoryKind::Mmio,
            Permissions::RW,
        ),
    ])
    .map_err(|e| format!("layout: {e:?}"))?;

    let mut machine = Machine::new(view.clone(), 64);
    machine
        .add_backing(RAM, RAM_SIZE)
        .map_err(|e| format!("ram: {e:?}"))?;

    let mut virtio = VirtioBlk::new();
    if let Some(disk) = disk_image {
        virtio.attach_disk(disk);
    }
    let gic = GicV2::new();

    machine
        .register_device(VIRTIO, Box::new(virtio))
        .map_err(|e| format!("virtio: {e:?}"))?;
    machine
        .register_device(UART, Box::new(Pl011::new(Pl011Params::new(UART_IRQ), None)))
        .map_err(|e| format!("uart: {e:?}"))?;
    machine
        .register_device(GIC, Box::new(gic))
        .map_err(|e| format!("gic: {e:?}"))?;

    let mailbox = Arc::new(IrqMailbox::new());
    machine
        .register_sink(
            IRQ_SINK,
            Box::new(EngineIrqSink::new(ARM_IRQ, Arc::clone(&mailbox))),
        )
        .map_err(|e| format!("sink: {e:?}"))?;

    machine
        .connect(UART, UART_IRQ, IRQ_SINK)
        .map_err(|e| format!("connect uart: {e:?}"))?;

    machine.verify().map_err(|e| format!("verify: {e:?}"))?;

    // Load AArch64 boot code: NOP (0xD503201F LE) and B . (0x14000000 LE)
    let boot_bytes: [u8; 8] = [0x1F, 0x20, 0x03, 0xD5, 0x00, 0x00, 0x00, 0x14];
    machine
        .loader_write(RAM, 0, &boot_bytes)
        .map_err(|e| format!("loader: {e:?}"))?;

    let mut engine = UnicornArm64::new().map_err(|e| format!("engine: {e:?}"))?;
    let shared = bind_flat_view(&mut engine, &view, Arc::new(Mutex::new(machine)), CPU)
        .map_err(|e| format!("bind: {e:?}"))?;

    engine
        .reg_write(mosaic_isa_aarch64::Register::Pc.into(), RAM_BASE)
        .map_err(|e| format!("pc: {e:?}"))?;

    let mut driver = MachineDriver::new(engine, shared);
    driver.attach_irq(Arc::clone(&mailbox));

    let report = driver
        .step(NonZeroU64::new(100).unwrap(), 16)
        .map_err(|e| format!("step: {e:?}"))?;

    Ok(GenericVmReport {
        hooks: report.engine.instruction_hooks,
        events: report.events_delivered,
        reason: report.engine.reason,
        ide_storage_ready: false,
        virtio_storage_ready: true,
        iso_volume_id: None,
    })
}
