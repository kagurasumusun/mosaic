//! Derive the engine memory map from a runtime FlatView and connect MMIO.
//!
//! Ram/Rom leaves become native private pages preloaded from the machine
//! backing blocks (the P1 DMI substitute); leaves whose node is declared
//! [`SharedRegion`] become host-backed pages mapped from one buffer into
//! every core (design §12.3 rule 1: shared RAM is the same host memory in
//! each instance). Mmio leaves become callback regions routed through
//! [`MachineBus`]. The FlatView stays the single source of truth: nothing
//! here invents an address range.

use std::sync::{Arc, Mutex};

use mosaic_contract_cpu_engine::{CpuEngine, EngineBus, EngineError};
use mosaic_core_runtime::{FlatView, Machine};
use mosaic_core_types::{Addr, BusFault, MemoryKind, NodeId, Permissions};
use mosaic_engine_unicorn::{SharedRam, UnicornArm, PAGE_SIZE};

#[cfg(feature = "unicorn-riscv")]
use mosaic_engine_unicorn::UnicornRiscV32;

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
    /// A shared leaf is larger than its host buffer.
    SharedTooSmall(Addr),
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

/// The engine-side mapping operations the binder needs. Both Unicorn adapters
/// implement it; keeping the trait local means the engine contract stays free
/// of mapping APIs that only this composition root uses.
pub trait EngineMap: CpuEngine {
    fn attach_bus(&mut self, bus: Box<dyn EngineBus>) -> Result<(), EngineError>;
    fn map_mmio(&mut self, base: Addr, size: u64) -> Result<(), EngineError>;
    fn map_shared_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
        ram: Arc<SharedRam>,
    ) -> Result<(), EngineError>;
}

impl EngineMap for UnicornArm {
    fn attach_bus(&mut self, bus: Box<dyn EngineBus>) -> Result<(), EngineError> {
        // Fully qualified inherent method: the trait method of the same name
        // would otherwise recurse.
        UnicornArm::attach_bus(self, bus)
    }

    fn map_mmio(&mut self, base: Addr, size: u64) -> Result<(), EngineError> {
        UnicornArm::map_mmio(self, base, size)
    }

    fn map_shared_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
        ram: Arc<SharedRam>,
    ) -> Result<(), EngineError> {
        UnicornArm::map_shared_memory(self, base, size, permissions, ram)
    }
}

#[cfg(feature = "unicorn-riscv")]
impl EngineMap for UnicornRiscV32 {
    fn attach_bus(&mut self, bus: Box<dyn EngineBus>) -> Result<(), EngineError> {
        UnicornRiscV32::attach_bus(self, bus)
    }

    fn map_mmio(&mut self, base: Addr, size: u64) -> Result<(), EngineError> {
        UnicornRiscV32::map_mmio(self, base, size)
    }

    fn map_shared_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
        ram: Arc<SharedRam>,
    ) -> Result<(), EngineError> {
        UnicornRiscV32::map_shared_memory(self, base, size, permissions, ram)
    }
}

/// A Ram/Rom leaf whose backing is mapped into engines from one host buffer
/// shared by every core that maps it (the DMI path, design §13.1).
pub struct SharedRegion {
    pub node: NodeId,
    pub ram: Arc<SharedRam>,
}

/// Bind `view` onto `engine` using `machine` as the backing and MMIO authority.
///
/// The machine must already carry every backing block, device and wiring the
/// view references; callers normally run [`Machine::verify`] first. Returns the
/// shared machine handle for the driver.
pub fn bind_flat_view(
    engine: &mut dyn EngineMap,
    view: &FlatView,
    machine: Arc<Mutex<Machine>>,
    master: NodeId,
) -> Result<Arc<Mutex<Machine>>, BindError> {
    bind_flat_view_shared(engine, view, machine, master, &[])
}

/// [`bind_flat_view`] with host-backed shared leaves. For every Ram/Rom mapping
/// whose target node appears in `shared`, the machine backing block is copied
/// into the shared buffer and the buffer is mapped into the engine, so all
/// cores binding the same [`SharedRegion`] observe one memory. The copy runs
/// while no core has executed yet and is idempotent per core.
pub fn bind_flat_view_shared(
    engine: &mut dyn EngineMap,
    view: &FlatView,
    machine: Arc<Mutex<Machine>>,
    master: NodeId,
    shared: &[SharedRegion],
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
                let length = usize::try_from(mapping.size)
                    .map_err(|_| BindError::BackingTooLarge(mapping.base))?;
                let mut bytes = vec![0u8; length];
                {
                    let guard = machine.lock().map_err(|_| BusFault::Device)?;
                    guard.loader_read(mapping.target, mapping.offset, &mut bytes)?;
                }
                match shared.iter().find(|region| region.node == mapping.target) {
                    Some(region) => {
                        if mapping.size > region.ram.len() as u64 {
                            return Err(BindError::SharedTooSmall(mapping.base));
                        }
                        region.ram.write(0, &bytes)?;
                        engine.map_shared_memory(
                            mapping.base,
                            mapping.size,
                            mapping.permissions,
                            Arc::clone(&region.ram),
                        )?;
                    }
                    None => {
                        engine.map_memory(mapping.base, mapping.size, mapping.permissions)?;
                        engine.write_memory(mapping.base, &bytes)?;
                    }
                }
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

    #[test]
    fn shared_leaves_map_one_host_buffer_into_every_core() {
        const CORE1: NodeId = NodeId(7);
        let view = FlatView::build(&[
            leaf(0, 0x1000, RAM, MemoryKind::Ram, Permissions::RWX),
            leaf(0x10000, 0x1000, DEVICE, MemoryKind::Ram, Permissions::RW),
        ])
        .unwrap();
        let mut machine = Machine::new(view.clone(), 16);
        machine.add_backing(RAM, 0x1000).unwrap();
        // The shared node keeps a normal backing block: it is the loader
        // source of truth and satisfies verify() for both viewpoints.
        machine.add_backing(DEVICE, 0x1000).unwrap();
        machine.add_view(CORE1, view.clone()).unwrap();
        machine.loader_write(DEVICE, 4, &[0xEE]).unwrap();
        machine.verify().unwrap();
        let machine = Arc::new(Mutex::new(machine));

        let ram = SharedRam::new(0x1000).unwrap();
        let shared = [SharedRegion {
            node: DEVICE,
            ram: Arc::clone(&ram),
        }];
        // Loader image is synced into the host buffer before any core runs.
        let mut bytes = [0u8; 8];
        ram.read(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);

        let mut first = UnicornArm::new().unwrap();
        let mut second = UnicornArm::new().unwrap();
        bind_flat_view_shared(&mut first, &view, Arc::clone(&machine), CPU, &shared).unwrap();
        bind_flat_view_shared(&mut second, &view, Arc::clone(&machine), CORE1, &shared).unwrap();
        ram.read(0, &mut bytes).unwrap();
        assert_eq!(bytes[4], 0xEE, "loader image must reach the shared buffer");

        // Guest store through the first core is instantly visible to the
        // second core and to host-side readers (design §12.3 rule 1).
        // MOV r0,#0xAB; LDR r1,[pc,#0]; STRB r0,[r1]; B .; .word 0x00010000
        let code = [
            0xabu8, 0, 0xa0, 0xe3, 0, 0x10, 0x9f, 0xe5, 0, 0, 0xc1, 0xe5, 0xfe, 0xff, 0xff, 0xea,
            0, 0, 1, 0,
        ];
        machine.lock().unwrap().loader_write(RAM, 0, &code).unwrap();
        first.write_memory(Addr(0), &code).unwrap();
        first.reg_write(Register::Pc.into(), 0).unwrap();
        first.run(core::num::NonZeroU64::new(3).unwrap());
        ram.read(0, &mut bytes).unwrap();
        assert_eq!(bytes[0], 0xAB);
        second.read_memory(Addr(0x10000), &mut bytes).unwrap();
        assert_eq!(bytes[0], 0xAB);
    }

    #[test]
    fn shared_leaves_must_fit_the_host_buffer() {
        let view = FlatView::build(&[leaf(
            0x10000,
            0x2000,
            DEVICE,
            MemoryKind::Ram,
            Permissions::RW,
        )])
        .unwrap();
        let mut machine = Machine::new(view.clone(), 8);
        machine.add_backing(DEVICE, 0x2000).unwrap();
        machine.verify().unwrap();
        let ram = SharedRam::new(0x1000).unwrap();
        let shared = [SharedRegion {
            node: DEVICE,
            ram,
        }];
        let mut engine = UnicornArm::new().unwrap();
        let result = bind_flat_view_shared(
            &mut engine,
            &view,
            Arc::new(Mutex::new(machine)),
            CPU,
            &shared,
        );
        assert!(matches!(result, Err(BindError::SharedTooSmall(_))));
    }
}
