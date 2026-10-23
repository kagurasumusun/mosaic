//! Runtime-backed implementation of the engine callback bus.
//!
//! The machine is shared with the L5 driver that drains the event queue between
//! engine quanta. Execution stays single-threaded; the mutex only satisfies the
//! C-callback ownership boundary, it is never held while another machine call
//! can run. A poisoned mutex maps to a guest-visible device fault rather than a
//! panic.

use std::sync::{Arc, Mutex};

use mosaic_contract_cpu_engine::EngineBus;
use mosaic_core_runtime::{bus_attr, Machine};
use mosaic_core_types::{AccessSize, Addr, BusFault, NodeId};

/// Forwards engine MMIO callbacks to a runtime [`Machine`].
pub struct MachineBus {
    machine: Arc<Mutex<Machine>>,
    master: NodeId,
}

impl MachineBus {
    pub fn new(machine: Arc<Mutex<Machine>>, master: NodeId) -> Self {
        Self { machine, master }
    }

    /// Handle kept by the driver for event delivery and inspection between
    /// engine quanta.
    pub fn share(&self) -> Arc<Mutex<Machine>> {
        Arc::clone(&self.machine)
    }
}

impl EngineBus for MachineBus {
    fn read(&mut self, address: Addr, width: AccessSize) -> Result<u64, BusFault> {
        let mut machine = self.machine.lock().map_err(|_| BusFault::Device)?;
        machine.bus_read(address, width, &bus_attr(self.master))
    }

    fn write(&mut self, address: Addr, width: AccessSize, value: u64) -> Result<(), BusFault> {
        let mut machine = self.machine.lock().map_err(|_| BusFault::Device)?;
        machine.bus_write(address, width, value, &bus_attr(self.master))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use mosaic_contract_mmio::MmioTarget;
    use mosaic_contract_model::{ContextError, Ctx};
    use mosaic_core_runtime::{FlatView, Machine, Region};
    use mosaic_core_types::{
        AccessSize, Addr, Attr, BusFault, Mapping, MemoryKind, NodeId, Permissions,
    };

    const DEVICE: NodeId = NodeId(3);

    #[derive(Default)]
    struct EchoDevice;

    impl MmioTarget for EchoDevice {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            offset: u64,
            _size: AccessSize,
            attr: &Attr,
        ) -> Result<u64, BusFault> {
            // Low word is the device-local offset; high word is the Attr master.
            Ok((u64::from(attr.master.0) << 32) | (0x1000 + offset))
        }

        fn write(
            &mut self,
            _ctx: &mut dyn Ctx,
            offset: u64,
            _size: AccessSize,
            _value: u64,
            _attr: &Attr,
        ) -> Result<(), BusFault> {
            if offset >= 16 {
                return Err(BusFault::Device);
            }
            Ok(())
        }
    }

    impl mosaic_contract_model::EventTarget for EchoDevice {
        fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
            Ok(())
        }
    }

    fn machine() -> Machine {
        let view = FlatView::build(&[Region::leaf(
            Mapping {
                base: Addr(0x2000),
                size: 4096,
                target: DEVICE,
                offset: 0,
                kind: MemoryKind::Mmio,
                permissions: Permissions::RW,
            },
            0,
        )])
        .unwrap();
        let mut machine = Machine::new(view, 8);
        machine
            .register_device(DEVICE, Box::new(EchoDevice))
            .unwrap();
        machine.verify().unwrap();
        machine
    }

    #[test]
    fn engine_bus_resolves_offsets_and_synthesizes_the_master_attribute() {
        let mut bus = MachineBus::new(Arc::new(Mutex::new(machine())), NodeId(9));
        assert_eq!(
            bus.read(Addr(0x2004), AccessSize::Word),
            Ok((9u64 << 32) | 0x1004)
        );
        assert!(bus.write(Addr(0x2000), AccessSize::Word, 7).is_ok());
        assert_eq!(
            bus.write(Addr(0x2010), AccessSize::Word, 7),
            Err(BusFault::Device)
        );
        // The driver handle keeps the same machine instance alive.
        assert!(Arc::strong_count(&bus.share()) >= 2);
    }
}
