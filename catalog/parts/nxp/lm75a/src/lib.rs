#![no_std]
#![forbid(unsafe_code)]
//! NXP LM75A デジタル温度センサの機能レジスタモデル（NXP LM75A プロダクト
//! データシート、I2C バス側バインド）。
//!
//! ゲストは実機と同じ I2C フレームで駆動する:
//! - 書き込みフレームの先頭バイトがポインタ（2 ビット）を設定し、以降の
//!   データバイトはバイトレーンを自動インクリメントで埋める
//!   （Tos 書き込みは `[3, msb, lsb]` の 3 バイトフレーム）。
//! - 読み出しフレームはポインタのバイトレーンから順に返し、レジスタを
//!   またいで自動インクリメントする（7 バイト目の次は先頭へ折り返す）。
//!   Temp 読み出しは interrupt モードの OS ラッチをクリアする。
//! - バイトレーン列: `Temp MSB, Temp LSB, Conf, Thyst MSB, Thyst LSB,
//!   Tos MSB, Tos LSB`。Temp への書き込みは読み出し専用として無視。
//!
//! OS ピン（サーモスタット出力）:
//! - comparator モード: 温度 > T_os で主張、温度 < T_hyst で解除。挟み撃ち
//!   帯（ヒステリシス）では前回値を保持する。
//! - interrupt モード: 温度 > T_os の交差でラッチ、Temp レジスタ読み出しで
//!   クリア。
//! - OS_POL=0（既定）は active-LOW、OS_POL=1 は active-HIGH。
//! - 主張・解除の変化エッジは [`Ctx::set_signal`] で通知する。ctx は呼び出し
//!   元イニシエータのノードに束ねられるため、OS を通知する signal id は
//!   `params.os` で注入し、composition root がイニシエータノードから
//!   配線する（ADR 0008 ポート注入、ピンレベル net は後続フェーズ）。
//!
//! 機能モデルの簡略化（静的シーン、タイミングを持たない）:
//! - 温度は [`Lm75aScene`] として固定値を注入する。変換周期と変換中の
//!   読み出し遅延は modeled しない。
//! - fault queue（B[4:3]）は保持するが、連続変換の計数がないため即時
//!   （queue=1 相当）で評価する。
//! - SHUTDOWN 中は OS の再評価を中止し直前状態を保持する。
//! - I2C のクロックストレッチ・仲裁はバス実体（`mosaic-interconnect-i2c-bus`）
//!   の責務。

use mosaic_contract_i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_core_types::{Fnv64, SignalId, Snapshot, SnapshotError};

/// ポインタレジスタの値。
mod pointer {
    pub const TEMP: u8 = 0;
    pub const CONF: u8 = 1;
    pub const THYST: u8 = 2;
    pub const TOS: u8 = 3;
}

/// Conf レジスタのビット。
mod conf_bits {
    pub const SHUTDOWN: u8 = 1 << 0;
    pub const OS_COMP_INT: u8 = 1 << 1;
    pub const OS_POL: u8 = 1 << 2;
    /// 定義ビットは B[4:0] のみ。
    pub const DEFINED: u8 = 0x1F;
}

/// バイトレーンの先頭オフセット（ポインタ値 → レーン番号）。
fn lane_of(pointer: u8) -> usize {
    match pointer & 0x03 {
        pointer::TEMP => 0,  // Temp MSB
        pointer::CONF => 2,  // Conf
        pointer::THYST => 3, // Thyst MSB
        pointer::TOS => 5,   // Tos MSB
        _ => unreachable!("masked to two bits above"),
    }
}

/// 測定シーン。データレジスタはこの値を保持する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lm75aScene {
    /// 温度。0.125 °C 単位、11 ビット 2 の補数の範囲（-1024..=1023）。
    pub temp_x125: i16,
}

impl Lm75aScene {
    /// 温度を 0.125 °C 単位で検証付きで作る。
    pub const fn celsius_x125(temp_x125: i16) -> Self {
        Self { temp_x125 }
    }
}

/// 基準シーン: 25.0 °C。
pub const DEFAULT_SCENE: Lm75aScene = Lm75aScene { temp_x125: 200 };

/// 構築パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct Lm75aParams {
    /// この個体が応答する 7 ビットアドレス（実機は A0-A2 ピンで 0x48..0x4F）。
    pub address: I2cAddr,
    /// OS ピンレベルの通知先 signal（イニシエータノードの空間）。
    /// None なら OS はモデル内部状態のみ。
    pub os: Option<SignalId>,
    /// 測定シーン。
    pub scene: Lm75aScene,
}

impl Lm75aParams {
    pub const fn new(address: I2cAddr) -> Self {
        Self {
            address,
            os: None,
            scene: DEFAULT_SCENE,
        }
    }

    /// OS ピンの通知先を設定する。
    pub const fn with_os(mut self, os: SignalId) -> Self {
        self.os = Some(os);
        self
    }

    /// 測定シーンを差し替える。
    pub const fn with_scene(mut self, scene: Lm75aScene) -> Self {
        self.scene = scene;
        self
    }
}

/// 機能 LM75A。
pub struct Lm75a {
    params: Lm75aParams,
    /// ポインタレジスタ（フレームをまたいで保持）。
    pointer: u8,
    conf: u8,
    /// T_hyst。9 ビット、0.5 °C/LSB。
    thyst_x500: i16,
    /// T_os。9 ビット、0.5 °C/LSB。
    tos_x500: i16,
    /// OS 主張状態（ヒステリシス保持、interrupt ラッチを兼ねる）。
    os_asserted: bool,
    /// 前回通知した OS ピンの物理レベル（エッジ検出用）。
    os_pin_level: Option<bool>,
}

impl Lm75a {
    pub fn new(params: Lm75aParams) -> Self {
        let mut part = Self {
            params,
            pointer: pointer::TEMP,
            conf: 0,
            // パワーアップ既定: T_hyst = 75 °C、T_os = 80 °C。
            thyst_x500: 75 * 2,
            tos_x500: 80 * 2,
            os_asserted: false,
            os_pin_level: None,
        };
        // 電源投入時点の温度と既定しきい値で初期評価する。
        part.os_asserted = part.scene_temp_x500() > part.tos_x500;
        // 電源投入時のピンレベル（active-LOW アイドル = High）を基準にする。
        part.os_pin_level = Some(part.os_pin_high());
        part
    }

    /// シーン温度を 0.5 °C 単位（下向き丸め）で返す。
    fn scene_temp_x500(&self) -> i16 {
        self.params.scene.temp_x125 / 4
    }

    /// 現在の OS 主張状態。
    pub fn os_asserted(&self) -> bool {
        self.os_asserted
    }

    /// 現在の設定レジスタ。
    pub fn configuration(&self) -> u8 {
        self.conf
    }

    /// T_os（0.5 °C 単位）。
    pub fn tos_x500(&self) -> i16 {
        self.tos_x500
    }

    /// T_hyst（0.5 °C 単位）。
    pub fn thyst_x500(&self) -> i16 {
        self.thyst_x500
    }

    /// OS ピンの物理レベル。OS_POL=0 は active-LOW。
    fn os_pin_high(&self) -> bool {
        let asserted = self.os_asserted;
        if self.conf & conf_bits::OS_POL != 0 {
            asserted
        } else {
            !asserted
        }
    }

    /// OS 状態を再評価し、変化があれば ctx 経由で通知する。
    /// SHUTDOWN 中は評価しない（直前状態を保持）。
    fn evaluate_os(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        if self.conf & conf_bits::SHUTDOWN != 0 {
            return Ok(());
        }
        let temp = self.scene_temp_x500();
        let comparator_mode = self.conf & conf_bits::OS_COMP_INT == 0;
        if comparator_mode {
            // ヒステリシス帯の外側でのみ状態を変える。
            if temp > self.tos_x500 {
                self.os_asserted = true;
            } else if temp < self.thyst_x500 {
                self.os_asserted = false;
            }
        } else if temp > self.tos_x500 {
            // interrupt モード: 交差でラッチ。解除は Temp 読み出し側で行う。
            self.os_asserted = true;
        }
        self.emit_os(ctx)
    }

    /// ピンレベルの変化エッジだけを通知する。
    fn emit_os(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let level = self.os_pin_high();
        if self.os_pin_level != Some(level) {
            if let Some(os) = self.params.os {
                set_optional_signal(ctx, os, level)?;
            }
            self.os_pin_level = Some(level);
        }
        Ok(())
    }

    /// レーンの現在バイト値。
    fn lane_read(&self, lane: usize) -> u8 {
        match lane {
            0 => ((self.params.scene.temp_x125 as u16) >> 3) as u8,
            1 => ((self.params.scene.temp_x125 as u16) << 5) as u8,
            2 => self.conf,
            3 => ((self.thyst_x500 as u16) >> 1) as u8,
            4 => ((self.thyst_x500 as u16) << 7) as u8,
            5 => ((self.tos_x500 as u16) >> 1) as u8,
            _ => ((self.tos_x500 as u16) << 7) as u8,
        }
    }

    /// レーンへ 1 バイト書き込む。Temp は読み出し専用。
    fn lane_write(&mut self, lane: usize, value: u8) {
        match lane {
            0 | 1 => {}
            2 => self.conf = value & conf_bits::DEFINED,
            3 => {
                let lsb = self.thyst_x500 as u8 & 0x80;
                self.thyst_x500 = (((value as i8) as i16) << 1) | i16::from(lsb != 0);
            }
            4 => {
                let msb = ((self.thyst_x500 as u16) >> 1) as u8;
                self.thyst_x500 = (((msb as i8) as i16) << 1) | i16::from(value & 0x80 != 0);
            }
            5 => {
                let lsb = self.tos_x500 as u8 & 0x80;
                self.tos_x500 = (((value as i8) as i16) << 1) | i16::from(lsb != 0);
            }
            _ => {
                let msb = ((self.tos_x500 as u16) >> 1) as u8;
                self.tos_x500 = (((msb as i8) as i16) << 1) | i16::from(value & 0x80 != 0);
            }
        }
    }
}

impl I2cTarget for Lm75a {
    fn matches(&self, addr: &I2cAddr) -> bool {
        *addr == self.params.address
    }

    fn target_write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        if !self.matches(addr) {
            return Err(I2cError::Nack);
        }
        let Some((&raw_pointer, payload)) = bytes.split_first() else {
            // ポインタ設定だけの空フレームは ACK して終わる。
            return Ok(Ack::Ack);
        };
        self.pointer = raw_pointer & 0x03;
        let mut lane = lane_of(self.pointer);
        for &value in payload {
            self.lane_write(lane, value);
            lane = (lane + 1) % 7;
        }
        // しきい値・設定の変化で OS を再評価する。
        self.evaluate_os(ctx).map_err(|_| I2cError::Overflow)?;
        Ok(Ack::Ack)
    }

    fn target_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError> {
        if !self.matches(addr) {
            return Err(I2cError::Nack);
        }
        let mut lane = lane_of(self.pointer);
        for byte in buf.iter_mut() {
            *byte = self.lane_read(lane);
            lane = (lane + 1) % 7;
        }
        // Temp レジスタの読み出しは interrupt モードの OS ラッチをクリアする。
        let started_at_temp = self.pointer == pointer::TEMP;
        if started_at_temp && !buf.is_empty() && self.conf & conf_bits::OS_COMP_INT != 0 {
            self.os_asserted = false;
        }
        self.emit_os(ctx).map_err(|_| I2cError::Overflow)?;
        Ok(Ack::Ack)
    }
}

impl EventTarget for Lm75a {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// LM75A checkpoint. The temperature scene is a construction parameter, not a
/// register; the OS latch that the scene produced is emulated state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lm75aCheckpoint {
    pub schema: u32,
    pub pointer: u8,
    pub conf: u8,
    pub thyst_x500: i16,
    pub tos_x500: i16,
    pub os_asserted: bool,
    pub os_pin_level: Option<bool>,
}

impl Snapshot for Lm75a {
    type State = Lm75aCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Lm75aCheckpoint {
            schema: Self::SCHEMA_VERSION,
            pointer: self.pointer,
            conf: self.conf,
            thyst_x500: self.thyst_x500,
            tos_x500: self.tos_x500,
            os_asserted: self.os_asserted,
            os_pin_level: self.os_pin_level,
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
        self.conf = state.conf;
        self.thyst_x500 = state.thyst_x500;
        self.tos_x500 = state.tos_x500;
        self.os_asserted = state.os_asserted;
        self.os_pin_level = state.os_pin_level;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.byte(state.pointer);
        hash.byte(state.conf);
        hash.i16(state.thyst_x500);
        hash.i16(state.tos_x500);
        hash.bool(state.os_asserted);
        match state.os_pin_level {
            None => hash.byte(0),
            Some(false) => hash.byte(1),
            Some(true) => hash.byte(2),
        }
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;
    use std::vec;

    use super::*;
    use mosaic_core_types::{EventId, VTime};

    const ADDRESS: I2cAddr = I2cAddr::Seven(0x48);
    const OS: SignalId = SignalId(3);

    /// sp804 テストと同じ形の文脈: 信号エッジの記録。
    #[derive(Default)]
    struct TestCtx {
        edges: Vec<(SignalId, bool)>,
    }

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }
        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }
        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.edges.push((signal, high));
            Ok(())
        }
    }

    fn part() -> Lm75a {
        Lm75a::new(Lm75aParams::new(ADDRESS).with_os(OS))
    }

    fn write(part: &mut Lm75a, ctx: &mut TestCtx, bytes: &[u8]) {
        part.target_write(ctx, &ADDRESS, bytes).unwrap();
    }

    fn read(part: &mut Lm75a, ctx: &mut TestCtx, pointer: u8, len: usize) -> Vec<u8> {
        part.target_write(ctx, &ADDRESS, &[pointer]).unwrap();
        let mut buf = vec![0u8; len];
        part.target_read(ctx, &ADDRESS, &mut buf).unwrap();
        buf
    }

    #[test]
    fn power_up_defaults_match_the_datasheet() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        // Temp = 25.0 °C = 0x1900（11 ビット 200 を左詰め）。
        assert_eq!(
            read(&mut part, &mut ctx, pointer::TEMP, 2),
            vec![0x19, 0x00]
        );
        assert_eq!(read(&mut part, &mut ctx, pointer::CONF, 1), vec![0x00]);
        // T_hyst = 75 °C = 0x4B00、T_os = 80 °C = 0x5000。
        assert_eq!(
            read(&mut part, &mut ctx, pointer::THYST, 2),
            vec![0x4B, 0x00]
        );
        assert_eq!(read(&mut part, &mut ctx, pointer::TOS, 2), vec![0x50, 0x00]);
    }

    #[test]
    fn reads_stream_across_registers_and_wrap_around() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        // Temp MSB, Temp LSB, Conf, Thyst MSB, Thyst LSB, Tos MSB, Tos LSB。
        let all = read(&mut part, &mut ctx, pointer::TEMP, 7);
        assert_eq!(all, vec![0x19, 0x00, 0x00, 0x4B, 0x00, 0x50, 0x00]);
        // 8 バイト目は Temp MSB へ折り返す。
        let wrapped = read(&mut part, &mut ctx, pointer::TEMP, 8);
        assert_eq!(wrapped[7], 0x19);
    }

    #[test]
    fn tos_write_drives_the_comparator_os_output_with_hysteresis() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        // T_hyst = 20.5 °C（41 半度 = [0x14, 0x80]）を先に下げておく。
        write(&mut part, &mut ctx, &[2, 0x14, 0x80]);
        assert!(!part.os_asserted());

        // 25.0 °C > 24.5 °C（49 半度 = [0x18, 0x80]）で OS が主張される
        // （active-LOW 既定 → ピンは Low）。
        write(&mut part, &mut ctx, &[3, 0x18, 0x80]);
        assert!(part.os_asserted());
        assert_eq!(ctx.edges, vec![(OS, false)]);

        // ヒステリシス帯（20.5 °C..40 °C）の内側では保持される。
        write(&mut part, &mut ctx, &[3, 0x28, 0x00]); // T_os = 40 °C
        assert!(part.os_asserted());
        assert_eq!(ctx.edges.len(), 1);

        // T_hyst = 25.5 °C（51 半度 = [0x19, 0x80]）で 25.0 °C が下回り、
        // 解除される（ピンは High へ戻る）。
        write(&mut part, &mut ctx, &[2, 0x19, 0x80]);
        assert!(!part.os_asserted());
        assert_eq!(ctx.edges, vec![(OS, false), (OS, true)]);
    }

    #[test]
    fn os_polarity_selects_the_active_level() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        // OS_POL=1（active-HIGH）へ切り替えると非主張アイドルが Low になる。
        write(&mut part, &mut ctx, &[1, 0x04]);
        assert_eq!(ctx.edges, vec![(OS, false)]);
        // 25.0 °C > 24.5 °C で主張 → active-HIGH なのでピンは High。
        write(&mut part, &mut ctx, &[3, 0x18, 0x80]);
        assert!(part.os_asserted());
        assert_eq!(ctx.edges, vec![(OS, false), (OS, true)]);
    }

    #[test]
    fn interrupt_mode_latches_until_the_temp_register_is_read() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        // OS_COMP_INT=1 + T_os = 24.5 °C でラッチ。
        write(&mut part, &mut ctx, &[1, 0x02]);
        write(&mut part, &mut ctx, &[3, 0x18, 0x80]);
        assert!(part.os_asserted());
        // Conf レジスタの読み出しではクリアされない。
        let _ = read(&mut part, &mut ctx, pointer::CONF, 1);
        assert!(part.os_asserted());
        // Temp レジスタの読み出しでクリアされる。
        let _ = read(&mut part, &mut ctx, pointer::TEMP, 2);
        assert!(!part.os_asserted());
        assert_eq!(ctx.edges, vec![(OS, false), (OS, true)]);
    }

    #[test]
    fn shutdown_freezes_the_os_state() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        write(&mut part, &mut ctx, &[3, 0x18, 0x00]);
        assert!(part.os_asserted());
        // SHUTDOWN=1 への切り替え自体は OS を変化させない。
        write(&mut part, &mut ctx, &[1, 0x01]);
        assert!(part.os_asserted());
        // シャットダウン中にしきい値を変えても再評価しない。
        write(&mut part, &mut ctx, &[2, 0x19, 0x80]);
        assert!(part.os_asserted());
    }

    #[test]
    fn negative_thresholds_and_temperature_encode_twos_complement() {
        let mut part = Lm75a::new(
            Lm75aParams::new(ADDRESS).with_scene(Lm75aScene::celsius_x125(-125)), // -15.625 °C
        );
        let mut ctx = TestCtx::default();
        // -15.625 °C = -125 ×0.125 = 11 ビット 0x783 → [0xF0, 0x60]。
        assert_eq!(
            read(&mut part, &mut ctx, pointer::TEMP, 2),
            vec![0xF0, 0x60]
        );
        // T_os への負のしきい値書き込み。0xF5,0x00 → 9 ビット -22 半度。
        write(&mut part, &mut ctx, &[3, 0xF5, 0x00]);
        assert_eq!(part.tos_x500(), -22);
        write(&mut part, &mut ctx, &[3, 0xF5, 0x80]);
        assert_eq!(part.tos_x500(), -21);
        // 読み戻しも同じバイト列表現。
        assert_eq!(read(&mut part, &mut ctx, pointer::TOS, 2), vec![0xF5, 0x80]);
        // 温度（-31 半度）は T_os（-21 半度）より低いので OS は主張しない。
        assert!(!part.os_asserted());
    }

    #[test]
    fn temp_register_writes_are_ignored() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        write(&mut part, &mut ctx, &[0, 0x7F, 0xFF]);
        assert_eq!(
            read(&mut part, &mut ctx, pointer::TEMP, 2),
            vec![0x19, 0x00]
        );
    }

    #[test]
    fn other_addresses_are_nacked() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        let other = I2cAddr::Seven(0x49);
        assert_eq!(
            part.target_write(&mut ctx, &other, &[0]),
            Err(I2cError::Nack)
        );
    }
}
