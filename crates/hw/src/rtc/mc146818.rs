//! Motorola MC146818 RTC & CMOS RAM model.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtcMc146818State {
    pub schema: u32,
    index: u8,
    cmos_data: [u8; 128],
}

pub struct RtcMc146818 {
    state: RtcMc146818State,
}

impl RtcMc146818 {
    pub fn new() -> Self {
        let mut cmos = [0u8; 128];
        // Standard CMOS defaults
        cmos[0x0A] = 0x26; // Status A: 32.768kHz, 1024Hz periodic
        cmos[0x0B] = 0x02; // Status B: 24h format, BCD
        cmos[0x0D] = 0x80; // Status D: Valid battery / CMOS

        // Default initial time (e.g. 2026-01-01 00:00:00 in BCD)
        cmos[0x00] = 0x00; // seconds
        cmos[0x02] = 0x00; // minutes
        cmos[0x04] = 0x00; // hours
        cmos[0x07] = 0x01; // day of month
        cmos[0x08] = 0x01; // month
        cmos[0x09] = 0x26; // year

        Self {
            state: RtcMc146818State {
                schema: 1,
                index: 0,
                cmos_data: cmos,
            },
        }
    }
}

impl Default for RtcMc146818 {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioTarget for RtcMc146818 {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let val = match offset {
            0x00 => u64::from(self.state.index),
            0x01 => {
                let idx = (self.state.index & 0x7F) as usize;
                let data = self.state.cmos_data[idx];
                if idx == 0x0C {
                    // Status C read clears interrupt flags
                    self.state.cmos_data[0x0C] = 0;
                }
                u64::from(data)
            }
            _ => 0xFF,
        };
        Ok(val)
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        let val = value as u8;
        match offset {
            0x00 => {
                self.state.index = val;
            }
            0x01 => {
                let idx = (self.state.index & 0x7F) as usize;
                if idx != 0x0C && idx != 0x0D {
                    self.state.cmos_data[idx] = val;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for RtcMc146818 {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        // Periodic RTC interrupt
        self.state.cmos_data[0x0C] |= 0xC0; // Interrupt request + periodic interrupt
        let _ = ctx.set_signal(SignalId(0), true);
        Ok(())
    }
}

impl Snapshot for RtcMc146818 {
    type State = RtcMc146818State;
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
        let mut hash = mosaic_exec::Fnv64::new();
        hash.u32(version);
        hash.u32(state.schema);
        hash.byte(state.index);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::{AccessKind, Endian, NodeId, Security};

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
    fn rtc_read_write_cmos() {
        let mut rtc = RtcMc146818::new();
        let mut dummy = DummyCtx;
        let attr = test_attr();
        rtc.write(&mut dummy, 0x00, AccessSize::Byte, 0x15, &attr)
            .unwrap();
        rtc.write(&mut dummy, 0x01, AccessSize::Byte, 0x42, &attr)
            .unwrap();
        assert_eq!(
            rtc.read(&mut dummy, 0x01, AccessSize::Byte, &attr).unwrap(),
            0x42
        );
    }

    struct DummyCtx;
    impl Ctx for DummyCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_exec::VTime,
            _token: u64,
        ) -> Result<mosaic_exec::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_exec::EventId) -> bool {
            false
        }
        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }
}
