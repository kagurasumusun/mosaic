//! Derive the engine memory map from a runtime FlatView and connect MMIO.
//!
//! Ram/Rom leaves become native private pages preloaded from the machine
//! backing blocks (the P1 DMI substitute); Mmio leaves become callback regions
//! routed through [`MachineBus`]. The FlatView stays the single source of
//! truth: nothing here invents an address range.

use std::sync::{Arc, Mutex};

use mosaic_contract_cpu_engine::{CpuEngine, EngineError};
use mosaic_core_runtime::{FlatView, Machine};
use mosaic_core_types::{Addr, BusFault, MemoryKind, NodeId};
use mosaic_engine_unicorn::{UnicornArm, PAGE_SIZE};

use crate::bus::MachineBus;

/// Binding construction failures. Control-plane errors stay separate from the
/// guest bus faults recorded during emulation.
#[derive(Debug)]
#[non_exhaustive]
pub enum BindError {
    /// Backend MMIO maps are page-granular; the FlatView must match.
    NotPageAligned(Addr),
    /// A backing block does not fit a host-side transfer buffer.
    BackingTooLarge(Addr),
    Engine(EngineError),
    Bus(BusFault),
}

impl From<EngineError> for BindError {
    fn from(value: EngineError) -> Self {
        Self::Engine(value)
    }
}

impl From<BusFault> for BindError {
    fn from(value: BusFault) -> Self {
        Self::Bus(value)
    }
}

/// Bind `view` onto `engine` using `machine` as the backing and MMIO authority.
///
/// The machine must already carry every backing block, device and wiring the
/// view references; callers normally run [`Machine::verify`] first. Returns the
/// shared machine handle for the driver.
pub fn bind_flat_view(
    engine: &mut UnicornArm,
    view: &FlatView,
    machine: Arc<Mutex<Machine>>,
    master: NodeId,
) -> Result<Arc<Mutex<Machine>>, BindError> {
    for mapping in view.mappings() {
        if mapping.base.0 % PAGE_SIZE != 0 || mapping.size % PAGE_SIZE != 0 {
            return Err(BindError::NotPageAligned(mapping.base));
        }
    }

    engine.attach_bus(Box::new(MachineBus::new(Arc::clone(&machine), master)))?;

    for mapping in view.mappings() {
        match mapping.kind {
            MemoryKind::Ram | MemoryKind::Rom => {
                engine.map_memory(mapping.base, mapping.size, mapping.permissions)?;
                let length = usize::try_from(mapping.size)
                    .map_err(|_| BindError::BackingTooLarge(mapping.base))?;
                let mut bytes = vec![0u8; length];
                {
                    let guard = machine.lock().map_err(|_| BusFault::Device)?;
                    guard.loader_read(mapping.target, mapping.offset, &mut bytes)?;
                }
                engine.write_memory(mapping.base, &bytes)?;
            }
            MemoryKind::Mmio => {
                engine.map_mmio(mapping.base, mapping.size)?;
            }
        }
    }

    Ok(machine)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mosaic_contract_mmio::MmioTarget;
    use mosaic_contract_model::{ContextError, Ctx};
    use mosaic_core_runtime::{FlatView, Machine, Region};
    use mosaic_core_types::{
        AccessSize, Addr, Attr, BusFault, Mapping, MemoryKind, NodeId, Permissions,
    };
    use mosaic_engine_unicorn::UnicornArm;
    use mosaic_isa_arm::Register;

    use super::*;

    const ROM: NodeId = NodeId(1);
    const RAM: NodeId = NodeId(2);
    const DEVICE: NodeId = NodeId(3);
    const CPU: NodeId = NodeId(0);

    type WriteLog = Arc<Mutex<Vec<(u64, u8, u64)>>>;

    /// Records writes and answers reads with a fixed pattern so the guest can
    /// observe that the callback round trip preserves width and offset.
    #[derive(Clone)]
    struct ProbeDevice {
        writes: WriteLog,
    }

    impl ProbeDevice {
        fn new() -> (Self, WriteLog) {
            let writes = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    writes: Arc::clone(&writes),
                },
                writes,
            )
        }
    }

    impl MmioTarget for ProbeDevice {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            offset: u64,
            size: AccessSize,
            _attr: &Attr,
        ) -> Result<u64, BusFault> {
            Ok(0x0bad_0000 | (u64::from(size.bytes()) << 8) | offset)
        }

        fn write(
            &mut self,
            _ctx: &mut dyn Ctx,
            offset: u64,
            size: AccessSize,
            value: u64,
            _attr: &Attr,
        ) -> Result<(), BusFault> {
            self.writes
                .lock()
                .unwrap()
                .push((offset, size.bytes(), value));
            Ok(())
        }
    }

    impl mosaic_contract_model::EventTarget for ProbeDevice {
        fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
            Ok(())
        }
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

    fn standard_view() -> FlatView {
        FlatView::build(&[
            leaf(0, 0x1000, ROM, MemoryKind::Rom, Permissions::RX),
            leaf(0x1000, 0x1000, RAM, MemoryKind::Ram, Permissions::RW),
            leaf(0x2000, 0x1000, DEVICE, MemoryKind::Mmio, Permissions::RW),
        ])
        .unwrap()
    }

    fn machine(view: FlatView) -> Machine {
        let (device, _) = ProbeDevice::new();
        let mut machine = Machine::new(view, 16);
        machine.add_backing(ROM, 0x1000).unwrap();
        machine.add_backing(RAM, 0x1000).unwrap();
        machine.register_device(DEVICE, Box::new(device)).unwrap();
        machine.verify().unwrap();
        machine
    }

    #[test]
    fn guest_mmio_and_ram_traverse_the_flatview_binding() {
        let view = standard_view();
        let (device, writes) = ProbeDevice::new();
        let mut machine = Machine::new(view.clone(), 16);
        machine.add_backing(ROM, 0x1000).unwrap();
        machine.add_backing(RAM, 0x1000).unwrap();
        machine.register_device(DEVICE, Box::new(device)).unwrap();
        machine.verify().unwrap();
        // RAM pattern the guest loads through the native engine page.
        machine
            .loader_write(RAM, 0, &0x1122_3344u32.to_le_bytes())
            .unwrap();
        // A32: MOV r0,#0x55; STR r0,[r2]; LDR r1,[r2]; LDR r4,[r5]; STRB r0,[r2,#4]
        let code = [
            0x55u8, 0, 0xa0, 0xe3, 0, 0, 0x82, 0xe5, 0, 0x10, 0x92, 0xe5, 0, 0x40, 0x95, 0xe5, 4,
            0, 0xc2, 0xe5,
        ];
        machine.loader_write(ROM, 0, &code).unwrap();

        let mut engine = UnicornArm::new().unwrap();
        let _machine =
            bind_flat_view(&mut engine, &view, Arc::new(Mutex::new(machine)), CPU).unwrap();

        engine.reg_write(Register::Pc.into(), 0).unwrap();
        engine.reg_write(Register::R2.into(), 0x2000).unwrap();
        engine.reg_write(Register::R5.into(), 0x1000).unwrap();
        let outcome = engine.run(core::num::NonZeroU64::new(5).unwrap());
        assert_eq!(
            outcome.reason,
            mosaic_contract_cpu_engine::StopReason::BudgetExhausted
        );
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(0x55));
        // Device read encodes width 4 and device-local offset 0.
        assert_eq!(engine.reg_read(Register::R1.into()), Ok(0x0bad_0400));
        assert_eq!(engine.reg_read(Register::R4.into()), Ok(0x1122_3344));
        assert_eq!(*writes.lock().unwrap(), vec![(0, 4, 0x55), (4, 1, 0x55)]);
        assert!(engine.take_bus_fault().is_none());
    }

    #[test]
    fn non_page_aligned_views_are_rejected_before_anything_is_mapped() {
        let unaligned = FlatView::build(&[leaf(
            0x10,
            0x1000,
            DEVICE,
            MemoryKind::Mmio,
            Permissions::RW,
        )])
        .unwrap();
        let mut engine = UnicornArm::new().unwrap();
        let result = bind_flat_view(
            &mut engine,
            &unaligned,
            Arc::new(Mutex::new(machine(standard_view()))),
            CPU,
        );
        assert!(matches!(result, Err(BindError::NotPageAligned(_))));
    }
}
