#![forbid(unsafe_code)]
//! Intel 8254 Programmable Interval Timer (PIT) model for x86.

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError, VDelta};

#[derive(Clone, Debug, PartialEq, Eq)]
struct PitCounterState {
    count: u16,
    reload: u16,
    latched_count: u16,
    count_latched: bool,
    read_state: u8,  // 0=LSB, 1=MSB
    write_state: u8, // 0=LSB, 1=MSB
    access_mode: u8, // 1=LSB, 2=MSB, 3=LSB then MSB
    mode: u8,        // 0..5
    bcd: bool,
    gate: bool,
    output: bool,
}

impl Default for PitCounterState {
    fn default() -> Self {
        Self {
            count: 0,
            reload: 0,
            latched_count: 0,
            count_latched: false,
            read_state: 0,
            write_state: 0,
            access_mode: 3,
            mode: 0,
            bcd: false,
            gate: true,
            output: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pit8254State {
    pub schema: u32,
    counters: [PitCounterState; 3],
}

pub struct Pit8254 {
    state: Pit8254State,
}

impl Pit8254 {
    pub fn new() -> Self {
        Self {
            state: Pit8254State {
                schema: 1,
                counters: [
                    PitCounterState::default(),
                    PitCounterState::default(),
                    PitCounterState::default(),
                ],
            },
        }
    }

    fn read_counter(&mut self, channel: usize) -> u8 {
        if channel >= 3 {
            return 0xFF;
        }
        let counter = &mut self.state.counters[channel];
        let val = if counter.count_latched {
            counter.latched_count
        } else {
            counter.count
        };

        let byte = match counter.access_mode {
            1 => val as u8,        // LSB
            2 => (val >> 8) as u8, // MSB
            3 => {
                // LSB then MSB
                if counter.read_state == 0 {
                    counter.read_state = 1;
                    val as u8
                } else {
                    counter.read_state = 0;
                    if counter.count_latched {
                        counter.count_latched = false;
                    }
                    (val >> 8) as u8
                }
            }
            _ => 0,
        };
        byte
    }

    fn write_counter(&mut self, ctx: &mut dyn Ctx, channel: usize, val: u8) {
        if channel >= 3 {
            return;
        }
        let counter = &mut self.state.counters[channel];
        match counter.access_mode {
            1 => {
                counter.reload = u16::from(val);
                self.load_count(ctx, channel);
            }
            2 => {
                counter.reload = u16::from(val) << 8;
                self.load_count(ctx, channel);
            }
            3 => {
                if counter.write_state == 0 {
                    counter.reload = (counter.reload & 0xFF00) | u16::from(val);
                    counter.write_state = 1;
                } else {
                    counter.reload = (counter.reload & 0x00FF) | (u16::from(val) << 8);
                    counter.write_state = 0;
                    self.load_count(ctx, channel);
                }
            }
            _ => {}
        }
    }

    fn write_control(&mut self, val: u8) {
        let channel = (val >> 6) as usize;
        if channel == 3 {
            // Read-back command or invalid
            return;
        }
        let access = (val >> 4) & 0x03;
        let mode = (val >> 1) & 0x07;
        let bcd = (val & 0x01) != 0;

        let counter = &mut self.state.counters[channel];
        if access == 0 {
            // Counter latch command
            if !counter.count_latched {
                counter.latched_count = counter.count;
                counter.count_latched = true;
            }
        } else {
            counter.access_mode = access;
            counter.mode = if mode > 5 { mode - 4 } else { mode };
            counter.bcd = bcd;
            counter.read_state = 0;
            counter.write_state = 0;
        }
    }

    fn load_count(&mut self, ctx: &mut dyn Ctx, channel: usize) {
        let counter = &mut self.state.counters[channel];
        let count = if counter.reload == 0 {
            0x10000
        } else {
            u64::from(counter.reload)
        };
        counter.count = counter.reload;

        if channel == 0 {
            // Schedule PIT IRQ 0 tick
            // Base frequency = 1193182 Hz -> period = ~838 ns per count
            let period_ps = count * 838_095;
            let _ = ctx.schedule_after(VDelta(period_ps), 0);
        }
    }
}

impl Default for Pit8254 {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for Pit8254 {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = match offset {
            0x00 => self.read_counter(0),
            0x01 => self.read_counter(1),
            0x02 => self.read_counter(2),
            _ => 0xFF,
        };
        Ok(u64::from(val))
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        let val = value as u8;
        match offset {
            0x00 => self.write_counter(ctx, 0, val),
            0x01 => self.write_counter(ctx, 1, val),
            0x02 => self.write_counter(ctx, 2, val),
            0x03 => self.write_control(val),
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for Pit8254 {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        if token == 0 {
            // Toggle IRQ0 / tick
            let _ = ctx.set_signal(SignalId(0), true);
            let _ = ctx.set_signal(SignalId(0), false);
            let reload = self.state.counters[0].reload;
            let count = if reload == 0 {
                0x10000
            } else {
                u64::from(reload)
            };
            let period_ps = count * 838_095;
            let _ = ctx.schedule_after(VDelta(period_ps), 0);
        }
        Ok(())
    }
}

impl Snapshot for Pit8254 {
    type State = Pit8254State;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        self.state.clone()
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.state = state.clone();
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = mosaic_core_types::Fnv64::new();
        hash.u32(version);
        hash.u32(state.schema);
        hash.u16(state.counters[0].reload);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_core_types::{AccessKind, Endian, NodeId, Security};

    fn test_attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: false,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0xFF,
            endian: Endian::Little,
        }
    }

    #[test]
    fn pit8254_counter0_write_and_read() {
        let mut pit = Pit8254::new();
        let mut dummy = DummyCtx;
        let attr = test_attr();
        // Control: Channel 0, LSB then MSB, Mode 2
        pit.write(&mut dummy, 0x03, AccessSize::Byte, 0x34, &attr)
            .unwrap();
        pit.write(&mut dummy, 0x00, AccessSize::Byte, 0x12, &attr)
            .unwrap();
        pit.write(&mut dummy, 0x00, AccessSize::Byte, 0x34, &attr)
            .unwrap();
        assert_eq!(pit.state.counters[0].reload, 0x3412);
    }

    struct DummyCtx;
    impl Ctx for DummyCtx {
        fn now(&self) -> mosaic_core_types::VTime {
            mosaic_core_types::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_core_types::VTime,
            _token: u64,
        ) -> Result<mosaic_core_types::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_core_types::EventId) -> bool {
            false
        }
        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }
}
