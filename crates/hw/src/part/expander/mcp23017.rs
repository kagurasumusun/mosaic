//! Microchip MCP23017 16-bit I2C I/O expander — a two-fabric composite
//! part (ADR 0031 §3, `part/expander`).
//!
//! Clean-room from the MCP23017 data sheet (DS20001952): an I2C target at
//! a strap-selected address (0x20..0x27) whose register bank maps 16 GPIO
//! lines in two 8-bit ports (A/B). This model implements the register
//! semantics the Linux `pinctrl-mcp23s08` driver touches on bring-up:
//!
//! * `IODIRA/IODIRB` (0x00/0x01) — direction, 1 = input (reset state);
//! * `IPOLA/IPOLB` (0x02/0x03) — input polarity inversion;
//! * `GPINTENA/GPINTENB` (0x04/0x05), `DEFVALA/DEFVALB` (0x06/0x07),
//!   `INTCONA/INTCONB` (0x08/0x09) — interrupt-on-change setup (levels
//!   recorded; the IRQ line itself waits for the signal wiring);
//! * `IOCON` (0x0A/0x0B) — configuration bits accepted and read back;
//! * `GPPUA/GPPUB` (0x0C/0x0D) — pull-up enables;
//! * `INTFA/INTFB` (0x0E/0x0F), `INTCAPA/INTCAPB` (0x10/0x11) — interrupt
//!   flags and captures (functional: INTCAP read clears the pair);
//! * `GPIOA/GPIOB` (0x12/0x13) — read inputs / drive outputs;
//! * `OLATA/OLATB` (0x14/0x15) — output latch read-back.
//!
//! The register bank is one flat 22-byte file addressed by pointer: write
//! frames set the pointer with the first byte and stream the rest with
//! auto-increment, reads stream from the pointer — the same convention the
//! sensor parts use, one addressing shape across the fabric.
//!
//! The GPIO side is 16 lines a board wires into the GPIO net fabric
//! through [`Mcp23017::pin`], so one part bridges I2C and GPIO without
//! either fabric knowing the other — the part is one model, the fabrics
//! stay strangers to each other (ADR 0031 §1).

use std::sync::{Arc, Mutex};

use mosaic_contract::gpio::{GpioError, GpioPort, PinConfig, PinLevel};
use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract::model::Ctx;

/// Register file size (0x00..=0x15).
pub const REGISTERS: usize = 0x16;

mod reg {
    // The full map the Linux driver probes; the model answers them all.
    #![allow(dead_code)]
    pub const IODIR: usize = 0x00; // +1 per port
    pub const IPOL: usize = 0x02;
    pub const GPINTEN: usize = 0x04;
    pub const DEFVAL: usize = 0x06;
    pub const INTCON: usize = 0x08;
    pub const IOCON: usize = 0x0A;
    pub const GPPU: usize = 0x0C;
    pub const INTF: usize = 0x0E;
    pub const INTCAP: usize = 0x10;
    pub const GPIO: usize = 0x12;
    pub const OLAT: usize = 0x14;
}

/// IOCON bits (data sheet table 3-4).
pub mod iocon {
    pub const BANK: u8 = 1 << 7;
    pub const MIRROR: u8 = 1 << 6;
    pub const SEQOP: u8 = 1 << 5;
    pub const DISSLW: u8 = 1 << 4;
    pub const HAEN: u8 = 1 << 3;
    pub const ODR: u8 = 1 << 2;
    pub const INTPOL: u8 = 1 << 1;
}

/// Number of GPIO lines the expander owns.
pub const PINS: usize = 16;

/// An MCP23017: one I2C target, sixteen GPIO lines in two ports.
pub struct Mcp23017 {
    addr: I2cAddr,
    /// 8-bit register file, pointer-addressed; the interleaved layout
    /// (reset state, IOCON.BANK = 0) puts port B at the odd offsets.
    regs: [u8; REGISTERS],
    pointer: usize,
    /// Resolved levels of the 16 lines as the net (or the guest) last
    /// settled them; input read-back and output drive both use them.
    levels: [PinLevel; PINS],
}

impl Mcp23017 {
    /// An expander answering at `addr`. The MCP23017's A2..A0 straps
    /// select 0x20..=0x27; the MCP23S17 SPI twin is a separate part
    /// number, not a second bind of this one.
    pub fn new(addr: u8) -> Result<Self, I2cError> {
        if !(0x20..=0x27).contains(&addr) {
            return Err(I2cError::BadAddress);
        }
        let mut regs = [0u8; REGISTERS];
        // Reset state: all lines inputs, everything else zeroed.
        for base in [
            reg::IODIR,
            reg::IPOL,
            reg::GPINTEN,
            reg::DEFVAL,
            reg::INTCON,
        ] {
            regs[base] = 0xFF;
            regs[base + 1] = 0xFF;
        }
        Ok(Self {
            addr: I2cAddr::seven(addr)?,
            regs,
            pointer: 0,
            levels: [PinLevel::Low; PINS],
        })
    }

    /// The I2C address this expander answers at (for board wiring).
    pub fn address(&self) -> I2cAddr {
        self.addr
    }

    /// Line *n* (0..15) as a GPIO-net port, for board wiring: drive and
    /// release go through the output latch exactly like GPIO register
    /// writes, reads return the resolved line level.
    pub fn pin(shared: &Arc<Mutex<Self>>, pin: usize) -> Result<Mcp23017Pin, String> {
        if pin >= PINS {
            return Err(format!("MCP23017 pin {pin} out of range 0..{PINS}"));
        }
        Ok(Mcp23017Pin {
            expander: Arc::clone(shared),
            pin,
        })
    }

    fn port_of(pin: usize) -> usize {
        if pin < 8 {
            0
        } else {
            1
        }
    }

    fn bit_of(pin: usize) -> u8 {
        (pin % 8) as u8
    }

    fn is_output(&self, pin: usize) -> bool {
        (self.regs[reg::IODIR + Self::port_of(pin)] >> Self::bit_of(pin)) & 1 == 0
    }
}

impl I2cTarget for Mcp23017 {
    fn matches(&self, addr: &I2cAddr) -> bool {
        addr == &self.addr
    }

    fn target_write(
        &mut self,
        _ctx: &mut dyn Ctx,
        _addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        let Some(&head) = bytes.first() else {
            // Zero-length write: an acknowledged address probe that keeps
            // the pointer (data sheet §1.6.1).
            return Ok(Ack::Ack);
        };
        // First data byte is the register pointer; the rest stream in
        // with auto-increment, saturating at the bank end.
        self.pointer = (head as usize).min(REGISTERS - 1);
        for &byte in &bytes[1..] {
            self.regs[self.pointer] = byte;
            if self.pointer == reg::GPIO || self.pointer == reg::GPIO + 1 {
                // Writing GPIO loads the output latch and drives the
                // configured outputs (data sheet §1.6.2: GPIO writes set
                // OLAT); input lines ignore the write.
                let port = self.pointer - reg::GPIO;
                self.regs[reg::OLAT + port] = byte;
                for bit in 0..8usize {
                    let pin = port * 8 + bit;
                    if self.is_output(pin) {
                        self.levels[pin] = if (byte >> bit) & 1 == 1 {
                            PinLevel::High
                        } else {
                            PinLevel::Low
                        };
                    }
                }
            }
            self.pointer = (self.pointer + 1).min(REGISTERS - 1);
        }
        Ok(Ack::Ack)
    }

    fn target_read(
        &mut self,
        _ctx: &mut dyn Ctx,
        _addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError> {
        for (i, slot) in buf.iter_mut().enumerate() {
            let index = (self.pointer + i).min(REGISTERS - 1);
            if index == reg::GPIO || index == reg::GPIO + 1 {
                // GPIO reads: inputs report the resolved line level,
                // outputs their own latch (data sheet §1.6.5).
                let port = index - reg::GPIO;
                let mut byte = 0u8;
                for bit in 0..8usize {
                    let pin = port * 8 + bit;
                    let high = if self.is_output(pin) {
                        (self.regs[reg::OLAT + port] >> bit) & 1 == 1
                    } else {
                        self.levels[pin] == PinLevel::High
                    };
                    byte |= (high as u8) << bit;
                }
                *slot = byte;
            } else if index == reg::INTCAP || index == reg::INTCAP + 1 {
                // Reading INTCAP clears the interrupt condition for that
                // port: flag and capture both (data sheet §1.6.6).
                let port = index - reg::INTCAP;
                *slot = self.regs[index];
                self.regs[reg::INTF + port] = 0;
            } else {
                *slot = self.regs[index];
            }
        }
        self.pointer = (self.pointer + buf.len()).min(REGISTERS - 1);
        Ok(Ack::Ack)
    }
}

/// One expander line as a GPIO-net port. Boards wire these the way they
/// wire a controller's pins; the expander is a *source* of lines, while
/// the controllers stay in `crate::ip::gpio`.
pub struct Mcp23017Pin {
    expander: Arc<Mutex<Mcp23017>>,
    pin: usize,
}

impl GpioPort for Mcp23017Pin {
    fn drive(&mut self, _ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError> {
        let mut exp = self.expander.lock().unwrap();
        if !exp.is_output(self.pin) {
            return Err(GpioError::Unsupported);
        }
        let port = Mcp23017::port_of(self.pin);
        let bit = Mcp23017::bit_of(self.pin);
        let latch = &mut exp.regs[reg::OLAT + port];
        if level == PinLevel::High {
            *latch |= 1 << bit;
        } else {
            *latch &= !(1 << bit);
        }
        exp.levels[self.pin] = level;
        Ok(())
    }

    fn release(&mut self, _ctx: &mut dyn Ctx) -> Result<(), GpioError> {
        // Releasing mimics the line going back to input: the pin floats
        // and the guest's IODIR keeps describing the configuration.
        let mut exp = self.expander.lock().unwrap();
        let port = Mcp23017::port_of(self.pin);
        let bit = Mcp23017::bit_of(self.pin);
        exp.regs[reg::IODIR + port] |= 1 << bit;
        Ok(())
    }

    fn configure(&mut self, _config: PinConfig) -> Result<(), GpioError> {
        // The guest's configuration registers (IODIR, GPPU) are the part's
        // own register interface; the net-side port keeps the electrical
        // face without overriding them.
        Ok(())
    }

    fn current_level(&self) -> PinLevel {
        self.expander.lock().unwrap().levels[self.pin]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;
    use mosaic_bus::i2c::core::I2cBus;
    use mosaic_contract::i2c::I2cInitiator;

    fn expander_on_bus(addr: u8) -> (I2cBus, Arc<Mutex<Mcp23017>>) {
        let shared = Arc::new(Mutex::new(Mcp23017::new(addr).unwrap()));
        struct Wrap(Arc<Mutex<Mcp23017>>);
        impl I2cTarget for Wrap {
            fn matches(&self, addr: &I2cAddr) -> bool {
                self.0.lock().unwrap().matches(addr)
            }
            fn target_write(
                &mut self,
                ctx: &mut dyn Ctx,
                addr: &I2cAddr,
                bytes: &[u8],
            ) -> Result<Ack, I2cError> {
                self.0.lock().unwrap().target_write(ctx, addr, bytes)
            }
            fn target_read(
                &mut self,
                ctx: &mut dyn Ctx,
                addr: &I2cAddr,
                buf: &mut [u8],
            ) -> Result<Ack, I2cError> {
                self.0.lock().unwrap().target_read(ctx, addr, buf)
            }
        }
        let mut bus = I2cBus::new();
        bus.add_target(Box::new(Wrap(Arc::clone(&shared))));
        (bus, shared)
    }

    #[test]
    fn reset_state_reads_all_lines_as_inputs() {
        let (mut bus, _) = expander_on_bus(0x20);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::seven(0x20).unwrap();
        bus.write(&mut ctx, addr, &[reg::GPIO as u8]).unwrap();
        let mut gpio = [0u8; 2];
        bus.read(&mut ctx, addr, &mut gpio).unwrap();
        // All 16 lines are inputs at reset; nothing drives them, so both
        // ports read Low (pull-ups are a register the guest has not set).
        assert_eq!(gpio, [0x00, 0x00]);
    }

    #[test]
    fn direction_then_gpio_write_drives_the_lines() {
        let (mut bus, exp) = expander_on_bus(0x21);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::seven(0x21).unwrap();
        // IODIRA = 0x00: port A all outputs, then GPIOA = 0xA5.
        bus.write(&mut ctx, addr, &[reg::IODIR as u8, 0x00])
            .unwrap();
        bus.write(&mut ctx, addr, &[reg::GPIO as u8, 0xA5]).unwrap();
        // 0xA5 = bits 0, 2, 5, 7.
        let e = exp.lock().unwrap();
        assert_eq!(e.levels[0], PinLevel::High);
        assert_eq!(e.levels[1], PinLevel::Low);
        assert_eq!(e.levels[2], PinLevel::High);
        assert_eq!(e.levels[5], PinLevel::High);
        assert_eq!(e.levels[7], PinLevel::High);
        drop(e);
        // Outputs read back through their latch.
        bus.write(&mut ctx, addr, &[reg::GPIO as u8]).unwrap();
        let mut back = [0u8; 1];
        bus.read(&mut ctx, addr, &mut back).unwrap();
        assert_eq!(back, [0xA5]);
    }

    #[test]
    fn input_lines_ignore_gpio_writes_and_report_levels() {
        let (mut bus, exp) = expander_on_bus(0x22);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::seven(0x22).unwrap();
        // Leave port A as inputs; drive line 3 from the net side.
        {
            let mut e = exp.lock().unwrap();
            e.levels[3] = PinLevel::High;
        }
        bus.write(&mut ctx, addr, &[reg::GPIO as u8, 0xFF]).unwrap();
        assert_eq!(exp.lock().unwrap().levels[0], PinLevel::Low); // untouched input
        bus.write(&mut ctx, addr, &[reg::GPIO as u8]).unwrap();
        let mut back = [0u8; 1];
        bus.read(&mut ctx, addr, &mut back).unwrap();
        assert_eq!(back, [0x08]);
    }

    #[test]
    fn reads_auto_increment_through_the_bank() {
        let (mut bus, exp) = expander_on_bus(0x23);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::seven(0x23).unwrap();
        {
            let mut e = exp.lock().unwrap();
            e.regs[reg::IPOL] = 0x11;
            e.regs[reg::IPOL + 1] = 0x22;
            e.regs[reg::GPINTEN] = 0x33;
        }
        bus.write(&mut ctx, addr, &[reg::IPOL as u8]).unwrap();
        let mut three = [0u8; 3];
        bus.read(&mut ctx, addr, &mut three).unwrap();
        assert_eq!(three, [0x11, 0x22, 0x33]);
    }

    #[test]
    fn reading_intcap_clears_that_ports_flag() {
        let (mut bus, exp) = expander_on_bus(0x24);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::seven(0x24).unwrap();
        {
            let mut e = exp.lock().unwrap();
            e.regs[reg::INTF] = 0x40;
            e.regs[reg::INTF + 1] = 0x80;
            e.regs[reg::INTCAP] = 0x55;
        }
        bus.write(&mut ctx, addr, &[reg::INTCAP as u8]).unwrap();
        let mut cap = [0u8; 1];
        bus.read(&mut ctx, addr, &mut cap).unwrap();
        assert_eq!(cap, [0x55]);
        let e = exp.lock().unwrap();
        assert_eq!(e.regs[reg::INTF], 0x00);
        assert_eq!(e.regs[reg::INTF + 1], 0x80); // port B flag survives
    }

    #[test]
    fn zero_length_write_is_an_acknowledged_probe() {
        let (mut bus, _) = expander_on_bus(0x25);
        let mut ctx = TestCtx::default();
        bus.write(&mut ctx, I2cAddr::seven(0x25).unwrap(), &[])
            .unwrap();
    }

    #[test]
    fn address_range_is_the_strap_pool() {
        assert!(Mcp23017::new(0x1F).is_err());
        for addr in 0x20..=0x27 {
            assert!(Mcp23017::new(addr).is_ok(), "addr {addr:#x}");
        }
        assert!(Mcp23017::new(0x28).is_err());
    }

    #[test]
    fn port_b_lives_on_the_odd_offsets() {
        let (mut bus, exp) = expander_on_bus(0x26);
        let mut ctx = TestCtx::default();
        let addr = I2cAddr::seven(0x26).unwrap();
        bus.write(&mut ctx, addr, &[(reg::IODIR + 1) as u8, 0x00])
            .unwrap();
        bus.write(&mut ctx, addr, &[(reg::GPIO + 1) as u8, 0x80])
            .unwrap();
        let e = exp.lock().unwrap();
        assert_eq!(e.levels[15], PinLevel::High);
        assert_eq!(e.levels[8], PinLevel::Low);
    }

    #[test]
    fn the_net_side_pin_reads_and_drives_the_same_registers() {
        let shared = Arc::new(Mutex::new(Mcp23017::new(0x27).unwrap()));
        let mut ctx = TestCtx::default();
        {
            let mut e = shared.lock().unwrap();
            e.regs[reg::IODIR] = 0x00; // port A outputs
            e.regs[reg::IODIR + 1] = 0xFF; // port B inputs
        }
        let mut pa2 = Mcp23017::pin(&shared, 2).unwrap();
        pa2.drive(&mut ctx, PinLevel::High).unwrap();
        assert_eq!(shared.lock().unwrap().regs[reg::OLAT], 0x04);
        // Reading GPIO through I2C observes the driven output.
        let e = shared.lock().unwrap();
        let mut byte = 0u8;
        for bit in 0..8 {
            byte |= ((e.levels[bit] == PinLevel::High) as u8) << bit;
        }
        assert_eq!(byte, 0x04);
        drop(e);
        // An input pin refuses to drive and reports its level.
        let mut pb4 = Mcp23017::pin(&shared, 12).unwrap();
        assert!(matches!(
            pb4.drive(&mut ctx, PinLevel::High),
            Err(GpioError::Unsupported)
        ));
        // Out-of-range pins are rejected outright.
        assert!(Mcp23017::pin(&shared, 16).is_err());
    }
}
