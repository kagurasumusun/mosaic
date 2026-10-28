//! TI TCA9548A 8-channel I2C bus switch — the responsibility-first
//! composite part (ADR 0031 §3, `part/mux`).
//!
//! Clean-room from the TCA9548A data sheet (SCDS147): the upstream bus
//! sees one target at a strap-selected control address (0x70..0x77); a
//! single write byte loads the control register whose bit *n* connects the
//! upstream pins to downstream segment *n*. While a segment is connected,
//! the switch is transparent: frames addressed to a downstream device are
//! re-driven on that segment as if the master sat on it directly.
//!
//! Fabric cooperation (the pattern every mux-shaped part follows):
//!
//! * upstream — the switch is an [`I2cTarget`] whose `matches` unions the
//!   control address with every *connected* segment's reachable addresses,
//!   exactly how `mosaic-bus::i2c` documents mux parts ("they extend the
//!   reachable addresses");
//! * downstream — each of the 8 segments is an ordinary shared
//!   [`I2cBus`] the board wires parts into (`channel_bus`), and forwarded
//!   frames go through the segment's own [`I2cInitiator`] port, so a
//!   downstream target cannot tell whether a controller or a switch
//!   re-drove the frame.
//!
//! Functional-model simplifications (declared, timing-free):
//!
//! * rise/fall propagation delays and the 400 kHz upstream limit are not
//!   modeled — the fabric owns timing policy;
//! * multiple connected segments merge electrically on the real part;
//!   here a write reaches every connected segment that resolves the
//!   address, and a read is served by the first such segment;
//! * reset releases every segment (`0x00`), and the control register
//!   survives no bus error — a failed write leaves it unchanged.

use std::sync::{Arc, Mutex};

use mosaic_bus::i2c::core::I2cBus;
use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cInitiator, I2cTarget};
use mosaic_contract::model::Ctx;

/// Number of downstream segments the switch fans out to.
pub const CHANNELS: usize = 8;

/// The lowest strap address (`A2..A0` all low).
pub const BASE_ADDRESS: u8 = 0x70;

/// A TCA9548A switch: one upstream target, eight downstream segments.
pub struct Tca9548a {
    control_addr: I2cAddr,
    /// Bit n = segment n connected (the whole control register).
    control: u8,
    segments: [Arc<Mutex<I2cBus>>; CHANNELS],
}

impl Tca9548a {
    /// A switch answering at `base`. The strap pins select exactly
    /// 0x70..=0x77 (data sheet §8.5.2 table: the 1110xxx pool); anything
    /// else is a wiring mistake, not a reserved-address technicality.
    /// Every segment starts disconnected, matching the power-on state.
    pub fn new(base: u8) -> Result<Self, I2cError> {
        if !(BASE_ADDRESS..=0x77).contains(&base) {
            return Err(I2cError::BadAddress);
        }
        Ok(Self {
            control_addr: I2cAddr::seven(base)?,
            control: 0x00,
            segments: Default::default(),
        })
    }

    /// Segment *n*'s bus, for board wiring: attach the downstream parts
    /// here, the way a board hangs its sensors off the switch's channels.
    pub fn channel_bus(&self, channel: usize) -> Arc<Mutex<I2cBus>> {
        Arc::clone(&self.segments[verify_channel(channel)])
    }

    /// The control register as written (bit n = channel n enabled).
    pub fn control(&self) -> u8 {
        self.control
    }

    fn is_control_address(&self, addr: &I2cAddr) -> bool {
        addr == &self.control_addr
    }

    /// Segments whose bit is set in the control register.
    fn connected_segments(&self) -> impl Iterator<Item = usize> + '_ {
        (0..CHANNELS).filter(move |n| self.control & (1 << n) != 0)
    }

    /// Segments that are connected *and* hold a target answering `addr`.
    fn resolving_segments(&self, addr: &I2cAddr) -> Vec<usize> {
        if self.is_control_address(addr) {
            return Vec::new();
        }
        self.connected_segments()
            .filter(|&n| self.segments[n].lock().unwrap().resolves(addr))
            .collect()
    }
}

fn verify_channel(channel: usize) -> usize {
    assert!(channel < CHANNELS, "TCA9548A channel out of range");
    channel
}

impl I2cTarget for Tca9548a {
    fn matches(&self, addr: &I2cAddr) -> bool {
        self.is_control_address(addr) || !self.resolving_segments(addr).is_empty()
    }

    fn target_write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        if self.is_control_address(addr) {
            // One register, one byte: bit n connects segment n. A zero-length
            // write is the master probing for the switch; ACK it and change
            // nothing (data sheet §8.5.2 writes exactly one byte).
            if let Some(&byte) = bytes.first() {
                self.control = byte;
            }
            return Ok(Ack::Ack);
        }
        let segments = self.resolving_segments(addr);
        if segments.is_empty() {
            return Err(I2cError::Nack);
        }
        // The connected wires are shared on the real part: the frame is
        // re-driven on every resolving segment and the switch ACKs when
        // any of them answers.
        let mut answered = false;
        for n in segments {
            let mut segment = self.segments[n].lock().unwrap();
            if segment.write(ctx, *addr, bytes).is_ok() {
                answered = true;
            }
        }
        if answered {
            Ok(Ack::Ack)
        } else {
            Err(I2cError::Nack)
        }
    }

    fn target_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError> {
        if self.is_control_address(addr) {
            // Reads return the control register for as long as the master
            // keeps clocking (data sheet §8.5.3).
            for (i, slot) in buf.iter_mut().enumerate() {
                *slot = self.control.wrapping_add(i as u8);
            }
            return Ok(Ack::Ack);
        }
        let segments = self.resolving_segments(addr);
        // On the real wires every connected segment drives the read; the
        // functional model serves it from the first segment that holds the
        // addressed device, which is what a single-master probe observes.
        if let Some(&n) = segments.first() {
            let mut segment = self.segments[n].lock().unwrap();
            if segment.read(ctx, *addr, buf).is_ok() {
                return Ok(Ack::Ack);
            }
        }
        Err(I2cError::Nack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    /// Recorded downstream frames: (address, bytes).
    type Frames = std::sync::Arc<Mutex<Vec<(u8, Vec<u8>)>>>;

    /// A downstream target that records the frames it saw and answers
    /// reads from a fixed pattern.
    struct Probe {
        addr: u8,
        writes: Frames,
        reads: std::sync::Arc<Mutex<Vec<(u8, usize)>>>,
    }

    impl I2cTarget for Probe {
        fn matches(&self, addr: &I2cAddr) -> bool {
            *addr == I2cAddr::seven(self.addr).unwrap()
        }

        fn target_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            bytes: &[u8],
        ) -> Result<Ack, I2cError> {
            self.writes
                .lock()
                .unwrap()
                .push((self.addr, bytes.to_vec()));
            Ok(Ack::Ack)
        }

        fn target_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            buf: &mut [u8],
        ) -> Result<Ack, I2cError> {
            self.reads.lock().unwrap().push((self.addr, buf.len()));
            for (i, slot) in buf.iter_mut().enumerate() {
                *slot = 0xA0u8.wrapping_add(i as u8);
            }
            Ok(Ack::Ack)
        }
    }

    fn switch_with_probe_on(channel: usize, probe_addr: u8) -> (Tca9548a, Frames) {
        let mux = Tca9548a::new(BASE_ADDRESS).unwrap();
        let writes: Frames = std::sync::Arc::new(Mutex::new(Vec::new()));
        let probe = Probe {
            addr: probe_addr,
            writes: Arc::clone(&writes),
            reads: Default::default(),
        };
        mux.channel_bus(channel)
            .lock()
            .unwrap()
            .add_target(Box::new(probe));
        (mux, writes)
    }

    #[test]
    fn reset_disconnects_every_segment() {
        let (mux, _) = switch_with_probe_on(3, 0x21);
        assert_eq!(mux.control(), 0x00);
        let mut ctx = TestCtx::default();
        // Without a control write the segment is not reachable: the mux
        // itself only matches its control address.
        let mut upstream = I2cBus::new();
        upstream.add_target(Box::new(mux));
        let probe_addr = I2cAddr::seven(0x21).unwrap();
        assert!(!upstream.resolves(&probe_addr));
        assert!(matches!(
            upstream.write(&mut ctx, probe_addr, &[0x00]),
            Err(I2cError::Nack)
        ));
    }

    #[test]
    fn control_register_is_written_and_read_back() {
        let (mux, _) = switch_with_probe_on(2, 0x21);
        let mut upstream = I2cBus::new();
        upstream.add_target(Box::new(mux));
        let mut ctx = TestCtx::default();
        let control_addr = I2cAddr::seven(BASE_ADDRESS).unwrap();
        upstream.write(&mut ctx, control_addr, &[1u8 << 2]).unwrap();
        let mut readback = [0u8; 2];
        upstream
            .read(&mut ctx, control_addr, &mut readback)
            .unwrap();
        assert_eq!(readback, [1u8 << 2, (1u8 << 2).wrapping_add(1)]);
    }

    #[test]
    fn a_connected_segment_extends_the_reachable_addresses() {
        let (mux, writes) = switch_with_probe_on(0, 0x48);
        let mut upstream = I2cBus::new();
        upstream.add_target(Box::new(mux));
        let probe_addr = I2cAddr::seven(0x48).unwrap();

        let mut ctx = TestCtx::default();
        let control_addr = I2cAddr::seven(BASE_ADDRESS).unwrap();
        assert!(!upstream.resolves(&probe_addr));
        upstream.write(&mut ctx, control_addr, &[1u8 << 0]).unwrap();
        assert!(upstream.resolves(&probe_addr));

        // A write to the probe's address goes through the switch.
        upstream.write(&mut ctx, probe_addr, &[0x12, 0x34]).unwrap();
        assert_eq!(*writes.lock().unwrap(), vec![(0x48, vec![0x12, 0x34])]);

        // A read comes back from the probe through the same path.
        let mut data = [0u8; 2];
        upstream.read(&mut ctx, probe_addr, &mut data).unwrap();
        assert_eq!(data, [0xA0, 0xA1]);
    }

    #[test]
    fn disconnecting_drops_the_extension_and_reconnect_restores_it() {
        let (mux, _) = switch_with_probe_on(5, 0x2A);
        let mut upstream = I2cBus::new();
        upstream.add_target(Box::new(mux));
        let probe_addr = I2cAddr::seven(0x2A).unwrap();
        let control_addr = I2cAddr::seven(BASE_ADDRESS).unwrap();
        let mut ctx = TestCtx::default();

        upstream.write(&mut ctx, control_addr, &[1u8 << 5]).unwrap();
        assert!(upstream.resolves(&probe_addr));
        upstream.write(&mut ctx, control_addr, &[0x00]).unwrap();
        assert!(!upstream.resolves(&probe_addr));
        assert!(matches!(
            upstream.write(&mut ctx, probe_addr, &[0xFF]),
            Err(I2cError::Nack)
        ));
        upstream.write(&mut ctx, control_addr, &[1u8 << 5]).unwrap();
        assert!(upstream.resolves(&probe_addr));
    }

    #[test]
    fn both_connected_segments_receive_one_write() {
        let mux = Tca9548a::new(BASE_ADDRESS).unwrap();
        let writes_a = std::sync::Arc::new(Mutex::new(Vec::new()));
        let writes_b = std::sync::Arc::new(Mutex::new(Vec::new()));
        for (channel, writes) in [
            (1usize, Arc::clone(&writes_a)),
            (6usize, Arc::clone(&writes_b)),
        ] {
            mux.channel_bus(channel)
                .lock()
                .unwrap()
                .add_target(Box::new(Probe {
                    addr: 0x3C,
                    writes,
                    reads: Default::default(),
                }));
        }
        let mut upstream = I2cBus::new();
        upstream.add_target(Box::new(mux));
        let mut ctx = TestCtx::default();
        upstream
            .write(
                &mut ctx,
                I2cAddr::seven(BASE_ADDRESS).unwrap(),
                &[(1u8 << 1) | (1u8 << 6)],
            )
            .unwrap();
        let sensor = I2cAddr::seven(0x3C).unwrap();
        upstream.write(&mut ctx, sensor, &[0xDE]).unwrap();
        assert_eq!(*writes_a.lock().unwrap(), vec![(0x3C, vec![0xDE])]);
        assert_eq!(*writes_b.lock().unwrap(), vec![(0x3C, vec![0xDE])]);
    }

    #[test]
    fn the_control_address_still_reaches_the_switch_while_segments_are_connected() {
        let (mux, _) = switch_with_probe_on(7, 0x5D);
        let mut upstream = I2cBus::new();
        upstream.add_target(Box::new(mux));
        let mut ctx = TestCtx::default();
        let control_addr = I2cAddr::seven(BASE_ADDRESS).unwrap();
        upstream.write(&mut ctx, control_addr, &[1u8 << 7]).unwrap();
        let mut back = [0u8; 1];
        upstream.read(&mut ctx, control_addr, &mut back).unwrap();
        assert_eq!(back, [1u8 << 7]);
    }

    #[test]
    fn a_zero_length_control_write_is_an_acknowledged_probe() {
        let (mux, _) = switch_with_probe_on(4, 0x19);
        let mut upstream = I2cBus::new();
        upstream.add_target(Box::new(mux));
        let mut ctx = TestCtx::default();
        let control_addr = I2cAddr::seven(BASE_ADDRESS).unwrap();
        upstream.write(&mut ctx, control_addr, &[1u8 << 4]).unwrap();
        upstream.write(&mut ctx, control_addr, &[]).unwrap();
        let mut back = [0u8; 1];
        upstream.read(&mut ctx, control_addr, &mut back).unwrap();
        assert_eq!(back, [1u8 << 4]);
    }

    #[test]
    fn strap_addresses_across_the_legal_range() {
        assert!(matches!(Tca9548a::new(0x6F), Err(I2cError::BadAddress)));
        for base in 0x70..=0x77 {
            assert!(Tca9548a::new(base).is_ok(), "base {base:#x}");
        }
        assert!(matches!(Tca9548a::new(0x78), Err(I2cError::BadAddress)));
    }
}
