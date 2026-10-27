//! Bosch BME280 統合環境センサの機能モデル（構成設計書 P2 / ADR 0008）。
//!
//! レジスタ意味論は Bosch BME280 データシートに基づく:
//! - `0xD0` チップ ID（BME280 = `0x60`）、`0xE0` ソフトリセット（`0xB6`）。
//! - `0xF2` ctrl_hum / `0xF4` ctrl_meas / `0xF5` config（読み書き可）。
//! - `0xF3` status、`0xF7..0xFC` 圧力/温度データ、`0xFD..0xFE` 湿度データ
//!   （読み出し専用）。キャリブレーション NVM は `0x88..0x9F`、`0xA1`、
//!   `0xE1..0xE7`。
//! - 読み出しポインタは 8 ビットで、読み書きのたびに 1 加算され `0xFF` から
//!   `0x00` へ折り返す（データシート §5.4 のバースト読み出しと同じ）。
//!
//! 機能モデルの簡略化（宣言のみ、タイミングは持たない）:
//! - 測定は「常に完了している」: データレジスタは固定シーン
//!   ([`DEFAULT_SCENE`]) の生 ADC 値を保持し、ctrl_meas のモード遷移や
//!   measuring ビットは modeled しない。
//! - 補償演算（データシート §4.2.3 の整数アルゴリズム）は
//!   [`Calibration`] のメソッドとして公開し、ホスト側（ファームウェアや
//!   テスト）が実機と同じ手順で生値 + NVM から計算できる。

pub mod reg {
    //! データシート準拠のレジスタオフセット。
    pub const CALIB_TP_START: u8 = 0x88;
    pub const CALIB_H1: u8 = 0xA1;
    pub const CHIP_ID: u8 = 0xD0;
    pub const RESET: u8 = 0xE0;
    pub const CALIB_H_START: u8 = 0xE1;
    pub const CTRL_HUM: u8 = 0xF2;
    pub const STATUS: u8 = 0xF3;
    pub const CTRL_MEAS: u8 = 0xF4;
    pub const CONFIG: u8 = 0xF5;
    pub const PRESS_MSB: u8 = 0xF7;
    pub const TEMP_MSB: u8 = 0xFA;
    pub const HUM_MSB: u8 = 0xFD;
}

pub mod i2c;
pub mod spi;

/// BME280 のチップ ID 値（データシート Table 16）。
pub const CHIP_ID_BME280: u8 = 0x60;

/// `0xE0` への書き込みでソフトリセットを発動させるコマンド値。
pub const RESET_COMMAND: u8 = 0xB6;

use mosaic_util::bits::{read_u16_le, write_u16_le};

/// キャリブレーション係数（データシート §4.2.2 の dig_T1..dig_H6）。
///
/// 実機では NVM から読み出す値。本モデルは構築時に
/// [`DEFAULT_CALIBRATION`] をレジスタバンクへ符号化して格納し、読み出しは
/// 常にバンク経由（補償演算もバンクから再デコードする）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calibration {
    pub dig_t1: u16,
    pub dig_t2: i16,
    pub dig_t3: i16,
    pub dig_p1: u16,
    pub dig_p2: i16,
    pub dig_p3: i16,
    pub dig_p4: i16,
    pub dig_p5: i16,
    pub dig_p6: i16,
    pub dig_p7: i16,
    pub dig_p8: i16,
    pub dig_p9: i16,
    pub dig_h1: u8,
    pub dig_h2: i16,
    pub dig_h3: u8,
    pub dig_h4: i16,
    pub dig_h5: i16,
    pub dig_h6: i8,
}

/// データシートの算出例と同じ係数群。t_fine = 128422 / 25.08 °C /
/// 100196 Pa / 80.68 %RH の基準シーンを再現する。
pub const DEFAULT_CALIBRATION: Calibration = Calibration {
    dig_t1: 27504,
    dig_t2: 26435,
    dig_t3: -1000,
    dig_p1: 36477,
    dig_p2: -10685,
    dig_p3: 3024,
    dig_p4: 2855,
    dig_p5: 140,
    dig_p6: -7,
    dig_p7: 9900,
    dig_p8: -10230,
    dig_p9: 4285,
    dig_h1: 165,
    dig_h2: 335,
    dig_h3: 0,
    dig_h4: 262,
    dig_h5: 50,
    dig_h6: 30,
};

/// 固定シーンの生 ADC 値。データレジスタはこの値を保持する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scene {
    /// 20 ビット生圧力値。
    pub adc_p: u32,
    /// 20 ビット生温度値。
    pub adc_t: u32,
    /// 16 ビット生湿度値。
    pub adc_h: u16,
}

/// 基準シーン（データシートの算出例と同じ生値）。
pub const DEFAULT_SCENE: Scene = Scene {
    adc_p: 415_148,
    adc_t: 519_888,
    adc_h: 32_768,
};

/// 補償後の測定結果（データシート §4.2.3 の Q フォーマット）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compensated {
    /// 温度補償に使う中間値。圧力・湿度補償の入力。
    pub t_fine: i32,
    /// 温度。単位 0.01 °C（例: 2508 = 25.08 °C）。
    pub temperature: i32,
    /// 圧力。Q24.8、単位 Pa*256（例: 25_650_205 ≈ 100196.11 Pa）。
    pub pressure: u64,
    /// 湿度。Q22.10、単位 %RH*1024（例: 82623 ≈ 80.68 %RH）。
    pub humidity: u32,
}

impl Calibration {
    /// 温度補償（データシート §4.2.3 `BME280_compensate_T_int32`）。
    /// 戻り値は (t_fine, 0.01 °C 単位温度)。
    pub fn compensate_temperature(&self, adc_t: u32) -> (i32, i32) {
        let adc = adc_t as i32;
        let t1 = i32::from(self.dig_t1);
        let var1 = (((adc >> 3) - (t1 << 1)) * i32::from(self.dig_t2)) >> 11;
        let delta = (adc >> 4) - t1;
        let var2 = (((delta * delta) >> 12) * i32::from(self.dig_t3)) >> 14;
        let t_fine = var1 + var2;
        (t_fine, (t_fine * 5 + 128) >> 8)
    }

    /// 圧力補償（データシート §4.2.3 `BME280_compensate_P_int64`）。
    /// 戻り値は Q24.8（Pa*256）。var1 が 0 のときは 0（除算回避）。
    pub fn compensate_pressure(&self, adc_p: u32, t_fine: i32) -> u64 {
        let var1 = i64::from(t_fine) - 128_000;
        let var2 = var1 * var1 * i64::from(self.dig_p6)
            + ((var1 * i64::from(self.dig_p5)) << 17)
            + (i64::from(self.dig_p4) << 35);
        let var1 =
            ((var1 * var1 * i64::from(self.dig_p3)) >> 8) + ((var1 * i64::from(self.dig_p2)) << 12);
        let var1 = (((1i64 << 47) + var1) * i64::from(self.dig_p1)) >> 33;
        if var1 == 0 {
            return 0;
        }
        let mut p = 1_048_576 - i64::from(adc_p);
        p = (((p << 31) - var2) * 3125) / var1;
        let var1 = (i64::from(self.dig_p9) * (p >> 13) * (p >> 13)) >> 25;
        let var2 = (i64::from(self.dig_p8) * p) >> 19;
        p = ((p + var1 + var2) >> 8) + (i64::from(self.dig_p4) << 4);
        if p < 0 {
            0
        } else {
            p as u64
        }
    }

    /// 湿度補償（データシート §4.2.3 `BME280_compensate_H_int32`）。
    /// 戻り値は Q22.10（%RH*1024）、0..=419_430_400 >> 12 にクランプ。
    pub fn compensate_humidity(&self, adc_h: u16, t_fine: i32) -> u32 {
        let h = t_fine - 76_800;
        let outer = ((((i32::from(adc_h)) << 14)
            - (i32::from(self.dig_h4) << 20)
            - (i32::from(self.dig_h5) * h))
            + 16_384)
            >> 15;
        let inner = ((((((h * i32::from(self.dig_h6)) >> 10)
            * (((h * i32::from(self.dig_h3)) >> 11) + 32_768))
            >> 10)
            + 2_097_152)
            * i32::from(self.dig_h2)
            + 8_192)
            >> 14;
        let mut v = outer.wrapping_mul(inner);
        v = v.wrapping_sub(((((v >> 15) * (v >> 15)) >> 7) * i32::from(self.dig_h1)) >> 4);
        let v = v.clamp(0, 419_430_400);
        (v >> 12) as u32
    }
}

/// 12 ビット符号付き値（dig_H4 / dig_H5 のニブル連結）を復号する。
fn sign_extend_12(value: u16) -> i16 {
    let masked = value & 0x0FFF;
    if masked & 0x0800 != 0 {
        (masked | 0xF000) as i16
    } else {
        masked as i16
    }
}

/// BME280 のレジスタバンク（256 バイト全アドレス空間）。
///
/// 読み出し専用領域（ID・キャリブレーション・status・データ）への書き込みは
/// 無視し、`0xE0` への [`RESET_COMMAND`] 書き込みだけ既定値へ戻す。
pub struct Bme280Core {
    bank: [u8; 256],
}

impl Default for Bme280Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Bme280Core {
    /// 既定キャリブレーション・既定シーンで構築する。
    pub fn new() -> Self {
        Self::with(DEFAULT_CALIBRATION, DEFAULT_SCENE)
    }

    /// Register bank for a versioned checkpoint. Host bindings stay outside.
    pub fn export_bank(&self) -> [u8; 256] {
        self.bank
    }

    pub fn import_bank(&mut self, bank: [u8; 256]) {
        self.bank = bank;
    }

    /// 指定キャリブレーション・シーンで構築する（テスト用フック）。
    pub fn with(calibration: Calibration, scene: Scene) -> Self {
        let mut bank = [0u8; 256];
        // 温度・圧力係数: 0x88..0x9F の 24 バイト、16 ビット LE。
        let tp = [
            calibration.dig_t1,
            calibration.dig_t2 as u16,
            calibration.dig_t3 as u16,
            calibration.dig_p1,
            calibration.dig_p2 as u16,
            calibration.dig_p3 as u16,
            calibration.dig_p4 as u16,
            calibration.dig_p5 as u16,
            calibration.dig_p6 as u16,
            calibration.dig_p7 as u16,
            calibration.dig_p8 as u16,
            calibration.dig_p9 as u16,
        ];
        for (index, word) in tp.iter().enumerate() {
            let base = usize::from(reg::CALIB_TP_START) + index * 2;
            let _ = write_u16_le(&mut bank[base..base + 2], *word);
        }
        bank[usize::from(reg::CALIB_H1)] = calibration.dig_h1;
        let h_base = usize::from(reg::CALIB_H_START);
        let _ = write_u16_le(&mut bank[h_base..h_base + 2], calibration.dig_h2 as u16);
        bank[h_base + 2] = calibration.dig_h3;
        // dig_H4 = 0xE4[7:0]:0xE5[3:0]、dig_H5 = 0xE6[7:0]:0xE5[7:4]。
        let h4 = calibration.dig_h4 as u16 & 0x0FFF;
        let h5 = calibration.dig_h5 as u16 & 0x0FFF;
        bank[h_base + 3] = (h4 >> 4) as u8;
        bank[h_base + 4] = ((h4 & 0x0F) | ((h5 & 0x0F) << 4)) as u8;
        bank[h_base + 5] = (h5 >> 4) as u8;
        bank[h_base + 6] = calibration.dig_h6 as u8;

        bank[usize::from(reg::CHIP_ID)] = CHIP_ID_BME280;
        // status / ctrl_hum / ctrl_meas / config / reset は POR = 0x00。
        Self::encode_20bit(&mut bank, reg::PRESS_MSB, scene.adc_p & 0x000F_FFFF);
        Self::encode_20bit(&mut bank, reg::TEMP_MSB, scene.adc_t & 0x000F_FFFF);
        bank[usize::from(reg::HUM_MSB)] = (scene.adc_h >> 8) as u8;
        bank[usize::from(reg::HUM_MSB) + 1] = (scene.adc_h & 0xFF) as u8;
        Self { bank }
    }

    fn encode_20bit(bank: &mut [u8; 256], base: u8, value: u32) {
        bank[usize::from(base)] = ((value >> 12) & 0xFF) as u8;
        bank[usize::from(base) + 1] = ((value >> 4) & 0xFF) as u8;
        bank[usize::from(base) + 2] = ((value << 4) & 0xF0) as u8;
    }

    fn decode_20bit(&self, base: u8) -> u32 {
        let i = usize::from(base);
        (u32::from(self.bank[i]) << 12)
            | (u32::from(self.bank[i + 1]) << 4)
            | (u32::from(self.bank[i + 2] >> 4))
    }

    /// レジスタ 1 バイト読み出し。オフセットは u8 なので折り返しは起きない。
    pub fn read_byte(&self, offset: u8) -> u8 {
        self.bank[usize::from(offset)]
    }

    /// レジスタ 1 バイト書き込み。書き込み可は ctrl_hum / ctrl_meas /
    /// config のみ。`0xE0` への [`RESET_COMMAND`] はソフトリセット。
    pub fn write_byte(&mut self, offset: u8, value: u8) {
        match offset {
            reg::RESET => {
                if value == RESET_COMMAND {
                    *self = Self::new();
                }
            }
            reg::CTRL_HUM | reg::CTRL_MEAS | reg::CONFIG => {
                self.bank[usize::from(offset)] = value;
            }
            // status は機能モデルでは常に 0（測定は同期・即時完了）。
            // ID・キャリブレーション・データと同様、書き込みは無視する。
            reg::STATUS | reg::CHIP_ID => {}
            _ => {}
        }
    }

    /// バンクからキャリブレーション係数を再デコードする。
    pub fn calibration(&self) -> Calibration {
        let tp = usize::from(reg::CALIB_TP_START);
        let word = |index: usize| -> u16 {
            read_u16_le(&self.bank[tp + index * 2..tp + index * 2 + 2]).unwrap_or(0)
        };
        let h_base = usize::from(reg::CALIB_H_START);
        let h4 = (u16::from(self.bank[h_base + 3]) << 4) | u16::from(self.bank[h_base + 4] & 0x0F);
        let h5 = (u16::from(self.bank[h_base + 5]) << 4) | u16::from(self.bank[h_base + 4] >> 4);
        Calibration {
            dig_t1: word(0),
            dig_t2: word(1) as i16,
            dig_t3: word(2) as i16,
            dig_p1: word(3),
            dig_p2: word(4) as i16,
            dig_p3: word(5) as i16,
            dig_p4: word(6) as i16,
            dig_p5: word(7) as i16,
            dig_p6: word(8) as i16,
            dig_p7: word(9) as i16,
            dig_p8: word(10) as i16,
            dig_p9: word(11) as i16,
            dig_h1: self.bank[usize::from(reg::CALIB_H1)],
            dig_h2: read_u16_le(&self.bank[h_base..h_base + 2]).unwrap_or(0) as i16,
            dig_h3: self.bank[h_base + 2],
            dig_h4: sign_extend_12(h4),
            dig_h5: sign_extend_12(h5),
            dig_h6: self.bank[h_base + 6] as i8,
        }
    }

    /// 現在のバンク内容（生 ADC + キャリブレーション）から補償結果を計算する。
    pub fn compensated(&self) -> Compensated {
        let calibration = self.calibration();
        let adc_p = self.decode_20bit(reg::PRESS_MSB);
        let adc_t = self.decode_20bit(reg::TEMP_MSB);
        let adc_h = (u16::from(self.bank[usize::from(reg::HUM_MSB)]) << 8)
            | u16::from(self.bank[usize::from(reg::HUM_MSB) + 1]);
        let (t_fine, temperature) = calibration.compensate_temperature(adc_t);
        Compensated {
            t_fine,
            temperature,
            pressure: calibration.compensate_pressure(adc_p, t_fine),
            humidity: calibration.compensate_humidity(adc_h, t_fine),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn calibration_round_trips_through_the_register_bank() {
        let core = Bme280Core::new();
        assert_eq!(core.calibration(), DEFAULT_CALIBRATION);
    }

    #[test]
    fn data_registers_hold_the_scene_raw_values() {
        let core = Bme280Core::new();
        assert_eq!(core.read_byte(reg::PRESS_MSB), 0x65);
        assert_eq!(core.read_byte(reg::PRESS_MSB + 1), 0x5A);
        assert_eq!(core.read_byte(reg::PRESS_MSB + 2), 0xC0);
        assert_eq!(core.read_byte(reg::TEMP_MSB), 0x7E);
        assert_eq!(core.read_byte(reg::TEMP_MSB + 1), 0xED);
        assert_eq!(core.read_byte(reg::TEMP_MSB + 2), 0x00);
        assert_eq!(core.read_byte(reg::HUM_MSB), 0x80);
        assert_eq!(core.read_byte(reg::HUM_MSB + 1), 0x00);
    }

    #[test]
    fn datasheet_example_compensates_to_documented_values() {
        let core = Bme280Core::new();
        let compensated = core.compensated();
        assert_eq!(compensated.t_fine, 128_422);
        assert_eq!(compensated.temperature, 2_508);
        assert_eq!(compensated.pressure, 25_650_205);
        assert_eq!(compensated.humidity, 82_623);
    }

    #[test]
    fn chip_id_is_read_only() {
        let mut core = Bme280Core::new();
        core.write_byte(reg::CHIP_ID, 0x00);
        assert_eq!(core.read_byte(reg::CHIP_ID), CHIP_ID_BME280);
    }

    #[test]
    fn control_registers_are_read_write_and_reset_restores_defaults() {
        let mut core = Bme280Core::new();
        core.write_byte(reg::CTRL_HUM, 0x01);
        core.write_byte(reg::CTRL_MEAS, 0x25);
        core.write_byte(reg::CONFIG, 0xA0);
        assert_eq!(core.read_byte(reg::CTRL_HUM), 0x01);
        assert_eq!(core.read_byte(reg::CTRL_MEAS), 0x25);
        assert_eq!(core.read_byte(reg::CONFIG), 0xA0);
        // 0xB6 以外はリセットを発動しない。
        core.write_byte(reg::RESET, 0x00);
        assert_eq!(core.read_byte(reg::CTRL_MEAS), 0x25);
        core.write_byte(reg::RESET, RESET_COMMAND);
        assert_eq!(core.read_byte(reg::CTRL_HUM), 0x00);
        assert_eq!(core.read_byte(reg::CTRL_MEAS), 0x00);
        assert_eq!(core.read_byte(reg::CONFIG), 0x00);
        // NVM とシーンはソフトリセット後も保持される。
        assert_eq!(core.read_byte(reg::CHIP_ID), CHIP_ID_BME280);
        assert_eq!(core.calibration(), DEFAULT_CALIBRATION);
    }

    #[test]
    fn data_and_calibration_writes_are_ignored() {
        let mut core = Bme280Core::new();
        core.write_byte(reg::PRESS_MSB, 0x00);
        core.write_byte(reg::CALIB_TP_START, 0x00);
        core.write_byte(reg::STATUS, 0xFF);
        assert_eq!(core.read_byte(reg::PRESS_MSB), 0x65);
        assert_eq!(core.read_byte(reg::CALIB_TP_START), 0x70);
        assert_eq!(core.read_byte(reg::STATUS), 0x00);
    }

    #[test]
    fn sign_extension_covers_negative_humidity_coefficients() {
        let mut calibration = DEFAULT_CALIBRATION;
        calibration.dig_h4 = -262;
        calibration.dig_h5 = -50;
        let core = Bme280Core::with(calibration, DEFAULT_SCENE);
        let decoded = core.calibration();
        assert_eq!(decoded.dig_h4, -262);
        assert_eq!(decoded.dig_h5, -50);
    }
}
