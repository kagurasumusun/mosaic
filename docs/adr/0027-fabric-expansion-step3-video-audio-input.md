# ADR 0027: ファブリック拡張 step 3 — ビデオ/オーディオ/入力の接続方式 12 種

Status: accepted
Date: 2026-10-27 (ADR 0026 と同日、step 3 として追記)
Supersedes: none (extends ADR 0026)

## Context

ADR 0026 (step 1+2) で `bus/` ファブリックを 9→32 種に拡張し、1029 件の
実モデルを追加、240 部品を共有機能コアに昇格させた。しかしいくつかの
重要な接続方式がまだ `bus/` に存在しなかった:

* **ビデオ出力**: HDMI / DisplayPort / MIPI DSI
* **カメラ入力**: MIPI CSI-2
* **人間入力**: PS/2 (キーボード・マウス) / GamePort (アナログジョイスティック)
* **デジタルオーディオ**: S/PDIF (IEC 60958) / MIPI SoundWire / SLIMbus
* **周辺バス**: eSPI (Intel Enhanced SPI) / IEEE 1394 (FireWire)
* **音楽機器**: MIDI 1.0 UART

これらは QEMU でも `hw/display/`, `hw/audio/`, `hw/input/` の各サブシステム
に散らばって実装されており、QEMU 自体もファブリック層として統合してい
ない。Mosaic では ADR 0025 の方針 (接続方式はすべて `bus/` に集約) に
従い、これら 12 種を新たに `bus/` ファブリックとして追加する。

## Decision

### 1. 12 新ファブリックの追加 (32 → 44 種)

| ファブリック | 接続方式 | ポート語彙 | 備考 |
|---|---|---|---|
| `hdmi` | HDMI 1.0+ (TMDS+DDC+CEC) | `HdmiSource` / `HdmiSink` | `VideoFrame` (24bpp RGB) / `AudioSample` (8ch IEC60958) / CEC メッセージ / HPD + EDID |
| `displayport` | VESA DisplayPort 1.x | `DisplayPortSource` / `DisplayPortSink` | メインリンク + AUX CH (read/write) + DPCD レジスタ読出し |
| `mipi_dsi` | MIPI DSI 1.x | `DsiHost` / `DsiPeripheral` | `DsiPacket` (VC + Data Type + payload) / Video/Command/Burst モード |
| `mipi_csi` | MIPI CSI-2 | `CsiSource` / `CsiReceiver` | `CsiPacket` (VC + DT) / `CsiDataType` 列挙 (YUV422/Raw8/10/12/14/RGB565/RGB888/Embedded/Generic) |
| `ps2` | PS/2 シリアル | `Ps2Host` / `Ps2Device` | ホスト→デバイス送信 / デバイス→ホスト poll キュー |
| `spdif` | IEC 60958 (S/PDIF) | `SpdifTransmitter` / `SpdifReceiver` | `SpdifFrame` (L/R 24-bit サンプル + VUC ビット) |
| `espi` | Intel eSPI 1.0+ | `EspiMaster` / `EspiTarget` | `EspiPacket` (cycle/tag/length/payload) / 4 サイクル型 (Peripheral/VirtualWire/Oob/Flash) |
| `soundwire` | MIPI SoundWire 1.x | `SoundWireMaster` / `SoundWirePeripheral` | 16-row frame (control+data) / 8ch |
| `slimbus` | SLIMbus 1.x | `SlimbusManager` / `SlimbusDevice` | `SlimbusMessage` (dest/src/code/value) / チャネルスケジューリング |
| `firewire` | IEEE 1394 (FireWire) | `FireWireInitiator` / `FireWireNode` | `FireWirePacket` (source/destination ID + tlabel + tcode + address + data) / 4 tcode (QR/QW/BR/BW) |
| `midi` | MIDI 1.0 UART (31250 baud) | `MidiOutput` / `MidiInput` | `MidiMessage` (status + data1 + data2) |
| `gameport` | IBM PC アナログゲームポート (DA-15) | `GamePortHost` / `GamePortStick` | `GamePortState` (4 軸 + 4 ボタン) / DAC トリガ |

設計規則は ADR 0025 を継承:

* 各ファブリックは `contract/src/<name>.rs` (ポート trait + エラー型 +
  メッセージ語彙) と `bus/src/<name>/{mod.rs, core.rs}` (ファブリック实体:
  仲裁・アドレス指定・配信) のペアで構成;
* ファブリックは `hw`/`machine`/`assemble`/`runtime` に依存しない
  (下位レベルのみ);
* ファブリックは仲裁・アドレス指定・配信のみを所有し、ゲスト可視の
  レジスタは持たない (レジスタは `hw/` のデバイスが持つ);
* デバイスもエンジンもファブリックに直接依存せず、`contract` を経由する。

各ファブリックには `bus/src/<name>/core.rs` の末尾に `#[cfg(test)] mod tests`
ブロックを追加し、NoopCtx + ダミーターゲット/ソースでハッピーパスと
空バスエラーの 2 ケースをカバーする (既存 `i3c`/`i2c`/`spi` ファブリック
と同じパターン)。`bus` クレートのテストは 38 → 62 件 (+24)。

### 2. xtask catalog の新バインディング対応

`[part]` manifest の `bindings` に以下を追加許可:

* **セクション必須** (5 種): `ps2`, `spdif`, `espi`, `midi`, `gameport`
  * `[ps2]`: `kind` (keyboard|mouse), `device-id` (optional)
  * `[spdif]`: `mode` (consumer|professional), `rate`
  * `[espi]`: `cycles` (optional), `chip-select`
  * `[midi]`: `direction` (in|out|both), `channels`
  * `[gameport]`: `axes`, `buttons`
* **セクション不要** (7 種): `hdmi`, `displayport`, `mipi_dsi`, `mipi_csi`,
  `soundwire`, `slimbus`, `firewire`
  * これらはポイントツーポイントまたはコントローラ主導の接続方式で、
    バスアドレスやストラップピンが存在しないため、`[<name>]` セクション
    を持たない。`part.bindings` に名前を宣言するだけでグラフがルーティ
    ングに使える。

### 3. 新ファブリックを消費する実モデルの追加 (178 件)

新ファブリックを使う実部品を 5 つのカテゴリに追加 (`hw/src/<cat>/<model>.rs`
+ `manifests/hw/<cat>/<model>.toml` のペア、`Placeholder` スタブ):

| カテゴリ | ファブリック | 件数 | 例 |
|---|---|---|---|
| display | hdmi | 20 | Dell S2722QC / LG 27GL850 / BenQ PD2700U / ASUS PG279QM / Samsung Odyssey G7 / HP Z27 / MSI Optix MPG27CQ / ViewSonic VP2785 / AOC Agon AG273QX / Gigabyte M27Q |
| display | displayport | 10 | Dell U3219C / Dell U2720Q / LG 32UN880 / BenQ PD3205U / ASUS PA329CV / Acer ConceptD CP7 / Samsung ViewFinity S9 / HP Z32 / MSI Creator PS321URV / ViewSonic VP3268 |
| display | mipi_dsi | 20 | Raspberry Pi 7" タッチスクリーン / Waveshare 5"/7" DSI / JDI LT070ME05000 / Sharp LS055R1SX04 / BOE TV101WUM-NL6 / Innolux P079ZCA / AUO B101UAN02 / LG ACX424AKM AMOLED / Samsung S6E3FA5 / Truly TD4322 |
| sensor | mipi_csi | 25 | Sony IMX219/477/296/327/462/519 / OmniVision OV5640/5647/9281/9282/64A40/08D10 / onsemi AR0144/0234/0521/1335/MT9M001/MT9V022 / GalaxyCore GC2053/4653 / SmartSens SC2232/223A/200AI/500AI |
| input | ps2 | 20 | IBM Model M / Model M2 / Space Saver / ThinkPad T42 / Logitech Deluxe 250 / MX3200 / Wheel Mouse / Microsoft Natural 4000 / IntelliMouse / Basic Mouse / Logitech PS/2 Mouse / TrackPoint II / ALPS GlidePoint / Synaptics touchpad / Dell SK-8125 / HP KU-0316 / Keytronic E03601P2 / Chicony KB-9810 / Sun Type 6 / Compaq PS/2 |
| input | gameport | 12 | Microsoft SideWinder 3D Pro / FFB Pro / Precision Pro / Logitech WingMan Extreme / Warrior / CH Products F-16 Combatstick / Flightstick / ThrustMaster F-22 Pro / Gravis GamePad Pro / Saitek X-36 / Logitech ThunderPad Digital / Guillemot Falcon |
| audio | spdif | 14 | Asahi Kasei AK4114/4117/4384/4430 / Cirrus CS8416/8406/8422 / TI DIR9001/DIX4192/PCM9211 / Wolfson WM8804/8805 / ESS ES9023/9028 |
| audio | midi | 15 | Roland MT-32/SC-55/SC-88/SC-8820 / Yamaha MU50/MU80/DX7 / Korg M1/05R/W / Alesis NanoPiano / E-mu Proteus FX / Akai S3000XL / M-Audio MIDISPORT 2x2 / Focusrite Scarlett 2i2 MIDI / Behringer UMC202HD MIDI |
| audio | soundwire | 14 | Realtek RT5682/5682S/700/711/711-SDCA/1308/1316/715/715-SDCA / Maxim MAX98373/98390 / Qualcomm WSA8810/8830/8835 |
| audio | slimbus | 6 | Qualcomm WCD9335/9340/9360/9375 / Tasha WCD9335 / Tavil WCD9341 |
| misc | espi | 12 | ITE IT8728F/8528/8528E / Nuvoton NPCD390/NPCD376 / SMSC SCH5627/MEC1308 / Intel SPT LPSS SPI / Microchip MEC1428/1633 / ENE KB3926 / ASPEED AST2600 eSPI BMC bridge |
| misc | firewire | 10 | Seagate 120GB SBP-2 / WD 250GB SBP-2 / Pioneer DVR-109 DVD-RW / Sony DCR-TRV900 DV / Canon XL1 DV / MOTU 828 mkI / Focusrite Saffire Pro 40 / PreSonus FireStudio / Apple iPod 1G / LaCie Porsche FW800 |

合計 178 件の新モデル。各モデルの要約には実製品名・メーカー・解像度
または機能の要点を記載し、`provenance = { doc = 'vendor datasheet',
confidence = 'medium' }` で出処を明示。

### 4. 新たな正規 origin の追加 (42 件)

`manifests/origins.toml` に以下のメーカーを追加:

* **ディスプレイ**: Dell / LG / BenQ / ASUS / Acer / MSI / ViewSonic /
  AOC / GIGABYTE / Waveshare / JDI / BOE / Innolux / AUO
* **カメラセンサ**: OmniVision / GalaxyCore / SmartSens
* **入力**: Key Tronic / Chicony / Sun / ALPS / CH Products /
  Thrustmaster / Gravis / Saitek / Guillemot
* **オーディオ**: Asahi Kasei / Roland / Korg / Alesis / E-mu / AKAI /
  M-Audio / Focusrite / Behringer / MOTU / PreSonus
* **ストレージ/その他**: Apple / LaCie / Compaq / HP / WD / Pioneer /
  Canon / Qualcomm

## Consequences

### 正の影響

* **ファブリック網羅**: QEMU が個別に扱うビデオ出力 (HDMI/DP/DSI)・
  カメラ入力 (CSI-2)・人間入力 (PS/2/GamePort)・デジタルオーディオ
  (S/PDIF/SoundWire/SLIMbus)・周辺バス (eSPI/FireWire)・音楽機器 (MIDI)
  をすべて `bus/` の統一ファブリック層に集約。QEMU にはない統一構造。
* **実モデル規模**: manifests/hw 配下の合計は 1048 → 1226 件 (+178)。
  オーナー指示「500 くらい」を大きく上回る。
* **テスト**: `bus` クレート 38 → 62 件 (+24)。`hw` 254 件・`boot` 58
  件・`xtask` 21 件は維持。`cargo build --workspace --all-features` 緑、
  `cargo test --workspace --all-features --lib` 全緑、`xtask check` OK。
* **バインディング語彙**: xtask が 13→20 バインディングを認識
  (既存 8 + step 3 セクション必須 5 + step 3 セクション不要 7)。

### 負の影響と緩和策

* **Placeholder スタブ**: 新モデルはすべて `Placeholder` 構造体のみで、
  ファブリックに接続しても応答しない。これは ADR 0026 step 2 と同じ
  戦略 (実装は後から共有機能コアで昇格) で、ボード構成の語彙としては
  即利用可能。
* **warning 403 件**: ADR 0026 から変わらず unused import 系。次ステップ
  で `cargo fix --lib` で自動解決可能。
* **新 fabric の IP コントローラ未登録**: HDMI/DP/DSI/CSI/SoundWire/
  SLIMbus のコントローラ (例: Intel TGL HDMI TX / RPi DSI host /
  Qualcomm CSI receiver / Realtek SoundWire master) はまだ `hw/` に
  カタログ登録されていない。次フェーズで追加する前提。

### 今後の方向

* 新 fabric の IP コントローラ追加 (HDMI TX/RX / DP AUX / DSI host /
  CSI receiver / SoundWire master / SLIMbus manager / FireWire OHCI /
  MIDI UART / GamePort DAC)。
* 新 fabric に繋がる leaf part の共有機能コア昇格 (HDMI sink EDID /
  DSI panel コマンド / CSI センサ frames / PS/2 scancode / MIDI
  statusバイト / GamePort 軸サンプリング)。
* 既存 `bus` ファブリックと同様に、新 fabric を使うボード manifest
  (例: RPi 4 with DSI 7" タッチスクリーン + CSI V2 カメラ; x86 PC with
  HDMI mon + PS/2 kbd/mouse + GamePort ジョイスティック)。
* warning 403 件の `cargo fix --lib` による一括解決。
