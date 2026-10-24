//! Multicore cooperative scheduling over heterogeneous engines (design §12.4).
//!
//! Cores run in fixed round-robin order: each core receives one instruction
//! quantum, then due events are delivered at the machine's current virtual
//! time, then that core's queued interrupt edges are flushed before the next
//! core runs. Fixed order plus one shared event queue keeps same-input/same-
//! output determinism (design §19 T8) while engines stay per-core instances
//! that know nothing about each other (design §12.3 rule 1). MMIO side
//! effects inside a quantum only queue events; they never re-enter an engine
//! (design §12.3 rule 3). Inter-core cooperation (shared RAM, mailbox, IPI)
//! is composition: the scheduler moves cores, the runtime moves data.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_contract_cpu_engine::{CpuEngine, EngineError, RunOutcome, StopReason};
use mosaic_core_runtime::Machine;
use mosaic_core_types::VTime;

use crate::{DriverError, IrqMailbox, StepReport};

/// Default resolved clock period for [`MulticoreDriver::step_clocked`]: a
/// 1 GHz core, one instruction per tick until a clock tree resolves per-core
/// frequencies (design §13.2).
pub const DEFAULT_PS_PER_INSTRUCTION: u64 = 1000;

/// A failure attributed to one core's slice of a round.
#[derive(Debug)]
#[non_exhaustive]
pub enum MulticoreError {
    /// The failing core's index plus the underlying single-core error.
    Core { index: usize, error: DriverError },
    /// A configuration call referenced a core that was never added.
    UnknownCore(usize),
    /// The shared machine mutex was poisoned.
    Poisoned,
}

/// One core's execution configuration: its engine instance, the mailbox that
/// carries wired interrupt edges into it between slices, and its resolved
/// clock period for instruction-count-to-VTime conversion.
struct CoreSlot {
    engine: Box<dyn CoreEngine>,
    irq: Option<Arc<IrqMailbox>>,
    ps_per_instruction: NonZeroU64,
}

/// A core's engine plus the shared-page synchronisation it supports. The
/// synchronisation lives in the same object as the engine because the
/// mirrors live in its address space (ADR 0010).
pub trait CoreEngine: CpuEngine + SharedSync {}

impl<T: CpuEngine + SharedSync> CoreEngine for T {}

/// Copy shared pages between the authoritative buffers and an engine's
/// mirrors. See [`crate::unicorn_bind::EngineMap`] for the backend contract.
pub trait SharedSync {
    fn sync_shared_in(&mut self) -> Result<(), EngineError>;
    fn sync_shared_out(&mut self) -> Result<(), EngineError>;
}

/// Outcome of one round-robin pass over every core.
#[derive(Clone, Debug)]
pub struct RoundReport {
    /// Per-core reports in fixed execution order.
    pub cores: Vec<StepReport>,
    /// Machine virtual time after the round.
    pub now: VTime,
}

/// Round-robin quantum scheduler over `N` engine instances and one machine.
pub struct MulticoreDriver {
    cores: Vec<CoreSlot>,
    machine: Arc<Mutex<Machine>>,
}

impl MulticoreDriver {
    pub fn new(machine: Arc<Mutex<Machine>>) -> Self {
        Self {
            cores: Vec::new(),
            machine,
        }
    }

    /// Add a core; its index fixes its position in every round. Addition
    /// order is execution order and never changes afterwards.
    pub fn add_core(&mut self, engine: Box<dyn CoreEngine>) -> usize {
        let index = self.cores.len();
        let ps = NonZeroU64::new(DEFAULT_PS_PER_INSTRUCTION)
            .expect("the default clock period is never zero");
        self.cores.push(CoreSlot {
            engine,
            irq: None,
            ps_per_instruction: ps,
        });
        index
    }

    /// Route CPU-bound interrupt edges of one core through `mailbox`, the same
    /// mailbox that backs the [`crate::EngineIrqSink`] nodes of that core.
    pub fn attach_irq(
        &mut self,
        core: usize,
        mailbox: Arc<IrqMailbox>,
    ) -> Result<(), MulticoreError> {
        let slot = self
            .cores
            .get_mut(core)
            .ok_or(MulticoreError::UnknownCore(core))?;
        slot.irq = Some(mailbox);
        Ok(())
    }

    /// Set one core's resolved clock period for [`Self::step_clocked`].
    /// Heterogeneous cores may run at different periods.
    pub fn set_ps_per_instruction(
        &mut self,
        core: usize,
        ps: NonZeroU64,
    ) -> Result<(), MulticoreError> {
        let slot = self
            .cores
            .get_mut(core)
            .ok_or(MulticoreError::UnknownCore(core))?;
        slot.ps_per_instruction = ps;
        Ok(())
    }

    pub fn core_count(&self) -> usize {
        self.cores.len()
    }

    /// Engine of one core for inspection between rounds (registers, recorded
    /// interrupt edges). `None` when the index was never added.
    pub fn core(&self, index: usize) -> Option<&dyn CpuEngine> {
        self.cores
            .get(index)
            .map(|slot| slot.engine.as_ref() as &dyn CpuEngine)
    }

    pub fn machine(&self) -> Arc<Mutex<Machine>> {
        Arc::clone(&self.machine)
    }

    /// One round-robin pass without advancing virtual time: every core runs
    /// one quantum, then events due at the current machine time are delivered
    /// and the core's interrupt mailbox is flushed. Matches
    /// [`crate::MachineDriver::step`] per core.
    pub fn step(
        &mut self,
        quantum: NonZeroU64,
        event_budget: usize,
    ) -> Result<RoundReport, MulticoreError> {
        let mut reports = Vec::with_capacity(self.cores.len());
        for (index, core) in self.cores.iter_mut().enumerate() {
            // Shared pages: pull the authoritative bytes into the core's
            // mirror before the slice, push them back afterwards.
            core.engine
                .sync_shared_in()
                .map_err(|error| MulticoreError::Core {
                    index,
                    error: DriverError::Engine(error),
                })?;
            let outcome = core.engine.run(quantum);
            if outcome.reason == StopReason::Fault {
                return Err(MulticoreError::Core {
                    index,
                    error: fault_of(core.engine.as_mut(), outcome),
                });
            }
            let (events_delivered, now) = {
                let mut machine = self.machine.lock().map_err(|_| MulticoreError::Poisoned)?;
                let now = machine.now();
                let delivered =
                    machine
                        .run_until(now, event_budget)
                        .map_err(|error| MulticoreError::Core {
                            index,
                            error: DriverError::Machine(error),
                        })?;
                (delivered, now)
            };
            flush(core, index)?;
            core.engine
                .sync_shared_out()
                .map_err(|error| MulticoreError::Core {
                    index,
                    error: DriverError::Engine(error),
                })?;
            reports.push(StepReport {
                engine: outcome,
                events_delivered,
                now,
            });
        }
        let now = self
            .machine
            .lock()
            .map_err(|_| MulticoreError::Poisoned)?
            .now();
        Ok(RoundReport {
            cores: reports,
            now,
        })
    }

    /// One round-robin pass where every core advances the shared virtual time
    /// by its consumed instruction count times its own resolved clock period,
    /// draining events due by each resulting deadline before the next core
    /// runs. The global timeline concatenates the slices in fixed order, so
    /// heterogeneous clocks stay deterministic.
    pub fn step_clocked(
        &mut self,
        quantum: NonZeroU64,
        event_budget: usize,
    ) -> Result<RoundReport, MulticoreError> {
        let mut reports = Vec::with_capacity(self.cores.len());
        for (index, core) in self.cores.iter_mut().enumerate() {
            core.engine
                .sync_shared_in()
                .map_err(|error| MulticoreError::Core {
                    index,
                    error: DriverError::Engine(error),
                })?;
            let outcome = core.engine.run(quantum);
            if outcome.reason == StopReason::Fault {
                return Err(MulticoreError::Core {
                    index,
                    error: fault_of(core.engine.as_mut(), outcome),
                });
            }
            let ps = core.ps_per_instruction.get();
            let delta = u128::from(outcome.instruction_hooks)
                .checked_mul(u128::from(ps))
                .and_then(|value| u64::try_from(value).ok())
                .ok_or(MulticoreError::Core {
                    index,
                    error: DriverError::TimeOverflow,
                })?;
            let (events_delivered, now) = {
                let mut machine = self.machine.lock().map_err(|_| MulticoreError::Poisoned)?;
                let deadline = machine
                    .now()
                    .0
                    .checked_add(delta)
                    .ok_or(MulticoreError::Core {
                        index,
                        error: DriverError::TimeOverflow,
                    })?;
                let delivered =
                    machine
                        .run_until(VTime(deadline), event_budget)
                        .map_err(|error| MulticoreError::Core {
                            index,
                            error: DriverError::Machine(error),
                        })?;
                (delivered, VTime(deadline))
            };
            flush(core, index)?;
            core.engine
                .sync_shared_out()
                .map_err(|error| MulticoreError::Core {
                    index,
                    error: DriverError::Engine(error),
                })?;
            reports.push(StepReport {
                engine: outcome,
                events_delivered,
                now,
            });
        }
        let now = self
            .machine
            .lock()
            .map_err(|_| MulticoreError::Poisoned)?
            .now();
        Ok(RoundReport {
            cores: reports,
            now,
        })
    }

    /// Run `rounds` passes with identical settings, in order.
    pub fn run(
        &mut self,
        rounds: usize,
        quantum: NonZeroU64,
        event_budget: usize,
    ) -> Result<Vec<RoundReport>, MulticoreError> {
        let mut all = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            all.push(self.step(quantum, event_budget)?);
        }
        Ok(all)
    }
}

/// Forward the core's queued interrupt edges into its engine (FIFO).
fn flush(core: &mut CoreSlot, index: usize) -> Result<(), MulticoreError> {
    if let Some(mailbox) = &core.irq {
        mailbox
            .drain_into(core.engine.as_mut())
            .map_err(|error| MulticoreError::Core {
                index,
                error: DriverError::Engine(error),
            })?;
    }
    Ok(())
}

/// Classify a fault stop: a recorded bus fault wins, otherwise the raw
/// engine outcome is preserved for the composition root.
fn fault_of(engine: &mut dyn CoreEngine, outcome: RunOutcome) -> DriverError {
    match engine.take_bus_fault() {
        Some(fault) => DriverError::Bus(fault),
        None => DriverError::EngineFault(outcome),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mosaic_core_runtime::{FlatView, Machine, Region};
    use mosaic_core_types::{Addr, Mapping, MemoryKind, NodeId, Permissions};
    use mosaic_engine_unicorn::{SharedRam, UnicornArm};

    use super::*;
    use crate::{bind_flat_view_shared, SharedRegion};

    const CPU0: NodeId = NodeId(0);
    const CPU1: NodeId = NodeId(1);
    const RAM0: NodeId = NodeId(10);
    const RAM1: NodeId = NodeId(11);
    const SHARED: NodeId = NodeId(20);

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

    fn words(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    /// Two ARM cores, per-core images at 0x0 and one shared page at 0x10000.
    /// core0 stores 0xAA to shared[0]; core1 reads it, adds 1 and stores the
    /// result to shared[1] — the round-robin order makes the exchange
    /// complete inside a single round.
    fn two_arm_cores() -> (MulticoreDriver, Arc<SharedRam>) {
        let view0 = FlatView::build(&[
            leaf(0, 0x1000, RAM0, MemoryKind::Ram, Permissions::RWX),
            leaf(0x10000, 0x1000, SHARED, MemoryKind::Ram, Permissions::RW),
        ])
        .unwrap();
        let view1 = FlatView::build(&[
            leaf(0, 0x1000, RAM1, MemoryKind::Ram, Permissions::RWX),
            leaf(0x10000, 0x1000, SHARED, MemoryKind::Ram, Permissions::RW),
        ])
        .unwrap();
        let mut machine = Machine::new(view0.clone(), 32);
        machine.add_view(CPU1, view1.clone()).unwrap();
        machine.add_backing(RAM0, 0x1000).unwrap();
        machine.add_backing(RAM1, 0x1000).unwrap();
        machine.add_backing(SHARED, 0x1000).unwrap();
        // core0: MOV r0,#0xAA; LDR r1,[pc,#4]; STRB r0,[r1]; B .; .word shared
        machine
            .loader_write(
                RAM0,
                0,
                &words(&[
                    0xE3A0_00AA,
                    0xE59F_1004,
                    0xE5C1_0000,
                    0xEAFF_FFFE,
                    0x0001_0000,
                ]),
            )
            .unwrap();
        // core1: LDR r1,[pc,#12]; LDRB r2,[r1]; ADD r2,#1; STRB r2,[r1,#1];
        //        B .; .word shared
        machine
            .loader_write(
                RAM1,
                0,
                &words(&[
                    0xE59F_100C,
                    0xE5D1_2000,
                    0xE282_2001,
                    0xE5C1_2001,
                    0xEAFF_FFFE,
                    0x0001_0000,
                ]),
            )
            .unwrap();
        machine.verify().unwrap();
        let machine = Arc::new(Mutex::new(machine));

        let ram = SharedRam::new(0x1000).unwrap();
        let shared = [SharedRegion {
            node: SHARED,
            ram: Arc::clone(&ram),
        }];
        let mut first = UnicornArm::new().unwrap();
        let mut second = UnicornArm::new().unwrap();
        bind_flat_view_shared(&mut first, &view0, Arc::clone(&machine), CPU0, &shared).unwrap();
        bind_flat_view_shared(&mut second, &view1, Arc::clone(&machine), CPU1, &shared).unwrap();
        for engine in [&mut first, &mut second] {
            engine
                .reg_write(mosaic_isa_arm::Register::Pc.into(), 0)
                .unwrap();
        }
        let mut driver = MulticoreDriver::new(machine);
        driver.add_core(Box::new(crate::SharedEngine::new(Box::new(first))));
        driver.add_core(Box::new(crate::SharedEngine::new(Box::new(second))));
        (driver, ram)
    }

    #[test]
    fn round_robin_exchanges_data_through_shared_ram_in_one_round() {
        let (mut driver, ram) = two_arm_cores();
        let round = driver.step(NonZeroU64::new(5).unwrap(), 8).unwrap();
        assert_eq!(round.cores.len(), 2);
        // core0 retires its three instructions and spins out the budget;
        // core1 retires four and spins.
        assert_eq!(round.cores[0].engine.instruction_hooks, 5);
        assert_eq!(round.cores[1].engine.instruction_hooks, 5);
        let mut bytes = [0u8; 2];
        ram.read(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0xAA, 0xAB]);
    }

    #[test]
    fn identical_schedules_replay_identically() {
        let (mut first_driver, first_ram) = two_arm_cores();
        let (mut second_driver, second_ram) = two_arm_cores();
        let quantum = NonZeroU64::new(5).unwrap();
        let first = first_driver.run(3, quantum, 8).unwrap();
        let second = second_driver.run(3, quantum, 8).unwrap();
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.now, b.now);
            for (core_a, core_b) in a.cores.iter().zip(b.cores.iter()) {
                assert_eq!(core_a.engine, core_b.engine);
                assert_eq!(core_a.events_delivered, core_b.events_delivered);
            }
        }
        let mut left = [0u8; 2];
        let mut right = [0u8; 2];
        first_ram.read(0, &mut left).unwrap();
        second_ram.read(0, &mut right).unwrap();
        assert_eq!(left, right);
    }

    #[test]
    fn clocked_rounds_advance_the_shared_timeline_per_core_period() {
        let (mut driver, _ram) = two_arm_cores();
        driver
            .set_ps_per_instruction(1, NonZeroU64::new(2000).unwrap())
            .unwrap();
        let round = driver.step_clocked(NonZeroU64::new(5).unwrap(), 8).unwrap();
        // core0: 5 hooks at the default 1000 ps, core1: 5 hooks at 2000 ps.
        assert_eq!(round.cores[0].now, VTime(5000));
        assert_eq!(round.cores[1].now, VTime(15000));
        assert_eq!(round.now, VTime(15000));
    }

    #[test]
    fn unknown_core_indexes_are_rejected() {
        let (mut driver, _ram) = two_arm_cores();
        assert_eq!(driver.core_count(), 2);
        assert!(matches!(
            driver.attach_irq(9, Arc::new(IrqMailbox::new())),
            Err(MulticoreError::UnknownCore(9))
        ));
        assert!(matches!(
            driver.set_ps_per_instruction(9, NonZeroU64::new(1).unwrap()),
            Err(MulticoreError::UnknownCore(9))
        ));
        assert!(driver.core(2).is_none());
        assert!(driver.core(0).is_some());
    }

    #[cfg(feature = "unicorn-riscv")]
    #[test]
    fn heterogeneous_rounds_mix_arm_and_riscv_cores() {
        use mosaic_engine_unicorn::UnicornRiscV32;
        use mosaic_isa_riscv::Register as RvReg;

        const CPU_RV: NodeId = NodeId(2);
        const RAM_RV: NodeId = NodeId(12);
        let view0 = FlatView::build(&[
            leaf(0, 0x1000, RAM0, MemoryKind::Ram, Permissions::RWX),
            leaf(0x10000, 0x1000, SHARED, MemoryKind::Ram, Permissions::RW),
        ])
        .unwrap();
        let view_rv = FlatView::build(&[
            leaf(0, 0x1000, RAM_RV, MemoryKind::Ram, Permissions::RWX),
            leaf(0x10000, 0x1000, SHARED, MemoryKind::Ram, Permissions::RW),
        ])
        .unwrap();
        let mut machine = Machine::new(view0.clone(), 32);
        machine.add_view(CPU_RV, view_rv.clone()).unwrap();
        machine.add_backing(RAM0, 0x1000).unwrap();
        machine.add_backing(RAM_RV, 0x1000).unwrap();
        machine.add_backing(SHARED, 0x1000).unwrap();
        // ARM core: MOV r0,#1; B .
        machine
            .loader_write(RAM0, 0, &words(&[0xE3A0_0001, 0xEAFF_FFFE]))
            .unwrap();
        // RV32I core: lui x5,0x10; addi x6,x0,0x21; sw x6,0(x5); j .
        machine
            .loader_write(
                RAM_RV,
                0,
                &words(&[0x0001_02B7, 0x0210_0313, 0x0062_A023, 0x0000_006F]),
            )
            .unwrap();
        machine.verify().unwrap();
        let machine = Arc::new(Mutex::new(machine));

        let ram = SharedRam::new(0x1000).unwrap();
        let shared = [SharedRegion {
            node: SHARED,
            ram: Arc::clone(&ram),
        }];
        let mut arm = UnicornArm::new().unwrap();
        let mut riscv = UnicornRiscV32::new().unwrap();
        bind_flat_view_shared(&mut arm, &view0, Arc::clone(&machine), CPU0, &shared).unwrap();
        bind_flat_view_shared(&mut riscv, &view_rv, Arc::clone(&machine), CPU_RV, &shared).unwrap();
        arm.reg_write(mosaic_isa_arm::Register::Pc.into(), 0)
            .unwrap();
        riscv.reg_write(RvReg::Pc.into(), 0).unwrap();

        let mut driver = MulticoreDriver::new(machine);
        driver.add_core(Box::new(crate::SharedEngine::new(Box::new(arm))));
        driver.add_core(Box::new(crate::SharedEngine::new(Box::new(riscv))));
        let round = driver.step(NonZeroU64::new(4).unwrap(), 8).unwrap();
        assert_eq!(round.cores.len(), 2);
        assert_eq!(round.cores[0].engine.instruction_hooks, 2 + 2);
        assert_eq!(round.cores[1].engine.instruction_hooks, 4);
        let mut bytes = [0u8; 4];
        ram.read(0, &mut bytes).unwrap();
        assert_eq!(bytes, 0x21u32.to_le_bytes());
    }
}
