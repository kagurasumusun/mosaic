//! USB bus fabric: port list, device addressing and transfer routing.
//!
//! Host controllers drive devices through the phases of each transfer type
//! as the USB 2.0 specification sequences them:
//!
//! * **control** — SETUP, zero or more DATA packets, STATUS. The fabric
//!   assembles the data phase across the (bounded) packet sizes and hands
//!   the whole request to the device, because a control request only means
//!   something as a whole;
//! * **bulk / interrupt** — single DATA packets per token, delivered by
//!   endpoint number and direction;
//! * **isochronous** — DATA without handshakes (delivered like bulk, minus
//!   the retry semantics).
//!
//! Device addressing (SET_ADDRESS) is fabric bookkeeping, exactly as the
//! spec puts it on the bus, not on the device — a device model never stores
//! its own bus address, so the same model enumerates identically on any
//! controller.

use std::collections::BTreeMap;

use mosaic_contract::usb::{ControlResult, SetupPacket, UsbDevice, UsbStatus};

/// One USB port a device can attach to (a root-hub port or, later, a hub
/// downstream port).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UsbPort(pub u8);

/// The default address every device answers before SET_ADDRESS.
pub const DEFAULT_ADDRESS: u8 = 0;

/// A USB bus with its attached devices.
pub struct UsbBus {
    /// Devices by port, so composition order is deterministic.
    ports: BTreeMap<UsbPort, Box<dyn UsbDevice>>,
    /// Device address -> port. Exactly the bus's own view of addressing.
    addresses: BTreeMap<u8, UsbPort>,
    next_address: u8,
}

impl Default for UsbBus {
    fn default() -> Self {
        Self::new()
    }
}

impl UsbBus {
    pub fn new() -> Self {
        Self {
            ports: BTreeMap::new(),
            addresses: BTreeMap::new(),
            next_address: 1,
        }
    }

    /// Attach a device to a port. It answers at [`DEFAULT_ADDRESS`] until the
    /// controller assigns one; attaching a second un-addressed device moves
    /// the default address to it, exactly as a fresh enumeration would.
    pub fn attach(&mut self, port: UsbPort, device: Box<dyn UsbDevice>) -> Result<(), UsbStatus> {
        if self.ports.contains_key(&port) {
            return Err(UsbStatus::NoResponse);
        }
        self.addresses.insert(DEFAULT_ADDRESS, port);
        self.ports.insert(port, device);
        Ok(())
    }

    /// Detach a device. Controllers observe this through their own port
    /// status registers; the bus just stops routing to the port.
    pub fn detach(&mut self, port: UsbPort) -> Option<Box<dyn UsbDevice>> {
        let device = self.ports.remove(&port)?;
        self.addresses.retain(|_address, owner| *owner != port);
        Some(device)
    }

    pub fn device_count(&self) -> usize {
        self.ports.len()
    }

    /// The address a port's device currently answers at (for diagnostics).
    pub fn address_of(&self, port: UsbPort) -> Option<u8> {
        self.addresses
            .iter()
            .find(|(_address, owner)| **owner == port)
            .map(|(address, _owner)| *address)
    }

    fn port_of(&self, address: u8) -> Option<UsbPort> {
        self.addresses.get(&address).copied()
    }

    fn device_mut(&mut self, address: u8) -> Option<&mut Box<dyn UsbDevice>> {
        let port = self.port_of(address)?;
        self.ports.get_mut(&port)
    }

    /// One control transfer to the device at `address`.
    ///
    /// `data_out` carries the complete host-to-device data phase (controllers
    /// assemble it before calling). The return value is the complete
    /// device-to-host data phase. `SET_ADDRESS` is answered by the fabric.
    pub fn control_transfer(
        &mut self,
        address: u8,
        setup: SetupPacket,
        data_out: &[u8],
    ) -> Result<Vec<u8>, UsbStatus> {
        // SET_ADDRESS belongs to the bus: the device never sees it, and the
        // fabric re-routes the port to the new address.
        if setup.bm_request_type == 0x00
            && setup.b_request == mosaic_contract::usb::request::SET_ADDRESS
        {
            let port = self.port_of(address).ok_or(UsbStatus::NoResponse)?;
            let assigned = (setup.w_value & 0x7F) as u8;
            if assigned == DEFAULT_ADDRESS {
                self.addresses.insert(DEFAULT_ADDRESS, port);
            } else {
                self.next_address = self.next_address.max(assigned.saturating_add(1));
                // Retire the default-address mapping for this port so the
                // device answers at its new address only.
                if self.addresses.get(&DEFAULT_ADDRESS) == Some(&port) {
                    self.addresses.remove(&DEFAULT_ADDRESS);
                }
                self.addresses.insert(assigned, port);
            }
            return Ok(Vec::new());
        }
        let device = self.device_mut(address).ok_or(UsbStatus::NoResponse)?;
        match device.control(setup, data_out)? {
            ControlResult::Stall => Err(UsbStatus::Stall),
            _ => {
                // A to-host control transfer's data phase comes back through
                // the default endpoint's IN direction.
                if setup.to_host() && setup.w_length > 0 {
                    Ok(device
                        .data_in(0, setup.w_length as usize)?
                        .unwrap_or_default())
                } else {
                    Ok(Vec::new())
                }
            }
        }
    }

    /// One IN data phase on a non-control endpoint (bulk/interrupt/iso).
    pub fn data_in(
        &mut self,
        address: u8,
        endpoint: u8,
        max: usize,
    ) -> Result<Option<Vec<u8>>, UsbStatus> {
        self.device_mut(address)
            .ok_or(UsbStatus::NoResponse)?
            .data_in(endpoint, max)
    }

    /// One OUT data phase on a non-control endpoint.
    pub fn data_out(
        &mut self,
        address: u8,
        endpoint: u8,
        data: &[u8],
    ) -> Result<UsbStatus, UsbStatus> {
        self.device_mut(address)
            .ok_or(UsbStatus::NoResponse)?
            .data_out(endpoint, data)
    }

    /// The standard enumeration sequence a host controller performs at
    /// attach: read the first 8 bytes of the device descriptor (to learn
    /// `bMaxPacketSize0`), assign an address, read the full descriptor.
    ///
    /// Returns the assigned address. Controllers and conformance tests use
    /// this instead of reimplementing the dance.
    pub fn enumerate(&mut self, _port: UsbPort) -> Result<u8, UsbStatus> {
        let first = self.control_transfer(
            DEFAULT_ADDRESS,
            SetupPacket {
                bm_request_type: 0x80,
                b_request: mosaic_contract::usb::request::GET_DESCRIPTOR,
                w_value: 0x0100, // DEVICE
                w_index: 0,
                w_length: 8,
            },
            &[],
        )?;
        if first.len() < 8 || first[0] != 18 {
            return Err(UsbStatus::NoResponse);
        }
        let assigned = self.next_address;
        self.control_transfer(
            DEFAULT_ADDRESS,
            SetupPacket {
                bm_request_type: 0x00,
                b_request: mosaic_contract::usb::request::SET_ADDRESS,
                w_value: u16::from(assigned),
                w_index: 0,
                w_length: 0,
            },
            &[],
        )?;
        let full = self.control_transfer(
            assigned,
            SetupPacket {
                bm_request_type: 0x80,
                b_request: mosaic_contract::usb::request::GET_DESCRIPTOR,
                w_value: 0x0100,
                w_index: 0,
                w_length: 18,
            },
            &[],
        )?;
        if full.len() < 18 {
            return Err(UsbStatus::NoResponse);
        }
        Ok(assigned)
    }

    /// Attach-time descriptor passthrough for controllers that report the
    /// device's class in their own port registers.
    pub fn descriptor_of(&self, port: UsbPort) -> Option<[u8; 18]> {
        self.ports
            .get(&port)
            .map(|device| device.device_descriptor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::usb::{descriptor, request, UsbSpeed};

    /// A minimal standard device that answers GET_DESCRIPTOR(device).
    struct TestDevice {
        ep0_in: Vec<u8>,
    }

    impl TestDevice {
        fn new() -> Self {
            Self { ep0_in: Vec::new() }
        }
    }

    fn device_descriptor_bytes() -> Vec<u8> {
        let mut d = vec![0u8; 18];
        d[0] = 18; // bLength
        d[1] = descriptor::DEVICE;
        d[4] = 0x00; // per-interface class
        d[7] = 8; // bMaxPacketSize0
        d[8] = 0x10; // bcdUSB low
        d[9] = 0x01; // bcdUSB high: 0110 = 1.10
        d
    }

    impl UsbDevice for TestDevice {
        fn speed(&self) -> UsbSpeed {
            UsbSpeed::Full
        }

        fn device_descriptor(&self) -> [u8; 18] {
            let mut bytes = [0u8; 18];
            bytes.copy_from_slice(&device_descriptor_bytes());
            bytes
        }

        fn configuration_descriptor(&self) -> Vec<u8> {
            Vec::new()
        }

        fn string_descriptor(&self, _index: u8) -> Option<Vec<u8>> {
            None
        }

        fn control(
            &mut self,
            setup: SetupPacket,
            _payload_in: &[u8],
        ) -> Result<ControlResult, UsbStatus> {
            if setup.bm_request_type == 0x80
                && setup.b_request == request::GET_DESCRIPTOR
                && setup.w_value == 0x0100
            {
                self.ep0_in = device_descriptor_bytes();
                return Ok(ControlResult::Complete {
                    len: self.ep0_in.len(),
                });
            }
            Ok(ControlResult::Stall)
        }

        fn data_in(&mut self, endpoint: u8, _max: usize) -> Result<Option<Vec<u8>>, UsbStatus> {
            if endpoint == 0 {
                return Ok(Some(std::mem::take(&mut self.ep0_in)));
            }
            Ok(None)
        }

        fn data_out(&mut self, _endpoint: u8, _data: &[u8]) -> Result<UsbStatus, UsbStatus> {
            Ok(UsbStatus::Ack)
        }
    }

    #[test]
    fn enumeration_assigns_an_address_and_reads_the_descriptor() {
        let mut bus = UsbBus::new();
        bus.attach(UsbPort(1), Box::new(TestDevice::new())).unwrap();
        let address = bus.enumerate(UsbPort(1)).unwrap();
        assert_eq!(address, 1);
        assert_eq!(bus.address_of(UsbPort(1)), Some(1));
        let descriptor = bus.descriptor_of(UsbPort(1)).unwrap();
        assert_eq!(descriptor[0], 18);
        assert_eq!(descriptor[1], descriptor::DEVICE);
    }

    #[test]
    fn set_address_moves_the_device_away_from_the_default() {
        let mut bus = UsbBus::new();
        bus.attach(UsbPort(1), Box::new(TestDevice::new())).unwrap();
        let address = bus.enumerate(UsbPort(1)).unwrap();
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: request::GET_DESCRIPTOR,
            w_value: 0x0100,
            w_index: 0,
            w_length: 18,
        };
        // The assigned address answers.
        assert!(bus.control_transfer(address, setup, &[]).is_ok());
    }

    #[test]
    fn an_unknown_request_stalls() {
        let mut bus = UsbBus::new();
        bus.attach(UsbPort(1), Box::new(TestDevice::new())).unwrap();
        let setup = SetupPacket {
            bm_request_type: 0x00,
            b_request: 0x99,
            w_value: 0,
            w_index: 0,
            w_length: 0,
        };
        assert_eq!(
            bus.control_transfer(DEFAULT_ADDRESS, setup, &[]),
            Err(UsbStatus::Stall)
        );
    }

    #[test]
    fn detach_removes_the_addressing() {
        let mut bus = UsbBus::new();
        bus.attach(UsbPort(1), Box::new(TestDevice::new())).unwrap();
        bus.enumerate(UsbPort(1)).unwrap();
        bus.detach(UsbPort(1));
        assert_eq!(bus.device_count(), 0);
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: request::GET_DESCRIPTOR,
            w_value: 0x0100,
            w_index: 0,
            w_length: 8,
        };
        assert_eq!(
            bus.control_transfer(DEFAULT_ADDRESS, setup, &[]),
            Err(UsbStatus::NoResponse)
        );
    }
}
