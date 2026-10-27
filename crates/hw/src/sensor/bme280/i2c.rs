//! I2C ターゲットバインド（データシート §5.3 / §6.2）。
//!
//! 書き込みフレームの先頭バイトがレジスタポインタを設定し、続くバイトは
//! ポインタへ書き込まれて自動インクリメントする。読み出しフレームは
//! ポインタから自動インクリメントで返し、`0xFF` の次は `0x00` へ折り返す。

use super::Bme280Core;
use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract::model::Ctx;
use mosaic_exec::{Fnv64, Snapshot, SnapshotError};

/// バインド構築パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct Bme280I2cParams {
    /// この個体が応答する 7 ビットアドレス（実機は SDO ピンで 0x76/0x77）。
    pub address: I2cAddr,
}

impl Bme280I2cParams {
    pub const fn new(address: I2cAddr) -> Self {
        Self { address }
    }
}

/// I2C 経由の BME280。ロジックは [`Bme280Core`] に集約する。
pub struct Bme280I2c {
    core: Bme280Core,
    address: I2cAddr,
    pointer: u8,
}

impl Bme280I2c {
    pub fn new(params: Bme280I2cParams) -> Self {
        Self {
            core: Bme280Core::new(),
            address: params.address,
            pointer: 0,
        }
    }

    /// 内部ロジックへの参照（ホスト側補償演算の検証用）。
    pub fn core(&self) -> &Bme280Core {
        &self.core
    }
}

impl I2cTarget for Bme280I2c {
    fn matches(&self, addr: &I2cAddr) -> bool {
        *addr == self.address
    }

    fn target_write(
        &mut self,
        _ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        if !self.matches(addr) {
            return Err(I2cError::Nack);
        }
        let Some((&pointer, payload)) = bytes.split_first() else {
            // ポインタ設定だけの空フレームは ACK して終わる。
            return Ok(Ack::Ack);
        };
        self.pointer = pointer;
        for &value in payload {
            self.core.write_byte(self.pointer, value);
            self.pointer = self.pointer.wrapping_add(1);
        }
        Ok(Ack::Ack)
    }

    fn target_read(
        &mut self,
        _ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError> {
        if !self.matches(addr) {
            return Err(I2cError::Nack);
        }
        for slot in buf.iter_mut() {
            *slot = self.core.read_byte(self.pointer);
            self.pointer = self.pointer.wrapping_add(1);
        }
        Ok(Ack::Ack)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bme280I2cCheckpoint {
    pub schema: u32,
    pub pointer: u8,
    pub bank: [u8; 256],
}

impl Snapshot for Bme280I2c {
    type State = Bme280I2cCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Bme280I2cCheckpoint {
            schema: Self::SCHEMA_VERSION,
            pointer: self.pointer,
            bank: self.core.export_bank(),
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.pointer = state.pointer;
        self.core.import_bank(state.bank);
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.byte(state.pointer);
        hash.bytes(&state.bank);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::super::{reg, CHIP_ID_BME280, DEFAULT_CALIBRATION, DEFAULT_SCENE};
    use super::*;
    use mosaic_contract::i2c::I2cAddr;
    use mosaic_contract::model::ContextError;

    struct CtxStub;

    impl Ctx for CtxStub {
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
        fn set_signal(
            &mut self,
            _signal: mosaic_exec::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }

    fn part() -> Bme280I2c {
        Bme280I2c::new(Bme280I2cParams::new(I2cAddr::Seven(0x76)))
    }

    #[test]
    fn pointer_write_then_burst_read_crosses_register_blocks() {
        let mut device = part();
        let mut ctx = CtxStub;
        let addr = I2cAddr::Seven(0x76);
        // ポインタを 0xF7 に置いて 9 バイトバースト: press(3)+temp(3)+hum(2)+config? ではなく
        // 0xF7..0xFF の 9 バイトを読み切る。
        device
            .target_write(&mut ctx, &addr, &[reg::PRESS_MSB])
            .unwrap();
        let mut buf = [0u8; 9];
        device.target_read(&mut ctx, &addr, &mut buf).unwrap();
        assert_eq!(&buf[..3], &[0x65, 0x5A, 0xC0]);
        assert_eq!(&buf[3..6], &[0x7E, 0xED, 0x00]);
        assert_eq!(&buf[6..8], &[0x80, 0x00]);
        assert_eq!(buf[8], 0x00); // 0xFF は未使用領域
                                  // ポインタは 0xFF の次で 0x00 へ折り返す。
        let mut wrapped = [0u8; 1];
        device.target_read(&mut ctx, &addr, &mut wrapped).unwrap();
        assert_eq!(wrapped[0], 0x00);
    }

    #[test]
    fn chip_id_read_matches_the_transaction_path() {
        let mut device = part();
        let mut ctx = CtxStub;
        let addr = I2cAddr::Seven(0x76);
        device
            .target_write(&mut ctx, &addr, &[reg::CHIP_ID])
            .unwrap();
        let mut id = [0u8; 1];
        device.target_read(&mut ctx, &addr, &mut id).unwrap();
        assert_eq!(id[0], CHIP_ID_BME280);
    }

    #[test]
    fn multi_byte_write_sets_control_then_increments() {
        let mut device = part();
        let mut ctx = CtxStub;
        let addr = I2cAddr::Seven(0x76);
        // ctrl_hum(0xF2) から 4 バイト: 0xF2←0x01、0xF3 は読み出し専用で
        // 無視、0xF4←0x25、0xF5←0xA0 と自動インクリメントされる。
        device
            .target_write(&mut ctx, &addr, &[reg::CTRL_HUM, 0x01, 0x00, 0x25, 0xA0])
            .unwrap();
        device
            .target_write(&mut ctx, &addr, &[reg::CTRL_MEAS])
            .unwrap();
        let mut buf = [0u8; 2];
        device.target_read(&mut ctx, &addr, &mut buf).unwrap();
        assert_eq!(buf, [0x25, 0xA0]); // ctrl_meas, config
        device
            .target_write(&mut ctx, &addr, &[reg::CTRL_HUM])
            .unwrap();
        device.target_read(&mut ctx, &addr, &mut buf).unwrap();
        assert_eq!(buf, [0x01, 0x00]); // ctrl_hum、status は常に 0
    }

    #[test]
    fn wrong_address_is_rejected() {
        let mut device = part();
        let mut ctx = CtxStub;
        assert!(!device.matches(&I2cAddr::Seven(0x77)));
        assert_eq!(
            device.target_write(&mut ctx, &I2cAddr::Seven(0x77), &[0]),
            Err(I2cError::Nack)
        );
        assert_eq!(
            device.target_read(&mut ctx, &I2cAddr::Seven(0x77), &mut [0]),
            Err(I2cError::Nack)
        );
    }

    #[test]
    fn host_side_compensation_matches_the_documented_scene() {
        let device = part();
        // ホストはバースト読み出しした生値と NVM から自分で補償する。
        assert_eq!(device.core().calibration(), DEFAULT_CALIBRATION);
        let compensated = device.core().compensated();
        assert_eq!(compensated.temperature, 2_508);
        assert_eq!(compensated.pressure, 25_650_205);
        assert_eq!(compensated.humidity, 82_623);
        assert_eq!(DEFAULT_SCENE.adc_t, 519_888);
    }
}

#[cfg(test)]
mod snapshot_roundtrip {
    use mosaic_contract::i2c::I2cAddr;
    use mosaic_exec::Snapshot;

    use super::*;

    #[test]
    fn pointer_and_register_bank_restore() {
        let mut part = Bme280I2c::new(Bme280I2cParams::new(I2cAddr::Seven(0x76)));
        part.pointer = 0xF4;
        part.core.write_byte(0xF4, 0x25);
        let saved = part.save();
        part.pointer = 0;
        part.core.write_byte(0xF4, 0);
        part.load(Bme280I2c::SCHEMA_VERSION, &saved).unwrap();
        assert_eq!(part.pointer, 0xF4);
        assert_eq!(part.core.read_byte(0xF4), 0x25);
        assert_ne!(Bme280I2c::fingerprint(Bme280I2c::SCHEMA_VERSION, &saved), 0);
    }
}
