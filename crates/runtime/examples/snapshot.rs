//! Demonstrates the first P4 runtime checkpoint: RAM, a typed device State,
//! and pending virtual-time events are restored together and replayed.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{
    AccessSize, Addr, Attr, BusFault, Mapping, MemoryKind, NodeId, Permissions, Snapshot,
    SnapshotError, VTime,
};
use mosaic_runtime::{bus_attr, FlatView, Machine, Region};

const RAM: NodeId = NodeId(1);
const DEVICE: NodeId = NodeId(2);
const MASTER: NodeId = NodeId(7);

#[derive(Clone)]
struct CounterState {
    schema: u32,
    value: u64,
    ticks: u64,
}

struct Counter {
    value: u64,
    ticks: u64,
}

impl MmioTarget for Counter {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        match offset {
            0 => Ok(self.value),
            4 => Ok(self.ticks),
            _ => Err(BusFault::Device),
        }
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if offset != 0 {
            return Err(BusFault::Device);
        }
        self.value = value;
        Ok(())
    }
}

impl EventTarget for Counter {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        self.ticks += token;
        Ok(())
    }
}

impl Snapshot for Counter {
    type State = CounterState;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        CounterState {
            schema: Self::SCHEMA_VERSION,
            value: self.value,
            ticks: self.ticks,
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.value = state.value;
        self.ticks = state.ticks;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = mosaic_exec::Fnv64::new();
        hash.u32(version);
        hash.u32(state.schema);
        hash.u64(state.value);
        hash.u64(state.ticks);
        hash.finish()
    }
}

fn main() {
    let view = FlatView::build(&[
        Region::leaf(
            Mapping {
                base: Addr(0x1000),
                size: 16,
                target: RAM,
                offset: 0,
                kind: MemoryKind::Ram,
                permissions: Permissions::RWX,
            },
            0,
        ),
        Region::leaf(
            Mapping {
                base: Addr(0x2000),
                size: 16,
                target: DEVICE,
                offset: 0,
                kind: MemoryKind::Mmio,
                permissions: Permissions::RW,
            },
            0,
        ),
    ])
    .expect("valid address map");

    let mut machine = Machine::new(view, 16);
    machine.add_backing(RAM, 16).expect("RAM backing");
    machine
        .register_snapshot_device(DEVICE, Counter { value: 0, ticks: 0 })
        .expect("snapshot-capable device");
    machine.verify().expect("complete machine map");
    machine
        .loader_write(RAM, 0, &[0xAA, 0xBB])
        .expect("initialize RAM");

    let attr = bus_attr(MASTER);
    machine
        .bus_write(Addr(0x2000), AccessSize::Word, 41, &attr)
        .expect("write the model");
    machine
        .schedule_timer(DEVICE, VTime(10), 7)
        .expect("schedule timer");
    let checkpoint = machine.save_snapshot().expect("capture runtime state");
    println!(
        "checkpoint schema={} graph={:016x} ram={} devices={} time={} state={:016x}",
        checkpoint.schema_version(),
        checkpoint.graph_fingerprint(),
        checkpoint.memory_bytes(),
        checkpoint.device_count(),
        checkpoint.virtual_time().0,
        checkpoint.state_hash()
    );

    // Advance and corrupt all three state domains before restoring them.
    machine
        .loader_write(RAM, 0, &[0xCC, 0xDD])
        .expect("mutate RAM");
    machine
        .bus_write(Addr(0x2000), AccessSize::Word, 99, &attr)
        .expect("mutate model");
    machine.run_until(VTime(10), 8).expect("dispatch timer");

    machine
        .load_snapshot(&checkpoint)
        .expect("restore runtime state");
    let mut bytes = [0u8; 2];
    machine
        .loader_read(RAM, 0, &mut bytes)
        .expect("read restored RAM");
    let value = machine
        .bus_read(Addr(0x2000), AccessSize::Word, &attr)
        .expect("read restored model");
    let ticks = machine
        .bus_read(Addr(0x2004), AccessSize::Word, &attr)
        .expect("read restored timer count");
    println!(
        "restored ram={:02x} {:02x} value={} ticks={} pending={} time={}",
        bytes[0],
        bytes[1],
        value,
        ticks,
        machine.pending_events(),
        machine.now().0
    );

    machine
        .run_until(VTime(10), 8)
        .expect("replay restored timer");
    let replayed_value = machine
        .bus_read(Addr(0x2000), AccessSize::Word, &attr)
        .expect("read replayed model");
    let replayed_ticks = machine
        .bus_read(Addr(0x2004), AccessSize::Word, &attr)
        .expect("read replayed timer count");
    let replay = machine.save_snapshot().expect("checkpoint after replay");
    println!(
        "replay value={} ticks={} time={} state={:016x}",
        replayed_value,
        replayed_ticks,
        machine.now().0,
        replay.state_hash()
    );
}
