#![no_std]
#![forbid(unsafe_code)]
//! TI TCA9548A I2C スイッチ（8 チャネル）の機能モデル（P2 / ADR 0008）。
//!
//! データシート準拠の意味論:
//! - 制御レジスタは 1 バイトのみ。書き込みフレームの先頭バイトがチャネル
//!   イネーブルのビットマスク（bit n = チャネル n）、読み出しは同じ値を
//!   返す。電源投入時は 0x00（全チャネル切断）。
//! - イネーブルされたチャネルのダウンストリーム部品だけが上流から見える:
//!   アドレス一致判定は「自分のアドレス、またはイネーブル済みチャネルの
//!   いずれかの部品が一致するとき」だけ真になる。
//! - 自分宛てではないフレームは、一致するイネーブル済みチャネルの部品へ
//!   そのまま転送される（パススルー）。
//!
//! 機能モデルの簡略化: アドレス変換・リセットピン・割り込みは modeled せず、
//! チャネルごとの部品は構築時に注入する（ボードの合成根が配線する）。

extern crate alloc;

use alloc::boxed::Box;
use mosaic_contract_i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract_model::Ctx;
use mosaic_core_types::{Fnv64, Snapshot, SnapshotError};

/// チャネル数（SD0..SD7）。
pub const CHANNELS: u8 = 8;

/// 構築・操作エラー。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Tca9548aError {
    /// チャネル番号が 0..8 を超えた。
    ChannelOutOfRange,
}

/// 構築パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct Tca9548aParams {
    /// 上流から見える自分のアドレス（実機は A0-A2 ピンで 0x70..0x77）。
    pub address: I2cAddr,
}

impl Tca9548aParams {
    pub const fn new(address: I2cAddr) -> Self {
        Self { address }
    }
}

/// TCA9548A スイッチ。`downstream[n]` がチャネル n の部品。
pub struct Tca9548a {
    address: I2cAddr,
    control: u8,
    downstream: [Option<Box<dyn I2cTarget>>; CHANNELS as usize],
}

impl Tca9548a {
    /// 全チャネル未接続・制御レジスタ 0x00 で構築する。
    pub fn new(params: Tca9548aParams) -> Self {
        Self {
            address: params.address,
            control: 0,
            downstream: core::array::from_fn(|_| None),
        }
    }

    /// チャネルに部品を接続する。再アタッチは前の部品を置き換える。
    pub fn attach(&mut self, channel: u8, target: Box<dyn I2cTarget>) -> Result<(), Tca9548aError> {
        if channel >= CHANNELS {
            return Err(Tca9548aError::ChannelOutOfRange);
        }
        self.downstream[usize::from(channel)] = Some(target);
        Ok(())
    }

    /// 現在の制御レジスタ値。
    pub fn control(&self) -> u8 {
        self.control
    }

    /// チャネルがイネーブルされているか。
    pub fn channel_enabled(&self, channel: u8) -> bool {
        channel < CHANNELS && self.control & (1 << channel) != 0
    }

    /// 転送先（一致するイネーブル済みチャネルの部品）を探す。
    fn route(&mut self, addr: &I2cAddr) -> Option<usize> {
        let mask = self.control;
        self.downstream
            .iter_mut()
            .enumerate()
            .find(|(channel, target)| {
                mask & (1 << *channel as u8) != 0
                    && target.as_ref().is_some_and(|t| t.matches(addr))
            })
            .map(|(channel, _)| channel)
    }
}

impl I2cTarget for Tca9548a {
    fn matches(&self, addr: &I2cAddr) -> bool {
        *addr == self.address
            || self.downstream.iter().enumerate().any(|(channel, target)| {
                self.control & (1 << channel as u8) != 0
                    && target.as_ref().is_some_and(|t| t.matches(addr))
            })
    }

    fn target_write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        if *addr == self.address {
            // 制御レジスタは 1 バイト。先頭バイトだけ受け取る。
            if let Some(&mask) = bytes.first() {
                self.control = mask;
            }
            return Ok(Ack::Ack);
        }
        match self.route(addr) {
            Some(channel) => {
                let target = self.downstream[channel]
                    .as_mut()
                    .expect("route は存在するチャネルだけ返す");
                target.target_write(ctx, addr, bytes)
            }
            None => Err(I2cError::Nack),
        }
    }

    fn target_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError> {
        if *addr == self.address {
            for slot in buf.iter_mut() {
                *slot = self.control;
            }
            return Ok(Ack::Ack);
        }
        match self.route(addr) {
            Some(channel) => {
                let target = self.downstream[channel]
                    .as_mut()
                    .expect("route は存在するチャネルだけ返す");
                target.target_read(ctx, addr, buf)
            }
            None => Err(I2cError::Nack),
        }
    }
}

/// Switch checkpoint. Downstream targets are host attachments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tca9548aCheckpoint {
    pub schema: u32,
    pub control: u8,
}

impl Snapshot for Tca9548a {
    type State = Tca9548aCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Tca9548aCheckpoint {
            schema: Self::SCHEMA_VERSION,
            control: self.control,
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
        self.control = state.control;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.byte(state.control);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use mosaic_contract_model::ContextError;

    /// ポインタ 1 レジスタだけの最小部品（転送の観察用）。
    struct PointerPart {
        address: I2cAddr,
        pointer: u8,
    }

    impl PointerPart {
        fn new(address: u8) -> Self {
            Self {
                address: I2cAddr::Seven(address),
                pointer: 0,
            }
        }
    }

    impl I2cTarget for PointerPart {
        fn matches(&self, addr: &I2cAddr) -> bool {
            *addr == self.address
        }
        fn target_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            bytes: &[u8],
        ) -> Result<Ack, I2cError> {
            if let Some(&first) = bytes.first() {
                self.pointer = first;
            }
            Ok(Ack::Ack)
        }
        fn target_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            buf: &mut [u8],
        ) -> Result<Ack, I2cError> {
            for slot in buf.iter_mut() {
                *slot = self.pointer;
                self.pointer = self.pointer.wrapping_add(1);
            }
            Ok(Ack::Ack)
        }
    }

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

    fn mux() -> Tca9548a {
        Tca9548a::new(Tca9548aParams::new(I2cAddr::Seven(0x70)))
    }

    #[test]
    fn power_on_disconnects_every_channel() {
        let device = mux();
        assert_eq!(device.control(), 0x00);
        for channel in 0..CHANNELS {
            assert!(!device.channel_enabled(channel));
        }
    }

    #[test]
    fn control_register_write_and_read_back() {
        let mut device = mux();
        let mut ctx = CtxStub;
        let own = I2cAddr::Seven(0x70);
        device.target_write(&mut ctx, &own, &[0x08, 0xFF]).unwrap();
        // 先頭バイトだけ受け取る: 2 バイト目以降は無視される。
        assert_eq!(device.control(), 0x08);
        assert!(device.channel_enabled(3));
        let mut buf = [0u8; 2];
        device.target_read(&mut ctx, &own, &mut buf).unwrap();
        assert_eq!(buf, [0x08, 0x08]);
    }

    #[test]
    fn downstream_parts_are_invisible_until_enabled() {
        let mut device = mux();
        let mut ctx = CtxStub;
        device.attach(3, Box::new(PointerPart::new(0x77))).unwrap();
        let part_addr = I2cAddr::Seven(0x77);
        assert!(!device.matches(&part_addr));
        assert_eq!(
            device.target_write(&mut ctx, &part_addr, &[1]),
            Err(I2cError::Nack)
        );
        device
            .target_write(&mut ctx, &I2cAddr::Seven(0x70), &[1 << 3])
            .unwrap();
        assert!(device.matches(&part_addr));
        device.target_write(&mut ctx, &part_addr, &[0x20]).unwrap();
        let mut buf = [0u8; 1];
        device.target_read(&mut ctx, &part_addr, &mut buf).unwrap();
        assert_eq!(buf[0], 0x20);
    }

    #[test]
    fn disabled_channels_do_not_leak_forwarded_frames() {
        let mut device = mux();
        let mut ctx = CtxStub;
        device.attach(0, Box::new(PointerPart::new(0x50))).unwrap();
        device.attach(1, Box::new(PointerPart::new(0x50))).unwrap();
        // 同じアドレスの部品が 2 チャネルにあっても、イネーブルされた
        // 側のインスタンスだけがフレームを受け取る。
        device
            .target_write(&mut ctx, &I2cAddr::Seven(0x70), &[1 << 1])
            .unwrap();
        device
            .target_write(&mut ctx, &I2cAddr::Seven(0x50), &[0x42])
            .unwrap();
        // チャネル 0 へ切り替えて読む: 漏れていればポインタが進んでいる。
        device
            .target_write(&mut ctx, &I2cAddr::Seven(0x70), &[1 << 0])
            .unwrap();
        let mut buf = [0u8; 1];
        device
            .target_read(&mut ctx, &I2cAddr::Seven(0x50), &mut buf)
            .unwrap();
        assert_eq!(buf[0], 0x00);
    }

    #[test]
    fn attach_is_bounds_checked() {
        let mut device = mux();
        assert_eq!(
            device.attach(CHANNELS, Box::new(PointerPart::new(0x50))),
            Err(Tca9548aError::ChannelOutOfRange)
        );
    }
}
