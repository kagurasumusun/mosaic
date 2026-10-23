//! Single-core cooperative scheduling between the CPU engine and the
//! deterministic event queue.
//!
//! One step runs an instruction quantum and then delivers every event due at
//! the machine's current virtual time. MMIO side effects produced inside the
//! quantum only queue events; they never re-enter the engine or a device, which
//! matches design §12.3 rule 3. Coupling instruction counts to virtual time
//! arrives with the clock tree; until then time advances only through explicit
//! event deadlines.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_contract_cpu_engine::{CpuEngine, EngineBusFault, RunOutcome, StopReason};
use mosaic_core_runtime::{Machine, MachineError};
use mosaic_core_types::VTime;
use mosaic_engine_unicorn::UnicornArm;

/// Failures surfaced by a cooperative step.
#[derive(Debug)]
#[non_exhaustive]
pub enum DriverError {
    /// The engine stopped on a callback bus fault.
    Bus(EngineBusFault),
    /// The engine stopped for a reason with no recorded bus fault, for example
    /// an instruction fetch from an unmapped page.
    EngineFault(RunOutcome),
    /// Event delivery failed, e.g. an exhausted event budget.
    Machine(MachineError),
    /// The shared machine mutex was poisoned.
    Poisoned,
}

/// Outcome of one cooperative step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StepReport {
    pub engine: RunOutcome,
    pub events_delivered: usize,
    pub now: VTime,
}

/// Owns one engine and the shared machine it executes against.
pub struct MachineDriver {
    engine: UnicornArm,
    machine: Arc<Mutex<Machine>>,
}

impl MachineDriver {
    pub fn new(engine: UnicornArm, machine: Arc<Mutex<Machine>>) -> Self {
        Self { engine, machine }
    }

    pub fn engine(&self) -> &UnicornArm {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut UnicornArm {
        &mut self.engine
    }

    pub fn machine(&self) -> Arc<Mutex<Machine>> {
        Arc::clone(&self.machine)
    }

    /// Run one instruction quantum and drain events due at the current virtual
    /// time. A bus or engine fault is returned before any event is delivered.
    pub fn step(
        &mut self,
        quantum: NonZeroU64,
        event_budget: usize,
    ) -> Result<StepReport, DriverError> {
        let outcome = self.engine.run(quantum);
        if outcome.reason == StopReason::Fault {
            return match self.engine.take_bus_fault() {
                Some(fault) => Err(DriverError::Bus(fault)),
                None => Err(DriverError::EngineFault(outcome)),
            };
        }
        let mut machine = self.machine.lock().map_err(|_| DriverError::Poisoned)?;
        let now = machine.now();
        let events_delivered = machine
            .run_until(now, event_budget)
            .map_err(DriverError::Machine)?;
        Ok(StepReport {
            engine: outcome,
            events_delivered,
            now,
        })
    }

    /// Advance virtual time to `deadline`, delivering due events. Used by
    /// boards whose timers are not yet driven by instruction-count time.
    pub fn advance(&mut self, deadline: VTime, event_budget: usize) -> Result<usize, DriverError> {
        let mut machine = self.machine.lock().map_err(|_| DriverError::Poisoned)?;
        machine
            .run_until(deadline, event_budget)
            .map_err(DriverError::Machine)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mosaic_contract_mmio::MmioTarget;
    use mosaic_contract_model::{ContextError, Ctx};
    use mosaic_contract_signal::SignalSink;
    use mosaic_core_runtime::{FlatView, Machine, Region};
    use mosaic_core_types::{
        AccessSize, Addr, Attr, BusFault, Mapping, MemoryKind, NodeId, Permissions, SignalId, VTime,
    };
    use mosaic_engine_unicorn::UnicornArm;
    use mosaic_isa_arm::Register;

    use super::*;
    use crate::bind_flat_view;

    const ROM: NodeId = NodeId(1);
    const DEVICE: NodeId = NodeId(3);
    const SINK: NodeId = NodeId(4);
    const CPU: NodeId = NodeId(0);

    #[derive(Default)]
    struct SignalDevice;

    impl MmioTarget for SignalDevice {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _offset: u64,
            _size: AccessSize,
            _attr: &Attr,
        ) -> Result<u64, BusFault> {
            Err(BusFault::Device)
        }

        fn write(
            &mut self,
            ctx: &mut dyn Ctx,
            _offset: u64,
            _size: AccessSize,
            _value: u64,
            _attr: &Attr,
        ) -> Result<(), BusFault> {
            ctx.set_signal(SignalId(1), true)
                .map_err(|_| BusFault::Device)
        }
    }

    impl mosaic_contract_model::EventTarget for SignalDevice {
        fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct LevelProbe {
        levels: Arc<Mutex<Vec<bool>>>,
    }

    impl SignalSink for LevelProbe {
        fn set_level(&mut self, high: bool) {
            self.levels.lock().unwrap().push(high);
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

    fn build(code: &[u8]) -> (MachineDriver, LevelProbe) {
        let view = FlatView::build(&[
            leaf(0, 0x1000, ROM, MemoryKind::Rom, Permissions::RX),
            leaf(0x2000, 0x1000, DEVICE, MemoryKind::Mmio, Permissions::RW),
        ])
        .unwrap();
        let probe = LevelProbe::default();
        let mut machine = Machine::new(view.clone(), 16);
        machine.add_backing(ROM, 0x1000).unwrap();
        machine
            .register_device(DEVICE, Box::new(SignalDevice))
            .unwrap();
        machine
            .register_sink(SINK, Box::new(probe.clone()))
            .unwrap();
        machine.connect(DEVICE, SignalId(1), SINK).unwrap();
        machine.verify().unwrap();
        machine.loader_write(ROM, 0, code).unwrap();

        let mut engine = UnicornArm::new().unwrap();
        let shared =
            bind_flat_view(&mut engine, &view, Arc::new(Mutex::new(machine)), CPU).unwrap();
        engine.reg_write(Register::Pc.into(), 0).unwrap();
        engine.reg_write(Register::R2.into(), 0x2000).unwrap();
        (MachineDriver::new(engine, shared), probe)
    }

    #[test]
    fn step_runs_the_quantum_then_delivers_queued_signals() {
        // MOV r0,#1; STR r0,[r2]
        let (mut driver, probe) = build(&[0x01, 0, 0xa0, 0xe3, 0, 0, 0x82, 0xe5]);
        let report = driver.step(NonZeroU64::new(2).unwrap(), 8).unwrap();
        assert_eq!(report.engine.reason, StopReason::BudgetExhausted);
        assert_eq!(report.engine.instruction_hooks, 2);
        assert_eq!(report.events_delivered, 1);
        assert_eq!(report.now, VTime::ZERO);
        assert_eq!(*probe.levels.lock().unwrap(), vec![true]);
    }

    #[test]
    fn bus_faults_abort_the_step_with_the_recorded_access() {
        // LDR r1,[r2] — the device answers every read with BusFault::Device.
        let (mut driver, _probe) = build(&[0, 0x10, 0x92, 0xe5]);
        let error = driver.step(NonZeroU64::new(2).unwrap(), 8).unwrap_err();
        match error {
            DriverError::Bus(fault) => {
                assert_eq!(fault.address, Addr(0x2000));
                assert_eq!(fault.width, 4);
                assert_eq!(fault.fault, BusFault::Device);
            }
            other => panic!("expected bus fault, got {other:?}"),
        }
        // The fault remains observable on the engine until taken.
        assert!(driver.engine_mut().take_bus_fault().is_none());
    }
}
