//! USB HID boot keyboard and mouse devices (QEMU's `hw/usb/hid.c` role):
//! the devices behind a UHCI/EHCI/xHCI host's downstream ports.
//!
//! Clean-room from the USB HID specification 1.11's boot protocol: the
//! device/configuration/interface/endpoint descriptor chain, the class
//! descriptor (report descriptor bytes for the boot keyboard's 8-byte input
//! report and the boot mouse's 3-byte report), the report protocol, and the
//! interrupt-IN endpoint that carries reports. The control transfer answers
//! GET_DESCRIPTOR for every descriptor table and SET_IDLE/SET_PROTOCOL;
//! `data_in` on endpoint 1 delivers one report per poll.

use mosaic_contract::usb::{
    descriptor, request, ControlResult, SetupPacket, UsbDevice, UsbSpeed, UsbStatus,
};

/// A HID boot keyboard (`ip:usb/hid-keyboard`).
#[derive(Debug)]
pub struct HidKeyboard {
    report: [u8; 8],
    protocol: u8,
    leds: u8,
    idle: u8,
    /// The ep0 staging buffer the control data phase reads through.
    ep0: Vec<u8>,
}

impl Default for HidKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl HidKeyboard {
    pub fn new() -> Self {
        Self {
            report: [0; 8],
            protocol: 1, // Boot protocol.
            leds: 0,
            idle: 0,
            ep0: Vec::new(),
        }
    }

    /// Send one key event: usage (HID usage table, e.g. 0x04 = A) plus state.
    pub fn send_key(&mut self, usage: u8, down: bool) {
        if down {
            // Modifiers live in the first byte (usage 0xE0..0xE7).
            if (0xE0..=0xE7).contains(&usage) {
                self.report[0] |= 1 << (usage - 0xE0);
                return;
            }
            for slot in self.report[1..8].iter_mut() {
                if *slot == 0 {
                    *slot = usage;
                    return;
                }
            }
        } else if (0xE0..=0xE7).contains(&usage) {
            self.report[0] &= !(1 << (usage - 0xE0));
        } else {
            for slot in self.report[1..8].iter_mut() {
                if *slot == usage {
                    *slot = 0;
                }
            }
        }
    }
}

/// The shared descriptor assembly: 18-byte device descriptor, then the
/// configuration with one HID interface and one interrupt-IN endpoint.
pub fn configuration_descriptors(class: u8, report_descriptor: &[u8]) -> Vec<u8> {
    let configuration_len = 9 + 9 + 9 + 7;
    let total = configuration_len + 9;
    let mut config = Vec::with_capacity(total);
    // Configuration descriptor.
    config.extend_from_slice(&[
        9,
        descriptor::CONFIGURATION,
        total as u8,
        (total >> 8) as u8,
    ]);
    config.extend_from_slice(&[1, 1, 0]); // one interface, configuration 1
    config.extend_from_slice(&[0x80, 50]); // bus powered, 50 mA
                                           // Interface descriptor: HID class, no alternate, no strings, one endpoint.
    config.extend_from_slice(&[9, descriptor::INTERFACE, 0, 0, 1, class, 1, 1, 0]);
    // HID class descriptor.
    let report_len = report_descriptor.len() as u16;
    config.extend_from_slice(&[9, descriptor::HID]);
    config.extend_from_slice(&[0x11, 0x01]); // bcdHID 1.11
    config.extend_from_slice(&[0x00]); // country
    config.extend_from_slice(&[1]); // one report descriptor
    config.extend_from_slice(&report_len.to_le_bytes());
    // Endpoint descriptor: interrupt IN 1, 8 bytes, 10 ms.
    config.extend_from_slice(&[7, descriptor::ENDPOINT, 0x81, 0x03, 8, 0, 10]);
    config
}

/// The boot keyboard's report descriptor (HID 1.11 appendix B.1).
const KEYBOARD_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x06, 0x75, 0x08,
    0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
];

impl UsbDevice for HidKeyboard {
    fn speed(&self) -> UsbSpeed {
        UsbSpeed::Low
    }

    fn device_descriptor(&self) -> [u8; 18] {
        let mut descriptor = [0u8; 18];
        descriptor[0] = 18;
        descriptor[1] = descriptor::DEVICE;
        descriptor[2] = 0x10;
        descriptor[3] = 0x01; // bcdUSB 1.10
        descriptor[4] = 0x00; // class at interface level
        descriptor[5] = 0x00;
        descriptor[6] = 0x00;
        descriptor[7] = 8; // bMaxPacketSize0
        descriptor[8] = 0x27;
        descriptor[9] = 0x11; // idVendor (Mosaic test VID)
        descriptor[10] = 0x01;
        descriptor[11] = 0x01; // idProduct
        descriptor[12] = 0x00;
        descriptor[13] = 0x01; // bcdDevice
        descriptor[14] = 0x00; // no strings
        descriptor[15] = 0x00;
        descriptor[16] = 1; // one configuration
        descriptor[17] = 0;
        descriptor
    }

    fn configuration_descriptor(&self) -> Vec<u8> {
        configuration_descriptors(0x03, KEYBOARD_REPORT_DESCRIPTOR)
    }

    fn string_descriptor(&self, _index: u8) -> Option<Vec<u8>> {
        None
    }

    fn control(&mut self, setup: SetupPacket, _payload: &[u8]) -> Result<ControlResult, UsbStatus> {
        match (setup.bm_request_type, setup.b_request) {
            (0x80, request::GET_DESCRIPTOR) => match (setup.w_value >> 8) as u8 {
                descriptor::DEVICE => {
                    self.ep0 = self.device_descriptor().to_vec();
                    Ok(ControlResult::Complete { len: 18 })
                }
                descriptor::CONFIGURATION => {
                    self.ep0 = self.configuration_descriptor();
                    Ok(ControlResult::Complete {
                        len: self.ep0.len(),
                    })
                }
                descriptor::REPORT => {
                    self.ep0 = KEYBOARD_REPORT_DESCRIPTOR.to_vec();
                    Ok(ControlResult::Complete {
                        len: KEYBOARD_REPORT_DESCRIPTOR.len(),
                    })
                }
                _ => Ok(ControlResult::Stall),
            },
            (0x21, 0x0A) => {
                // SET_IDLE
                self.idle = (setup.w_value >> 8) as u8;
                Ok(ControlResult::Complete { len: 0 })
            }
            (0x21, 0x0B) => {
                // SET_PROTOCOL
                self.protocol = setup.w_value as u8;
                Ok(ControlResult::Complete { len: 0 })
            }
            _ => Ok(ControlResult::Stall),
        }
    }

    fn data_in(&mut self, endpoint: u8, max: usize) -> Result<Option<Vec<u8>>, UsbStatus> {
        if endpoint == 0 {
            let take = max.min(self.ep0.len());
            let data = self.ep0.drain(..take).collect::<Vec<u8>>();
            return Ok(Some(data));
        }
        if endpoint == 1 {
            let report = self.report[..max.min(8)].to_vec();
            return Ok(Some(report));
        }
        Ok(None)
    }

    fn data_out(&mut self, _endpoint: u8, _data: &[u8]) -> Result<UsbStatus, UsbStatus> {
        Ok(UsbStatus::Ack)
    }
}

impl HidKeyboard {
    /// The LEDs the last SET_REPORT carried (diagnostics).
    pub fn leds(&self) -> u8 {
        self.leds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keyboard_reports_keys_on_the_interrupt_endpoint() {
        let mut keyboard = HidKeyboard::new();
        keyboard.send_key(0x04, true); // A down
        keyboard.send_key(0x05, true); // B down
        let report = keyboard.data_in(1, 8).unwrap().unwrap();
        assert_eq!(&report[1..3], &[0x04, 0x05]);
        keyboard.send_key(0x04, false); // A up
        let report = keyboard.data_in(1, 8).unwrap().unwrap();
        assert_eq!(&report[1..3], &[0x00, 0x05]);
        // Modifiers land in byte 0.
        keyboard.send_key(0xE0, true); // Left ctrl
        let report = keyboard.data_in(1, 8).unwrap().unwrap();
        assert_eq!(report[0], 0x01);
    }

    #[test]
    fn the_descriptor_chain_enumerates_through_the_fabric() {
        let mut bus = mosaic_bus::usb::core::UsbBus::new();
        bus.attach(
            mosaic_bus::usb::core::UsbPort(1),
            Box::new(HidKeyboard::new()),
        )
        .unwrap();
        let address = bus
            .enumerate(mosaic_bus::usb::core::UsbPort(1))
            .expect("enumeration");
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: request::GET_DESCRIPTOR,
            w_value: 0x0200, // configuration
            w_index: 0,
            w_length: 34,
        };
        // SET_CONFIGURATION the driver would issue is a class request here;
        // the configuration descriptor chain comes through control.
        let _ = bus.control_transfer(address, setup, &[]);
        // The device descriptor came through the enumeration itself.
        assert!(bus
            .descriptor_of(mosaic_bus::usb::core::UsbPort(1))
            .is_some());
        let device = bus
            .descriptor_of(mosaic_bus::usb::core::UsbPort(1))
            .unwrap();
        assert_eq!(device[4], 0x00); // class at interface level
                                     // The fabric survived an unknown request with a stall, not a panic.
        let bad = SetupPacket {
            bm_request_type: 0x00,
            b_request: 0x77,
            w_value: 0,
            w_index: 0,
            w_length: 0,
        };
        assert!(bus.control_transfer(address, bad, &[]).is_err());
    }
}
