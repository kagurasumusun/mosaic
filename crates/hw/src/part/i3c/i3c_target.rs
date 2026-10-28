//! A generic MIPI I3C Basic-spec target (`ip:mipi/i3c-target`): the
//! spec-conformant peer every I3C fabric test and every future controller
//! model talks to, QEMU-parity-plus (QEMU mainline has no I3C at all).
//!
//! The model implements the target side of the specification: a 48-bit
//! provisional ID, a static address option (for SETDASA), the CCCs every
//! target must answer (GETMWL, GETMRL, GETPID, GETBCR, GETDCR, GETSTATUS),
//! SDR private register space (an RWL/RML-sized register bank), and the
//! vendor CCC range (0x9F) as an echo. It carries no board knowledge: the
//! board attaches it to the `mosaic-bus::i3c` fabric, which owns addressing.

use mosaic_contract::i3c::{ccc, Bcr, Ccc, I3cAddr, I3cError, I3cTarget, ProvisionalId};
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault};

/// The generic MIPI I3C target.
#[derive(Debug)]
pub struct I3cTargetModel {
    pid: ProvisionalId,
    /// Optional static address (I2C legacy address; SETDASA promotes it).
    static_address: Option<I3cAddr>,
    dynamic: Option<I3cAddr>,
    /// Max write length / max read length the target reports (spec minimums
    /// rounded to a bank).
    registers: Vec<u8>,
    /// Status word GETSTATUS answers.
    status: u16,
}

impl I3cTargetModel {
    /// A target with a 48-bit PID and an optional static address. The
    /// register bank is 8 bytes (MWL 8, MRL 8).
    pub fn new(pid: u64, static_address: Option<I3cAddr>) -> Self {
        Self {
            pid: ProvisionalId::new(pid, Bcr::simple(), 0x40),
            static_address,
            dynamic: None,
            registers: vec![0; 8],
            status: 0x0900, // Pending interrupts? none; SDR mode, activity idle.
        }
    }

    pub fn static_address(&self) -> Option<I3cAddr> {
        self.static_address
    }
}

impl I3cTarget for I3cTargetModel {
    fn provisional_id(&self) -> ProvisionalId {
        self.pid
    }

    fn dynamic_address(&self) -> Option<I3cAddr> {
        self.dynamic
    }

    fn set_dynamic_address(&mut self, addr: I3cAddr) {
        self.dynamic = Some(addr);
    }

    fn clear_dynamic_address(&mut self) {
        self.dynamic = None;
    }

    fn ccc(
        &mut self,
        _ctx: &mut dyn Ctx,
        ccc: Ccc,
        payload_in: &[u8],
    ) -> Result<Option<Vec<u8>>, I3cError> {
        match ccc.code {
            code if code == ccc::GETMWL.code => Ok(Some(vec![0x08, 0x00])), // 8 bytes
            code if code == ccc::GETMRL.code => Ok(Some(vec![0x08, 0x00])),
            code if code == ccc::GETPID.code => {
                let pid = self.pid.id;
                Ok(Some(pid.to_be_bytes()[2..8].to_vec()))
            }
            code if code == ccc::GETBCR.code => Ok(Some(vec![self.pid.bcr.0])),
            code if code == ccc::GETDCR.code => Ok(Some(vec![self.pid.dcr])),
            code if code == ccc::GETSTATUS.code => Ok(Some(self.status.to_be_bytes().to_vec())),
            code if code == ccc::RSTDAA.code => {
                self.dynamic = None;
                Ok(None)
            }
            code if code == ccc::SETMWL.code => {
                if payload_in.len() != 2 {
                    return Err(I3cError::UnsupportedCcc);
                }
                Ok(None)
            }
            code if code == ccc::SETMRL.code => {
                if payload_in.len() != 2 {
                    return Err(I3cError::UnsupportedCcc);
                }
                Ok(None)
            }
            0x9F => Ok(Some(payload_in.to_vec())), // vendor echo
            _ => Ok(None),
        }
    }

    fn sdr_write(&mut self, _ctx: &mut dyn Ctx, data: &[u8]) -> Result<(), I3cError> {
        if data.len() > self.registers.len() {
            return Err(I3cError::WriteOverflow);
        }
        for (index, byte) in data.iter().enumerate() {
            self.registers[index] = *byte;
        }
        Ok(())
    }

    fn sdr_read(
        &mut self,
        _ctx: &mut dyn Ctx,
        out: &mut Vec<u8>,
        max: usize,
    ) -> Result<(), I3cError> {
        let take = max.min(self.registers.len());
        if take < max {
            return Err(I3cError::ReadOverflow);
        }
        out.extend_from_slice(&self.registers[..take]);
        Ok(())
    }
}

impl MmioTarget for I3cTargetModel {
    /// A bare bus part: no register surface. A board that wires it as an
    /// I3C target maps nothing to it; the MmioTarget surface exists so the
    /// node registers like any part (read-as-zero until a part grows one).
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        _offset: u64,
        _size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        Ok(0)
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        _offset: u64,
        _size: AccessSize,
        _value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        Ok(())
    }
}

impl EventTarget for I3cTargetModel {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::model::ContextError;
    use mosaic_exec::EventId;
    use mosaic_exec::{SignalId, VTime};

    struct NoCtx;

    impl Ctx for NoCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }

        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }

        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }

        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }

    #[test]
    fn the_mandatory_cccs_answer_their_tables() {
        let mut target = I3cTargetModel::new(0xA000_0000_0112, None);
        let mut ctx = NoCtx;
        let mwl = target.ccc(&mut ctx, ccc::GETMWL, &[]).unwrap().unwrap();
        assert_eq!(mwl, vec![0x08, 0x00]);
        let pid = target.ccc(&mut ctx, ccc::GETPID, &[]).unwrap().unwrap();
        assert_eq!(pid, vec![0xA0, 0x00, 0x00, 0x00, 0x01, 0x12]);
        let bcr = target.ccc(&mut ctx, ccc::GETBCR, &[]).unwrap().unwrap();
        assert_eq!(bcr, vec![0x00]);
    }

    #[test]
    fn sdr_accesses_the_register_bank() {
        let mut target = I3cTargetModel::new(1, None);
        let mut ctx = NoCtx;
        target
            .sdr_write(&mut ctx, &[1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();
        let mut out = Vec::new();
        target.sdr_read(&mut ctx, &mut out, 8).unwrap();
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            target.sdr_write(&mut ctx, &[0; 9]),
            Err(I3cError::WriteOverflow)
        );
    }

    #[test]
    fn the_dynamic_address_follows_the_fabric() {
        let mut target = I3cTargetModel::new(1, Some(I3cAddr(0x50)));
        assert_eq!(target.static_address(), Some(I3cAddr(0x50)));
        assert_eq!(target.dynamic_address(), None);
        target.set_dynamic_address(I3cAddr(0x09));
        assert_eq!(target.dynamic_address(), Some(I3cAddr(0x09)));
    }
}
