//! MIPI I3C port vocabulary (transaction level).
//!
//! The I3C Basic specification defines a two-wire bus with *dynamic address
//! assignment* (DAA), common command codes (CCC) and SDR private transfers.
//! This contract fixes the transaction-level vocabulary a target model
//! implements; the wire-level fabric lives in `mosaic-bus::i3c`.
//!
//! A target cannot tell (and must not care) whether a controller model drove
//! it through the transaction-level [`I3cInitiator`] port or a bit-level
//! decoder reconstructed the same transfer from SDA/SCL edges.

use crate::model::Ctx;

/// 7-bit I3C dynamic address. `0x00` is the broadcast address and is never
/// assigned to a target.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct I3cAddr(pub u8);

impl I3cAddr {
    pub const BROADCAST: Self = Self(0x00);
}

/// Why an I3C transaction failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum I3cError {
    /// No target answers this dynamic address.
    NoAck,
    /// The target rejected the address (parity/vendor-specific).
    AddrRejected,
    /// A target stalled the bus past its max write length.
    WriteOverflow,
    /// More data was read than the target's max read length allows.
    ReadOverflow,
    /// A CCC the target does not implement.
    UnsupportedCcc,
    /// The transfer is invalid on this bus state (e.g. SDR to an
    /// unaddressed target).
    NotAddressed,
}

/// A common command code: `CCC` byte plus its payload phase.
///
/// The MSB of the code selects the addressing: 0 = broadcast (all targets),
/// 1 = direct (the dynamic address follows). Well-known codes are the
/// constants; vendor-defined codes are permitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ccc {
    pub code: u8,
}

impl Ccc {
    pub const fn broadcast(code: u8) -> Self {
        Self { code: code & 0x7F }
    }

    pub const fn direct(code: u8) -> Self {
        Self { code: code | 0x80 }
    }

    pub const fn is_broadcast(self) -> bool {
        self.code & 0x80 == 0
    }
}

/// Well-known broadcast CCCs (MIPI I3C Basic, CCC definitions).
pub mod ccc {
    use super::Ccc;

    /// Enable events (controller-managed interrupts/IBI).
    pub const ENEC: Ccc = Ccc::broadcast(0x00);
    /// Disable events.
    pub const DISEC: Ccc = Ccc::broadcast(0x01);
    /// Reset dynamic address acquired by all targets.
    pub const RSTDAA: Ccc = Ccc::broadcast(0x02);
    /// Enter activity state 0..3 (power management).
    pub const ENTAS0: Ccc = Ccc::broadcast(0x03);
    /// Enter dynamic address assignment.
    pub const ENTDAA: Ccc = Ccc::broadcast(0x07);
    /// Define list of targets (for the controller's own bookkeeping).
    pub const DEFTGTS: Ccc = Ccc::broadcast(0x08);
    /// Set max write length.
    pub const SETMWL: Ccc = Ccc::broadcast(0x09);
    /// Set max read length.
    pub const SETMRL: Ccc = Ccc::broadcast(0x0A);
    /// Enter test mode.
    pub const ENTTEST: Ccc = Ccc::broadcast(0x0B);

    /// Set dynamic address from the target's static address.
    pub const SETDASA: Ccc = Ccc::direct(0x87);
    /// Set a new dynamic address.
    pub const SETNEWDA: Ccc = Ccc::direct(0x88);
    /// Get max write length.
    pub const GETMWL: Ccc = Ccc::direct(0x8B);
    /// Get max read length.
    pub const GETMRL: Ccc = Ccc::direct(0x8C);
    /// Get provisional ID (48-bit PID from DAA).
    pub const GETPID: Ccc = Ccc::direct(0x8D);
    /// Get bus characteristics register.
    pub const GETBCR: Ccc = Ccc::direct(0x8E);
    /// Get device characteristics register.
    pub const GETDCR: Ccc = Ccc::direct(0x8F);
    /// Get status word.
    pub const GETSTATUS: Ccc = Ccc::direct(0x90);
    /// Get activity/ACCR (activity current consumption report).
    pub const GETACCR: Ccc = Ccc::direct(0x91);
}

/// Bus characteristics register (BCR) as a target publishes it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Bcr(pub u8);

impl Bcr {
    /// A plain I3C target, no IBI, max data speed SDR, legacy support off.
    pub const fn simple() -> Self {
        Self(0x00)
    }

    /// Bit 0-1: IBI capability format; bit 2: IBI request capable.
    pub const fn ibi_capable() -> Self {
        Self(0x04)
    }

    /// Bit 3: offline capable (I2C-like incbuf).
    pub const fn offline_capable() -> Self {
        Self(0x08)
    }

    /// Bit 5: max data speed limit bit (0 = SDR allowed).
    pub fn is_max_speed_sdr(self) -> bool {
        self.0 & 0x20 == 0
    }
}

/// What one I3C target reports when the controller asks during DAA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProvisionalId {
    /// 48-bit PID: vendor part number etc., as the target hardwires it.
    pub id: u64,
    /// Bus characteristics register.
    pub bcr: Bcr,
    /// Device characteristics register.
    pub dcr: u8,
}

impl ProvisionalId {
    pub const fn new(id: u64, bcr: Bcr, dcr: u8) -> Self {
        Self { id, bcr, dcr }
    }

    pub fn matches(&self, id: u64) -> bool {
        self.id == id
    }
}

/// One I3C target on the bus. Implemented by device models in `hw`;
/// addressed through the fabric by controller models.
pub trait I3cTarget: Send {
    /// The provisional ID this target answers DAA with. Deterministic per
    /// model instance: two simulations of the same composition assign the
    /// same addresses in the same order.
    fn provisional_id(&self) -> ProvisionalId;

    /// The dynamic address the DAA (or SETDASA) gave this target.
    fn dynamic_address(&self) -> Option<I3cAddr>;

    /// Assign or re-assign the dynamic address (SETNEWDA / DAA / SETDASA).
    fn set_dynamic_address(&mut self, addr: I3cAddr);

    /// Drop the dynamic address (RSTDAA).
    fn clear_dynamic_address(&mut self);

    /// One CCC. `payload_in` is the written data; the return value is the
    /// data the target drives during a read CCC (None when the CCC has no
    /// read phase).
    fn ccc(
        &mut self,
        ctx: &mut dyn Ctx,
        ccc: Ccc,
        payload_in: &[u8],
    ) -> Result<Option<Vec<u8>>, I3cError>;

    /// One SDR private write after addressing.
    fn sdr_write(&mut self, ctx: &mut dyn Ctx, data: &[u8]) -> Result<(), I3cError>;

    /// One SDR private read after addressing. Append up to `max` bytes.
    fn sdr_read(
        &mut self,
        ctx: &mut dyn Ctx,
        out: &mut Vec<u8>,
        max: usize,
    ) -> Result<(), I3cError>;
}

/// The controller-side port a bus-facing model speaks (the same relation
/// `I2cInitiator` has to `I2cTarget`).
pub trait I3cInitiator {
    fn entdaa(&mut self) -> Result<Vec<I3cAddr>, I3cError>;
    fn rstdaa(&mut self) -> Result<(), I3cError>;
    fn setdasa(&mut self, static_addr: I3cAddr) -> Result<I3cAddr, I3cError>;
    fn ccc_broadcast(
        &mut self,
        ccc: Ccc,
        payload: &[u8],
    ) -> Result<Vec<Result<(), I3cError>>, I3cError>;
    fn ccc_direct(
        &mut self,
        addr: I3cAddr,
        ccc: Ccc,
        payload: &[u8],
    ) -> Result<Option<Vec<u8>>, I3cError>;
    fn sdr_write(&mut self, addr: I3cAddr, data: &[u8]) -> Result<(), I3cError>;
    fn sdr_read(&mut self, addr: I3cAddr, max: usize) -> Result<Vec<u8>, I3cError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ccc_addressing_is_the_msb() {
        assert!(Ccc::broadcast(0x07).is_broadcast());
        assert!(!Ccc::direct(0x87).is_broadcast());
        assert_eq!(Ccc::broadcast(0x87).code, 0x07);
        assert_eq!(Ccc::direct(0x07).code, 0x87);
    }

    #[test]
    fn broadcast_is_never_assigned() {
        assert_eq!(I3cAddr::BROADCAST, I3cAddr(0));
    }
}
