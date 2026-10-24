//! SPI ターゲットバインド（データシート §5.3.2 の 4 線式）。
//!
//! コマンドバイトは bit7 = 方向（1 = 読み出し、0 = 書き込み）、bit6..0 =
//! レジスタアドレス下位 7 ビット。BME280 のレジスタは全て 0x80 以上なので
//! 実アドレスは `(コマンド & 0x7F) | 0x80`（Linux bmp280-spi ドライバの
//! 「full register address without bit 7」と同じ規則）。CS アサート後の
//! 最初の 1 バイトがコマンドで、以降のバイトはポインタの自動インクリメント
//! で読み書きされる。CS ディアサートでコマンドラッチは初期状態へ戻る
//! （契約: 「Deselect must reset any frame pointer」）。読み出しコマンド
//! 直後の 1 バイト目は実機同様 0x00 を返す。

use crate::Bme280Core;
use mosaic_contract_model::Ctx;
use mosaic_contract_spi::{SpiError, SpiMode, SpiTarget};

/// SPI 経由の BME280。ロジックは [`Bme280Core`] に集約する。
#[derive(Default)]
pub struct Bme280Spi {
    core: Bme280Core,
    pointer: u8,
    read_mode: bool,
    latched: bool,
}

impl Bme280Spi {
    pub fn new() -> Self {
        Self {
            core: Bme280Core::new(),
            pointer: 0,
            read_mode: false,
            latched: false,
        }
    }

    /// 内部ロジックへの参照（ホスト側補償演算の検証用）。
    pub fn core(&self) -> &Bme280Core {
        &self.core
    }
}

impl SpiTarget for Bme280Spi {
    fn selected(&mut self, _ctx: &mut dyn Ctx, assert: bool) {
        if !assert {
            // フレーム境界: 次のバイトは再びコマンドとして解釈される。
            self.latched = false;
            self.read_mode = false;
        }
    }

    fn exchange_byte(
        &mut self,
        _ctx: &mut dyn Ctx,
        _mode: SpiMode,
        mosi: u8,
    ) -> Result<u8, SpiError> {
        if !self.latched {
            self.latched = true;
            self.read_mode = mosi & 0x80 != 0;
            self.pointer = (mosi & 0x7F) | 0x80;
            return Ok(0x00);
        }
        let value = if self.read_mode {
            self.core.read_byte(self.pointer)
        } else {
            self.core.write_byte(self.pointer, mosi);
            0x00
        };
        self.pointer = self.pointer.wrapping_add(1);
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::{reg, CHIP_ID_BME280};
    use mosaic_contract_model::ContextError;

    struct CtxStub;

    impl Ctx for CtxStub {
        fn now(&self) -> mosaic_core_types::VTime {
            mosaic_core_types::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_core_types::VTime,
            _token: u64,
        ) -> Result<mosaic_core_types::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_core_types::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _signal: mosaic_core_types::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }

    #[test]
    fn read_command_returns_id_after_the_dummy_byte() {
        let mut device = Bme280Spi::new();
        let mut ctx = CtxStub;
        let mode = SpiMode::new(false, false);
        device.selected(&mut ctx, true);
        // 読み出しコマンド = レジスタ | 0x80（0xD0 はそのまま）。
        // 直後の 1 バイト目はダミー（実機の SDO は 0x00）。
        assert_eq!(
            device
                .exchange_byte(&mut ctx, mode, reg::CHIP_ID | 0x80)
                .unwrap(),
            0x00
        );
        assert_eq!(
            device.exchange_byte(&mut ctx, mode, 0x00).unwrap(),
            CHIP_ID_BME280
        );
        device.selected(&mut ctx, false);
    }

    #[test]
    fn burst_read_walks_the_data_registers() {
        let mut device = Bme280Spi::new();
        let mut ctx = CtxStub;
        let mode = SpiMode::new(false, false);
        device.selected(&mut ctx, true);
        device
            .exchange_byte(&mut ctx, mode, reg::PRESS_MSB | 0x80)
            .unwrap();
        let mut buf = [0u8; 8];
        for slot in &mut buf {
            *slot = device.exchange_byte(&mut ctx, mode, 0x00).unwrap();
        }
        device.selected(&mut ctx, false);
        assert_eq!(&buf[..3], &[0x65, 0x5A, 0xC0]);
        assert_eq!(&buf[3..6], &[0x7E, 0xED, 0x00]);
        assert_eq!(&buf[6..8], &[0x80, 0x00]);
    }

    #[test]
    fn write_command_updates_control_registers() {
        let mut device = Bme280Spi::new();
        let mut ctx = CtxStub;
        let mode = SpiMode::new(false, false);
        device.selected(&mut ctx, true);
        // 書き込みコマンド: レジスタアドレスの bit7 を落とす
        // (0xF4 → 0x74、Linux bmp280-spi ドライバと同じ)。
        device
            .exchange_byte(&mut ctx, mode, reg::CTRL_MEAS & 0x7F)
            .unwrap();
        device.exchange_byte(&mut ctx, mode, 0x25).unwrap();
        device.selected(&mut ctx, false);
        device.selected(&mut ctx, true);
        device
            .exchange_byte(&mut ctx, mode, reg::CTRL_MEAS | 0x80)
            .unwrap();
        assert_eq!(device.exchange_byte(&mut ctx, mode, 0x00).unwrap(), 0x25);
        device.selected(&mut ctx, false);
        assert_eq!(device.core().read_byte(reg::CTRL_MEAS), 0x25);
    }

    #[test]
    fn deselect_resets_the_command_latch() {
        let mut device = Bme280Spi::new();
        let mut ctx = CtxStub;
        let mode = SpiMode::new(false, false);
        device.selected(&mut ctx, true);
        device
            .exchange_byte(&mut ctx, mode, reg::PRESS_MSB | 0x80)
            .unwrap();
        device.selected(&mut ctx, false);
        // 再アサート後の 1 バイト目はコマンドとして解釈される（ダミー 0x00）。
        device.selected(&mut ctx, true);
        assert_eq!(
            device
                .exchange_byte(&mut ctx, mode, reg::CHIP_ID | 0x80)
                .unwrap(),
            0x00
        );
        assert_eq!(
            device.exchange_byte(&mut ctx, mode, 0x00).unwrap(),
            CHIP_ID_BME280
        );
        device.selected(&mut ctx, false);
    }
}
