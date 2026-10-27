//! CPU-free machine demonstration: FlatView resolution, RAM/ROM backing,
//! MMIO dispatch into an arena device and deferred signal delivery.
//!
//! Like the events example this is deterministic and contains no firmware or
//! real device model. The same script runs twice; identical output proves the
//! machine trace is reproducible from construction onward.

use std::sync::{Arc, Mutex};

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_contract::signal::SignalSink;
use mosaic_exec::{
    AccessSize, Addr, BusFault, Mapping, MemoryKind, NodeId, Permissions, SignalId, VDelta,
};
use mosaic_runtime::{bus_attr, EventKind, FlatView, Machine, Region};

const ROM: NodeId = NodeId(1);
const RAM: NodeId = NodeId(2);
const DEVICE: NodeId = NodeId(3);
const SINK: NodeId = NodeId(4);

/// A write schedules a node-local timer; the timer raises the wired signal.
struct PacingDevice;

impl MmioTarget for PacingDevice {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &mosaic_exec::Attr,
    ) -> Result<u64, BusFault> {
        if offset >= 16 {
            return Err(BusFault::Device);
        }
        Ok(0x5a)
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        value: u64,
        _attr: &mosaic_exec::Attr,
    ) -> Result<(), BusFault> {
        if offset >= 16 {
            return Err(BusFault::Device);
        }
        ctx.schedule_after(VDelta(value.saturating_mul(100)), offset)
            .map_err(|_| BusFault::Device)?;
        Ok(())
    }
}

impl EventTarget for PacingDevice {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        ctx.set_signal(SignalId(1), true)
    }
}

#[derive(Default, Clone)]
struct TraceSink {
    levels: Arc<Mutex<Vec<bool>>>,
}

impl SignalSink for TraceSink {
    fn set_level(&mut self, high: bool) {
        self.levels.lock().expect("sink lock").push(high);
    }
}

fn build() -> (Machine, TraceSink) {
    let view = FlatView::build(&[
        Region::leaf(
            Mapping {
                base: Addr(0),
                size: 0x1000,
                target: ROM,
                offset: 0,
                kind: MemoryKind::Rom,
                permissions: Permissions::RX,
            },
            0,
        ),
        Region::leaf(
            Mapping {
                base: Addr(0x1000),
                size: 0x1000,
                target: RAM,
                offset: 0,
                kind: MemoryKind::Ram,
                permissions: Permissions::RW,
            },
            0,
        ),
        Region::leaf(
            Mapping {
                base: Addr(0x2000),
                size: 0x1000,
                target: DEVICE,
                offset: 0,
                kind: MemoryKind::Mmio,
                permissions: Permissions::RW,
            },
            0,
        ),
    ])
    .expect("fixed layout");
    let mut machine = Machine::new(view, 32);
    machine.add_backing(ROM, 0x1000).expect("rom backing");
    machine.add_backing(RAM, 0x1000).expect("ram backing");
    machine
        .register_device(DEVICE, Box::new(PacingDevice))
        .expect("device");
    let probe = TraceSink::default();
    machine
        .register_sink(SINK, Box::new(probe.clone()))
        .expect("sink");
    machine.connect(DEVICE, SignalId(1), SINK).expect("wiring");
    machine.verify().expect("complete machine");
    (machine, probe)
}

fn script() -> Result<Vec<String>, String> {
    let (mut machine, probe) = build();
    let attr = bus_attr(RAM);
    let mut lines = Vec::new();

    machine
        .loader_write(ROM, 0, &[10, 20, 30, 40])
        .map_err(|e| format!("loader: {e:?}"))?;
    let mut bytes = [0; 4];
    machine
        .loader_read(ROM, 0, &mut bytes)
        .map_err(|e| format!("loader: {e:?}"))?;
    lines.push(format!("rom loader: {bytes:?}"));

    let rom_fault = machine
        .bus_write(Addr(0), AccessSize::Byte, 1, &attr)
        .expect_err("rom guest write must fail");
    lines.push(format!("guest write rom: {rom_fault:?}"));

    // Timer at 300ps (token 0), then timer at 100ps (token 4): queue orders.
    machine
        .bus_write(Addr(0x2000), AccessSize::Word, 3, &attr)
        .map_err(|e| format!("mmio: {e:?}"))?;
    machine
        .bus_write(Addr(0x2004), AccessSize::Word, 1, &attr)
        .map_err(|e| format!("mmio: {e:?}"))?;
    while let Some(event) = machine.step().map_err(|e| format!("dispatch: {e:?}"))? {
        if let EventKind::Signal { signal, high } = event.kind {
            lines.push(format!(
                "time={} signal={} high={high}",
                event.at.0, signal.0
            ));
        }
    }
    let recorded = probe.levels.lock().expect("sink lock");
    lines.push(format!("sink levels: {:?}", recorded.as_slice()));

    machine
        .bus_write(Addr(0x1000), AccessSize::Word, 0x1122_3344, &attr)
        .map_err(|e| format!("ram: {e:?}"))?;
    let value = machine
        .bus_read(Addr(0x1000), AccessSize::Word, &attr)
        .map_err(|e| format!("ram: {e:?}"))?;
    lines.push(format!("ram[0x1000]=0x{value:08x}"));

    let device_fault = machine
        .bus_write(Addr(0x2010), AccessSize::Word, 1, &attr)
        .expect_err("offset 16 is outside the device window");
    lines.push(format!("device fault: {device_fault:?}"));
    let unmapped = machine
        .bus_read(Addr(0x4000), AccessSize::Byte, &attr)
        .expect_err("nothing is mapped at 0x4000");
    lines.push(format!("unmapped read: {unmapped:?}"));
    let misaligned = machine
        .bus_write(Addr(0x1001), AccessSize::HalfWord, 1, &attr)
        .expect_err("halfword at an odd address");
    lines.push(format!("misaligned write: {misaligned:?}"));

    Ok(lines)
}

fn main() -> Result<(), String> {
    let first = script()?;
    let second = script()?;
    if first != second {
        return Err("machine trace diverged between identical runs".into());
    }
    println!("{}", first.join("\n"));
    println!("determinism: identical");
    Ok(())
}
