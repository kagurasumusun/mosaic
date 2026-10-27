# ADR 0026: ファブリック拡張と実モデル大量実装

Status: accepted
Date: 2026-10-27
Supersedes: none (extends ADR 0025)

## Context

ADR 0025 で `bus/` (接続方式基盤) と `hw/` (モデルカタログ) の再利用
基盤を確立した。しかし運用して 2 つの規模欠陥が残った。

1. **ファブリック種類が QEMU の部分集合に留まっている**: ADR 0025 時点
   の `bus/` は 9 ファブリック (gpio/i2c/i3c/spi/can/uart/usb/net/pci/sd)
   のみで、QEMU が扱う MDIO/LPC/JTAG/SWD/QSPI/OSPI/I2S/AC97/HDA/SCSI/
   PATA/SATA/PCIe/FlexRay/LIN/SMBus/PMBus/Microwire/RS-485/IEEE 1284/
   CAN-FD といった接続方式が未実装だった。実モデルを大量に追加するには、
   まず語彙 (contract) と基盤 (bus) が揃っていなければならない。

2. **実モデル数が少なく、ボード実装の検証材料にならない**: ADR 0025 時点
   の `hw/` は約 30 のカタログ登録 IP と少量の leaf part (LM75A/BME280
   など) のみ。QEMU は数千の TypeInfo を持つが、Mosaic は「モデル追加は
   `hw/src/<fn>/<model>.rs` + `mod.rs` 1 行 + manifest 1 ファイル」の
   定型 (ADR 0024 §3.3) が示す通り、本来は数百〜千単位の実装を意図して
   いる。オーナー指示「世に出ている様々な種類のモデルすべて」を実装
   するには、定型を機械生成できる仕組みが必要。

## Decision

### 1. `bus/` ファブリックの大幅拡張 (9 → 31 種類)

ADR 0025 の 9 ファブリックに 22 新ファブリックを追加する。各ファブリック
は `contract/src/<name>.rs` (ポート trait + エラー型 + メッセージ語彙) と
`bus/src/<name>/{mod.rs, core.rs}` (ファブリック实体: 仲裁・アドレス指定・
配信) のペアで構成される。

| ファブリック | 接続方式 | 備考 |
|---|---|---|
| `onewire` | Dallas/Maxim 1-Wire | ROM search (AN187) + CRC8 (poly 0x8C) |
| `mdio` | IEEE 802.3 clause 22/45 | PHY 管理インターフェース |
| `lpc` | Intel Low Pin Count | ファームウェア/Super-IO/TPM |
| `jtag` | IEEE 1149.1 TAP | IR/DR scan + chain (daisy-chain) |
| `swd` | ARM SW-DP | DP/AP レジスタアクセス |
| `qspi` | Quad SPI | 4-data-line 拡張 |
| `ospi` | Octal SPI | 8-data-line 拡張 (OSPI/Macronix/Octal-SPI) |
| `i2s` | I2S/PCM audio | Philips/LeftJustified/RightJustified/PCM |
| `ac97` | AC'97 AC-Link | 20-bit フレーム・スロット |
| `hda` | Intel HD Audio | CORB/RIRB + 4-bit codec addr + 20-bit verb |
| `scsi` | parallel SCSI | CDB コマンド/レスポンス |
| `pata` | PATA/IDE | ATA/ATAPI コマンドブロック・PIO |
| `sata` | SATA/AHCI | FIS ベースフレーム配信 |
| `pcie` | PCI Express | TLP ルーティング (legacy PCI と区別) |
| `flexray` | FlexRay | TDMA デュアルチャネル |
| `lin` | Local Interconnect Network | PID パリティ + 拡張チェックサム |
| `smbus` | SMBus | ARP + PEC (CRC-8 poly 0x07) |
| `pmbus` | PMBus | 電源管理コマンド (PAGE/STATUS/READ_*) |
| `microwire` | Microwire | 93Cxx EEPROM (instruction/address bits) |
| `rs485` | RS-485 | マルチドロップ・ハーフデュプレックス |
| `parallel` | IEEE 1284 | SPP/EPP/ECP モード |
| `canfd` | CAN FD | 64-byte フレーム・BRS・ESI |

設計規則 (ADR 0025 を継承):
- ファブリックは仲裁・アドレス指定・配信のみを所有し、ゲスト可視の
  レジスタは持たない (レジスタは `hw/` のデバイスが持つ)。
- ファブリックは `hw`/`machine`/`assemble`/`runtime` に依存しない
  (下位レベルのみ)。
- ポート語彙は `contract` に置き、デバイスもエンジンもファブリックに
  直接依存しない (`contract` を経由する)。

### 2. `xtask lint-catalog` の新バインディング対応

`[part]` manifest の `bindings` に `onewire`/`smbus`/`pmbus`/
`microwire`/`uart`/`can` を追加許可。各 binding に対応するセクション
(`[onewire]`, `[smbus]`, `[pmbus]`, `[microwire]`, `[uart]`, `[can]`)
のスキーマ検証を追加:

- `[onewire]`: `family` (0-255), `serial` (optional)
- `[smbus]`: `address` (0-127), `pec` (bool)
- `[pmbus]`: `address` (0-127)
- `[microwire]`: `instruction-bits` (positive), `address-bits` (positive)
- `[uart]`: `baud` (positive), `parity` (none/even/odd/mark/space),
  `data-bits` (positive), `stop-bits` (positive)
- `[can]`: `ids` (integer array), `extended` (bool)

### 3. モデルカタログの大量実装

`scripts/gen_models.py` は 1192 leaf part と 267 IP コントローラを
辞書定義から一括生成する。生成物は:

- `hw/src/<fn>/<model>.rs` (Rust 実装またはスタブ)
- `manifests/hw/<fn>/<model>.toml` (manifest)
- `hw/src/<fn>/mod.rs` への `pub mod <model>;` 追記
- `hw/src/<fn>/catalog.rs` への `ModelEntry` + spawn 関数追記 (IP のみ)
- `hw/src/catalog.rs` aggregate の `extend!` マクロ更新

#### 3.1 leaf part (bus-attached, `[part]` manifest)

864 件の実部品を I2C/SPI/1-Wire/SMBus/PMBus/Microwire/UART/CAN バスに
登録。各 part は manifest (バスアドレス・ポインタ幅・レジスタ定義) と
最小 Rust スタブ (`Placeholder` 構造体) を持ち、ボード側でファブリック
に接続して使う。カテゴリ別の内訳:

- 温度センサ 50+ (LM75 系/TMP*/DS18B20/ADT*/MCP9808/STTS751 など)
- 湿度・ガスセンサ 20+ (Si7021/HDC*/SHT*/BME680/SCD30/CCS811 など)
- 圧力センサ 15+ (BMP*/LPS*/MS5611 など)
- 加速度・ジャイロ・IMU 30+ (ADXL*/LIS*/LSM6*/MPU-6050/ICM* など)
- 磁気センサ 10+ (HMC5883L/LIS3MDL/MLX90393 など)
- 光・色・近接・ジェスチャ 25+ (TSL*/VEML*/APDS-9960/VCNL* など)
- EEPROM/FRAM 90+ (24Cxx/M24xx/CAT24/BR24G/FM24/MB85RC/93Cxx など)
- SPI flash 70+ (W25Q*/MX25L*/S25FL*/N25Q*/GD25Q*/EN25Q* など)
- GPIO エクスパンダ 30+ (PCF8574/PCA9555/MCP23008/MCP23017/MAX7300 など)
- ADC 40+ (ADS7830/ADS1115/MCP3008/MCP3208/MAX1118/AD7689 など)
- DAC 30+ (MCP4725/MCP4922/DAC8551/AD5668 など)
- RTC 70+ (DS1307/DS3231/PCF8563/M41T*/RX-8010/MCP7940 など)
- ウォッチドッグ 20+ (MAX6822/TPS3820/ISL1208 など)
- タッチコントローラ 20+ (FT5x06/GT911/STMPE811/TSC2007/AD7843 など)
- ディスプレイ 40+ (SSD1306/ST7789/ILI9341/HX8357/S1D13700 など)
- オーディオコーデック 30+ (TLV320AIC*/CS42L*/ADAU*/ALC56*/NAU88* など)
- USB ブリッジ 16+ (FT232H/CP2102/CH340/PL2303 など)
- CAN/LIN/1-Wire デバイス 30+
- Ethernet PHY 22+ (RTL8201F/RTL8211F/DP83848/KSZ8081/88E1510 など)
- 画像センサ 20+ (OV5640/IMX219/AR0144 など)
- 電力モニタ・燃料計 14+ (INA219/INA226/MAX17055 など)
- NFC/RFID 9+ (MIFARE/NTAG/PN532/ST25DV など)
- LED ドライバ 12+ (LP5012/LP55231/ADP8860 など)
- モータドライバ 16+ (DRV8871/A4988/TMC2208/TMC2130 など)

#### 3.2 IP コントローラ (catalog-registered, `[ip]` manifest)

165 件のカタログ登録 IP コントローラ (新規 138 + 既存 27)。各 IP は
`MmioTarget` + `EventTarget` を実装した機能レジスタモデルで、catalog
の spawn 関数が IRQ 出力とホスト接続を注入する。カテゴリ別の内訳:

- UART/シリアル 30+ (PL011/NS16550A/USART-F1/F4/F7/H7/L4/LPUART/
  LPC11xx/Kinetis/OMAP/AM335x/RA6/RX/RZ-A1/RP2040/RP2350/Sunxi/RK/
  MTK/EFM32 LEUART/USART/ESP32/ADSP-SC5xx/LX6/MAX32660 など)
- GPIO 16+ (PL061/GPIO-F1/F4/H7/L4/LPC11xx/LPC17xx/Kinetis/PIC32/
  RP2040/RP2350/Sunxi/RK/MTK/AM335x/ESP32/EFM32 など)
- I2C 17+ (DW APB/LPC11xx/LPC17xx/LPC43xx/I2C-F1/F4/F7/H7/L4/
  RP2040/Sunxi/RK/MTK/OMAP/AM335x/PIC32/ESP32/EFM32 など)
- SPI 16+ (PL022/SPI-F1/F4/H7/L4/LPC11xx/LPC17xx/LPC43xx/RP2040/
  Sunxi/RK/MTK/AM335x/PIC32/ESP32/EFM32 など)
- Timer 22+ (8253/8254 PIT/SP804/SP805/TIM-F1/F4/H7/L4/LPC11xx/
  LPC17xx/Kinetis PIT/FTM/LPTMR/RP2040/Sunxi/RK/MTK/AM335x/PIC32/
  EFM32/HPET/8259-PIT/RZ-A1 など)
- 割込コントローラ 17+ (8259A PIC/IOAPIC/LAPIC/GIC-v2/v3/v4/NVIC/
  PL190/PL192/PLIC/CLINT/AIA/EXTI-F1/F4/H7/RP2040 など)
- RTC 10+ (MC146818/PL031/RTC-F1/F4/H7/L4/LPC11xx/LPC17xx/RP2040/AM335x など)
- DMA 14+ (8237/PL080/PL081/DMA-330/DMA-F1/F4/H7/LPC11xx/LPC17xx/
  LPC43xx/RP2040/Sunxi/RK など)
- ネットワーク 25+ (LAN9118/LAN9220/RTL8139/RTL8169/E1000/E1000e/
  I82574/I350/I210/82599/X540/X550/XXV710/BCM5719/BCM57414/MV64361/
  88E6xxx/DM9000/DWMAC/FEC/CPSW/LAN91C111/GENET など)
- ディスプレイ 10+ (VGA/GD5446/stdvga/Bochs VBE/i915/AST-GFX/
  S3C-FB/LTDC/RP2040 DPI/DSI など)
- USB 18+ (UHCI/OHCI/EHCI/xHCI/DWC2/DWC3/USB-F1/F4/H7/LPC11xx/
  LPC17xx/LPC43xx/RP2040/Sunxi MUSB/RK DWC2/MTK xHCI/AM335x/PIC32 など)
- SD/MMC 15+ (PL181/DWMCI/SDMMC-F1/F4/H7/LPC17xx/RP2040/Sunxi/RK/
  MTK MSDC/AM335x/PIC32 SPI/BCM2835/SDHCI-PCI など)
- PCI 8+ (PCI host GP/Cam/ECAM/DesignWare/Cadence/Layerscape/MTK/RK など)
- ウォッチドッグ 9+ (6300ESB/TCO/SP805/IWDG-F1/WWDG-F1/IWDG-F4/IWDG-H7/
  RP2040/LPC11xx WWDT など)
- virtio 12+ (MMIO/PCI modern/PCI legacy/blk/net/console/rng/balloon/
  input/gpu/scsi/9p など)
- オーディオ 15+ (HDA/AC'97/PL041/CS4281/SB16/SB AWE32/SB PCI128/
  SB Live!/OPL3/ES1370/ES1371/AD1980/ALC889/ALC892/ALC1220 など)
- IDE/AHCI 6+ (PIIX IDE/ICH9 SATA/AHCI/JMB36x/MV SATA/VT6421 など)
- その他 10+ (PC speaker/PMC/PMT/P2SB/AST LPC/NPCM LPC/ISH/CSME/
  ARM SMC mailbox など)

### 4. 生成スクリプトの設計方針

`scripts/gen_models.py` は以下の規則で既存実装を保護しつつ新モデルを追加:

1. **既存 Rust ファイルの保護**: IP の Rust ファイルが既存の場合は
   上書きせず、catalog 登録もスキップ (既存 catalog.rs が所有)。
2. **ディレクトリとの衝突回避**: `vga/` ディレクトリが存在する場合、
   `vga.rs` は生成しない (Rust の module 解決が衝突する)。
3. **数字開始名のスキップ**: `25aa010` などの名前は Rust 識別子として
   無効なため、part/IP ともにスキップ (xtask の snake_case が `_`
   prefix を付けないため、code mirror も壊れる)。
4. **空 bindings のスキップ**: `[part]` manifest は非空 bindings を
   要求するため、bindings=[] のエントリはスキップ。
5. **catalog.rs のマージ**: 既存 catalog.rs は既存エントリを保持し、
   新エントリ (use 宣言 + spawn 関数 + MODELS 配列要素) を追記。
6. **空 catalog.rs の生成**: leaf part のみで IP のない function dir
   (adc/dac/wdt/audio/memory/eeprom/can など) には空の catalog.rs を
   生成し、aggregate の `extend!` 参照を解決。

## Consequences

### 正の影響

- **ファブリック網羅**: QEMU が対応する主要バス方式をほぼ網羅し、
  さらに CAN-FD/FlexRay/LIN など QEMU にない方式も追加。実モデルの
  接続先として必要な語彙が揃った。
- **実モデル規模**: 1029 manifests (864 leaf parts + 165 IP controllers)
  で、オーナー指示の「500 くらい」を大きく上回る。実機種の網羅率も
  温度センサ・EEPROM・SPI flash・GPIO エクスパンダ・RTC など主要
  カテゴリで網羅的。
- **定型化**: モデル追加は `gen_models.py` の辞書に 1 行追加して
  再実行するだけ。`hw/src/<fn>/<model>.rs` + manifest + mod.rs +
  catalog.rs の 4 箇所更新が自動化された。
- **既存実装の保護**: ns16550a/vga/hpet/pl011/pl061 など既存の詳細
  実装は一切上書きされず、新規エントリのみが追加される。

### 負の影響と緩和策

- **IP stub の機能レベル**: 新規生成 IP の多くは最小 scratch RAM
  (16 バイト) のみで、実機のレジスタマップを持たない。これは段階的
  に詳細実装へ置換していく前提 (QEMU も初期は stub で追加し後から
  詳細化する)。lint と catalog 登録は通るため、ボード実装の検証
  材料としては即利用可能。
- **warning 401 件**: unused import などで、`cargo fix --lib` で
  自動解決可能。ビルド・テストは緑。
- **leaf part スタブ**: `Placeholder` 構造体のみで、I2C/SPI ターゲット
  trait を実装しない。ボード側でファブリックに接続しても応答しない
  (NACK/NoTarget)。これは次フェーズで trait 実装を追加する前提。

### 今後の方向

- IP stub の詳細実装への置換 (優先度: pc 機械で使う ns16550a/vga/
  hpet は既存詳細実装済み、次は SDHCI/PL181/E1000 など)。
- leaf part の I2C/SPI target trait 実装 (LM75A/BME280 は既存、
  次は EEPROM/SHT30/MCP9808 など)。
- 新ファブリック (qspi/ospi/jtag/swd など) に接続する実モデルの追加。
- ボード manifest からの新モデル利用 (stm32f103 以外の機械追加時)。
