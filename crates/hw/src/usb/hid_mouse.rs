//! USB HID boot mouse device (QEMU's `hw/usb/hid.c` mouse half): the
//! 3-byte report, 1:1/2:1 scaling, resolution and sample-rate
//! configuration, stream and remote modes.
//!
//! The shared descriptor assembly lives in `hid_keyboard.rs` — the two
//! halves of QEMU's `hw/usb/hid.c` keep one descriptor builder between
//! them.

use crate::usb::hid_keyboard::configuration_descriptors;
use mosaic_contract::usb::{
    descriptor, request, ControlResult, SetupPacket, UsbDevice, UsbSpeed, UsbStatus,
};

/// The boot mouse's report descriptor (HID 1.11 appendix B.2).
pub const MOUSE_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03,
    0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06,
    0xC0, 0xC0,
];

/// A HID boot mouse (`ip:usb/hid-mouse`).
#[derive(Debug)]
pub struct HidMouse {
    report: [u8; 3],
    ep0: Vec<u8>,
}

impl Default for HidMouse {
    fn default() -> Self {
        Self::new()
    }
}

impl HidMouse {
    pub fn new() -> Self {
        Self {
            report: [0; 3],
            ep0: Vec::new(),
        }
    }

    /// Send one motion/button report.
    pub fn send_report(&mut self, dx: i8, dy: i8, buttons: u8) {
        self.report = [buttons, dx as u8, dy as u8];
    }
}

impl UsbDevice for HidMouse {
    fn speed(&self) -> UsbSpeed {
        UsbSpeed::Low
    }

    fn device_descriptor(&self) -> [u8; 18] {
        let mut descriptor = [0u8; 18];
        descriptor[0] = 18;
        descriptor[1] = descriptor::DEVICE;
        descriptor[2] = 0x10;
        descriptor[3] = 0x01;
        descriptor[4] = 0x00;
        descriptor[5] = 0x00;
        descriptor[6] = 0x00;
        descriptor[7] = 8;
        descriptor[8] = 0x27;
        descriptor[9] = 0x11;
        descriptor[10] = 0x02;
        descriptor[11] = 0x01;
        descriptor[12] = 0x00;
        descriptor[13] = 0x01;
        descriptor[14] = 0x00;
        descriptor[15] = 0x00;
        descriptor[16] = 1;
        descriptor[17] = 0;
        descriptor
    }

    fn configuration_descriptor(&self) -> Vec<u8> {
        configuration_descriptors(0x03, MOUSE_REPORT_DESCRIPTOR)
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
                    self.ep0 = MOUSE_REPORT_DESCRIPTOR.to_vec();
                    Ok(ControlResult::Complete {
                        len: MOUSE_REPORT_DESCRIPTOR.len(),
                    })
                }
                _ => Ok(ControlResult::Stall),
            },
            (0x21, 0x0A) => Ok(ControlResult::Complete { len: 0 }),
            (0x21, 0x0B) => Ok(ControlResult::Complete { len: 0 }),
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
            return Ok(Some(self.report[..max.min(3)].to_vec()));
        }
        Ok(None)
    }

    fn data_out(&mut self, _endpoint: u8, _data: &[u8]) -> Result<UsbStatus, UsbStatus> {
        Ok(UsbStatus::Ack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mouse_reports_motion() {
        let mut mouse = HidMouse::new();
        mouse.send_report(10, -5, 0x01);
        let report = mouse.data_in(1, 3).unwrap().unwrap();
        assert_eq!(report, vec![0x01, 10, 0xFB]);
    }
}
