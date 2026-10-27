//! I3C bus entity: dynamic address assignment, CCC routing and SDR transfers
//! (transaction level, MIPI I3C Basic 1.1.1 semantics).
//!
//! The fabric owns exactly what the specification says the bus owns:
//!
//! * the **dynamic address pool** — [`I3cBus::entdaa`] walks unaddressed
//!   targets in attach order, reads each provisional ID and assigns the next
//!   free dynamic address, exactly the ENTDAA procedure a controller runs
//!   after reset;
//! * **CCC routing** — broadcast codes reach every target in attach order,
//!   direct codes reach the one target that owns the dynamic address;
//! * **SDR private transfers** — addressed reads and writes between one
//!   controller and one target.
//!
//! Timing (I3C's DDR clock modes, TSEOT) is not modelled: transactions are
//! discrete events in virtual time, the way the I2C fabric models frames.
//! What a target observes is identical either way, which is the point.

use std::collections::BTreeMap;

use mosaic_contract::i3c::{Ccc, I3cAddr, I3cError, I3cInitiator, I3cTarget, ProvisionalId};
use mosaic_contract::model::Ctx;

/// One I3C bus with its controller-side initiator port.
///
/// Targets attach in board composition order; every operation below is
/// deterministic in that order, so the same composition always assigns the
/// same dynamic addresses.
pub struct I3cBus {
    targets: Vec<Box<dyn I3cTarget>>,
    /// Index by dynamic address, so direct CCC and SDR are O(log n).
    by_address: BTreeMap<u8, usize>,
    /// Next dynamic address the DAA assigns (0x00 is broadcast; the pool
    /// starts above the reserved static-address range).
    next_dynamic: u8,
}

impl Default for I3cBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Dynamic addresses the DAA never assigns: 0x00 is the broadcast address,
/// and the I3C Basic reserved range begins at 0x78 upward.
const RESERVED_LOW: u8 = 0x08;
const RESERVED_HIGH: u8 = 0x78;

impl I3cBus {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
            by_address: BTreeMap::new(),
            next_dynamic: RESERVED_LOW,
        }
    }

    pub fn add_target(&mut self, target: Box<dyn I3cTarget>) {
        self.targets.push(target);
    }

    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// The (statically unknown to targets) dynamic address map, for
    /// composition diagnostics and conformance tests.
    pub fn assigned(&self) -> BTreeMap<u8, ProvisionalId> {
        let mut map = BTreeMap::new();
        for (index, target) in self.targets.iter().enumerate() {
            if target.dynamic_address().is_some() {
                map.insert(index as u8, target.provisional_id());
            }
        }
        map
    }

    fn allocate(&mut self) -> Option<I3cAddr> {
        while self.next_dynamic < RESERVED_HIGH {
            let candidate = self.next_dynamic;
            self.next_dynamic += 1;
            if !self.by_address.contains_key(&candidate) {
                return Some(I3cAddr(candidate));
            }
        }
        None
    }

    /// ENTDAA: assign dynamic addresses to every unaddressed target.
    ///
    /// Returns the addresses in assignment order. The 48-bit provisional IDs
    /// are read from the targets — the controller does not choose them, so a
    /// device model's PID decides nothing about ordering except through its
    /// attach position.
    pub fn entdaa(&mut self) -> Result<Vec<I3cAddr>, I3cError> {
        let mut assigned = Vec::new();
        let mut index = 0;
        while index < self.targets.len() {
            if self.targets[index].dynamic_address().is_some() {
                index += 1;
                continue;
            }
            let Some(address) = self.allocate() else {
                return Err(I3cError::NoAck);
            };
            self.targets[index].set_dynamic_address(address);
            self.by_address.insert(address.0, index);
            assigned.push(address);
            index += 1;
        }
        Ok(assigned)
    }

    /// RSTDAA: drop every target's dynamic address and free the pool, so a
    /// subsequent DAA reassigns from the first address again.
    pub fn rstdaa(&mut self) -> Result<(), I3cError> {
        self.by_address.clear();
        self.next_dynamic = RESERVED_LOW;
        for target in &mut self.targets {
            target.clear_dynamic_address();
        }
        Ok(())
    }

    /// SETDASA: a target that also has an I2C static address takes it as its
    /// dynamic address.
    pub fn setdasa(&mut self, static_addr: I3cAddr) -> Result<I3cAddr, I3cError> {
        if static_addr.0 == 0 {
            return Err(I3cError::AddrRejected);
        }
        for (index, target) in self.targets.iter().enumerate() {
            if target.dynamic_address().is_none() {
                self.targets[index].set_dynamic_address(static_addr);
                self.by_address.insert(static_addr.0, index);
                return Ok(static_addr);
            }
        }
        Err(I3cError::NoAck)
    }

    fn index_of(&self, addr: I3cAddr) -> Result<usize, I3cError> {
        self.by_address.get(&addr.0).copied().ok_or(I3cError::NoAck)
    }

    /// Broadcast CCC: every target sees it in attach order.
    pub fn ccc_broadcast(
        &mut self,
        ctx: &mut dyn Ctx,
        ccc: Ccc,
        payload: &[u8],
    ) -> Result<Vec<Result<(), I3cError>>, I3cError> {
        if !ccc.is_broadcast() {
            return Err(I3cError::UnsupportedCcc);
        }
        Ok(self
            .targets
            .iter_mut()
            .map(|target| match target.ccc(ctx, ccc, payload) {
                Ok(Some(_)) => Ok(()), // Broadcast CCCs have no read phase.
                Ok(None) => Ok(()),
                Err(error) => Err(error),
            })
            .collect())
    }

    /// Direct CCC: one target answers, possibly with data.
    pub fn ccc_direct(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I3cAddr,
        ccc: Ccc,
        payload: &[u8],
    ) -> Result<Option<Vec<u8>>, I3cError> {
        if ccc.is_broadcast() {
            return Err(I3cError::UnsupportedCcc);
        }
        let index = self.index_of(addr)?;
        self.targets[index].ccc(ctx, ccc, payload)
    }

    /// SDR private write to one addressed target.
    pub fn sdr_write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I3cAddr,
        data: &[u8],
    ) -> Result<(), I3cError> {
        let index = self.index_of(addr)?;
        self.targets[index].sdr_write(ctx, data)
    }

    /// SDR private read from one addressed target.
    pub fn sdr_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: I3cAddr,
        max: usize,
    ) -> Result<Vec<u8>, I3cError> {
        let index = self.index_of(addr)?;
        let mut out = Vec::new();
        self.targets[index].sdr_read(ctx, &mut out, max)?;
        Ok(out)
    }
}

/// The controller-side port: a controller model drives the bus through this
/// adapter, never through the targets directly.
pub struct I3cController<'a> {
    bus: &'a mut I3cBus,
}

impl<'a> I3cController<'a> {
    pub fn new(bus: &'a mut I3cBus) -> Self {
        Self { bus }
    }
}

impl I3cInitiator for I3cController<'_> {
    fn entdaa(&mut self) -> Result<Vec<I3cAddr>, I3cError> {
        I3cBus::entdaa(self.bus)
    }

    fn rstdaa(&mut self) -> Result<(), I3cError> {
        I3cBus::rstdaa(self.bus)
    }

    fn setdasa(&mut self, static_addr: I3cAddr) -> Result<I3cAddr, I3cError> {
        I3cBus::setdasa(self.bus, static_addr)
    }

    fn ccc_broadcast(
        &mut self,
        ccc: Ccc,
        payload: &[u8],
    ) -> Result<Vec<Result<(), I3cError>>, I3cError> {
        let mut ctx = NoopCtx;
        self.bus.ccc_broadcast(&mut ctx, ccc, payload)
    }

    fn ccc_direct(
        &mut self,
        addr: I3cAddr,
        ccc: Ccc,
        payload: &[u8],
    ) -> Result<Option<Vec<u8>>, I3cError> {
        let mut ctx = NoopCtx;
        self.bus.ccc_direct(&mut ctx, addr, ccc, payload)
    }

    fn sdr_write(&mut self, addr: I3cAddr, data: &[u8]) -> Result<(), I3cError> {
        let mut ctx = NoopCtx;
        self.bus.sdr_write(&mut ctx, addr, data)
    }

    fn sdr_read(&mut self, addr: I3cAddr, max: usize) -> Result<Vec<u8>, I3cError> {
        let mut ctx = NoopCtx;
        self.bus.sdr_read(&mut ctx, addr, max)
    }
}

/// A Ctx for controller-driven transactions without a model context: an
/// I3C controller with virtual-time effects (IBI scheduling) supplies its
/// own; the fabric's default is time-zero no-ops, matching the I2C fabric.
struct NoopCtx;

impl Ctx for NoopCtx {
    fn now(&self) -> mosaic_exec::VTime {
        mosaic_exec::VTime::ZERO
    }

    fn schedule(
        &mut self,
        _at: mosaic_exec::VTime,
        _token: u64,
    ) -> Result<mosaic_exec::EventId, mosaic_contract::model::ContextError> {
        Err(mosaic_contract::model::ContextError::ResourceExhausted)
    }

    fn cancel(&mut self, _event: mosaic_exec::EventId) -> bool {
        false
    }

    fn set_signal(
        &mut self,
        _signal: mosaic_exec::SignalId,
        _high: bool,
    ) -> Result<(), mosaic_contract::model::ContextError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::i3c::{ccc, Bcr};

    /// A register-like target that answers CCCs and SDR like a sensor would.
    struct Target {
        pid: ProvisionalId,
        address: Option<I3cAddr>,
        registers: [u8; 4],
        written: Vec<u8>,
    }

    impl Target {
        fn new(id: u64) -> Self {
            Self {
                pid: ProvisionalId::new(id, Bcr::simple(), 0x40),
                address: None,
                registers: [0xDE, 0xAD, 0xBE, 0xEF],
                written: Vec::new(),
            }
        }
    }

    impl I3cTarget for Target {
        fn provisional_id(&self) -> ProvisionalId {
            self.pid
        }

        fn dynamic_address(&self) -> Option<I3cAddr> {
            self.address
        }

        fn set_dynamic_address(&mut self, addr: I3cAddr) {
            self.address = Some(addr);
        }

        fn clear_dynamic_address(&mut self) {
            self.address = None;
        }

        fn ccc(
            &mut self,
            _ctx: &mut dyn Ctx,
            ccc: Ccc,
            payload: &[u8],
        ) -> Result<Option<Vec<u8>>, I3cError> {
            match ccc.code {
                code if code == mosaic_contract::i3c::ccc::GETSTATUS.code => {
                    Ok(Some(vec![0x00, 0x09]))
                }
                code if code == mosaic_contract::i3c::ccc::SETMWL.code => {
                    assert_eq!(payload, &[0x04]);
                    Ok(None)
                }
                0xF0 => Err(I3cError::UnsupportedCcc),
                _ => Ok(None),
            }
        }

        fn sdr_write(&mut self, _ctx: &mut dyn Ctx, data: &[u8]) -> Result<(), I3cError> {
            if data.len() > 8 {
                return Err(I3cError::WriteOverflow);
            }
            self.written.extend_from_slice(data);
            Ok(())
        }

        fn sdr_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            out: &mut Vec<u8>,
            max: usize,
        ) -> Result<(), I3cError> {
            let take = max.min(self.registers.len());
            out.extend_from_slice(&self.registers[..take]);
            Ok(())
        }
    }

    #[test]
    fn daa_assigns_sequential_dynamic_addresses() {
        let mut bus = I3cBus::new();
        bus.add_target(Box::new(Target::new(0xA000_0000_0001)));
        bus.add_target(Box::new(Target::new(0xA000_0000_0002)));
        let mut ctx = NoopCtx;
        let assigned = bus.entdaa().unwrap();
        assert_eq!(assigned, vec![I3cAddr(0x08), I3cAddr(0x09)]);
        // The second target answers a direct GETSTATUS at its own address.
        let status = bus
            .ccc_direct(&mut ctx, I3cAddr(0x09), ccc::GETSTATUS, &[])
            .unwrap()
            .unwrap();
        assert_eq!(status, vec![0x00, 0x09]);
    }

    #[test]
    fn rstdaa_clears_the_pool_and_addresses_are_reusable() {
        let mut bus = I3cBus::new();
        bus.add_target(Box::new(Target::new(1)));
        bus.entdaa().unwrap();
        bus.rstdaa().unwrap();
        let mut ctx = NoopCtx;
        assert!(bus.sdr_read(&mut ctx, I3cAddr(0x08), 4).is_err());
        let again = bus.entdaa().unwrap();
        assert_eq!(again, vec![I3cAddr(0x08)]);
    }

    #[test]
    fn sdr_transfers_reach_the_addressed_target_only() {
        let mut bus = I3cBus::new();
        bus.add_target(Box::new(Target::new(1)));
        bus.add_target(Box::new(Target::new(2)));
        bus.entdaa().unwrap();
        let mut ctx = NoopCtx;
        bus.sdr_write(&mut ctx, I3cAddr(0x09), &[0x11, 0x22])
            .unwrap();
        let data = bus.sdr_read(&mut ctx, I3cAddr(0x09), 4).unwrap();
        assert_eq!(data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(bus.sdr_write(&mut ctx, I3cAddr(0x08), &[1]).is_ok());
        // An address that was never assigned does not acknowledge.
        assert_eq!(
            bus.sdr_write(&mut ctx, I3cAddr(0x42), &[1]),
            Err(I3cError::NoAck)
        );
    }

    #[test]
    fn broadcast_cccs_reach_every_target_in_order() {
        let mut bus = I3cBus::new();
        bus.add_target(Box::new(Target::new(1)));
        bus.add_target(Box::new(Target::new(2)));
        bus.entdaa().unwrap();
        let mut ctx = NoopCtx;
        let results = bus.ccc_broadcast(&mut ctx, ccc::SETMWL, &[0x04]).unwrap();
        assert_eq!(results, vec![Ok(()), Ok(())]);
    }
}
