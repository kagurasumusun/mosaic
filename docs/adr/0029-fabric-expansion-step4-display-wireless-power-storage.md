# ADR 0029: ファブリック拡張 step 4 — 表示・無線・音声・電力・ストレージ・計測の 12 種 (44→56)

* 状態: 採用
* 日付: 2026-10-28
* 関連: ADR 0025 (bus 基盤), ADR 0026 (step 1), ADR 0027 (step 3),
  ADR 0028 (crates/ 集約)

## 背景

step 3 (ADR 0027) までで 44 種の接続方式 (fabric) が揃い、ビデオ
(HDMI/DP/DSI/CSI)、入力 (PS/2/GamePort)、音声 (S/PDIF/SoundWire/
SLIMbus/MIDI) をカバーした。残る空白は (1) アナログ系の表示 (VGA/DVI/
LVDS)、(2) 無線・IO コンボ (SDIO)、(3) デジタルマイク (PDM)、(4) 電力・
モーション (PWM)、(5) ストレージの低層 (NAND/UFS/HyperBus/ISA/PCMCIA)、
(6) 計測器 (GPIB) である。いずれも実機に存在する接続方式であり、
QEMU が統一的に扱っていない領域を埋める。

## 決定

1. **12 種の fabric を追加** (bus 44→56 種、contract も同数):
   * `vga` — アナログ RGB + DDC EDID (VgaSource/VgaSink, VgaMode/VgaFrame)
   * `dvi` — シングル/デュアルリンク TMDS + DDC (DviSource/DviSink)
   * `lvds` — ノートパネル LVDS リンク (LvdsSource/LvdsSink, バックライト)
   * `sdio` — SD I/O (関数 0-7, CMD/CMD52) (SdioHost/SdioCard)
   * `pdm` — PDM デジタルマイク (PdmSource/PdmMic, クロック停止検出)
   * `pwm` — PWM チャネル出力 (PwmSource/PwmSink, duty>period 拒否)
   * `nand` — RAW NAND バス (CLE/ALE/RE/WE, NandOp) (NandHost/NandDevice)
   * `isa` — 8/16 ビット ISA ポートサイクル (claim_io デコード)
   * `pcmcia` — 16 ビット PC Card (アトリビュート/CIS + I/O 空間)
   * `ufs` — Universal Flash Storage (UPIU 交換, タスクタグ検証)
   * `hyperbus` — HyperFlash/HyperRAM 16 ビット DDR (線形 + レジスタ空間)
   * `gpib` — IEEE-488 計測器バス (プライマリアドレス 0-30 ルーティング)
2. **part.toml は 12 種ともセクション必須**: `[vga] max-pixel-clock-khz`、
   `[dvi] lanes`、`[lvds] lanes`、`[sdio] functions`、`[pdm] channels`、
   `[pwm] channels`、`[nand] page-size/oob-size/blocks`、
   `[isa] dma-channels`、`[pcmcia] slot (16bit|cardbus)`、`[ufs] lu-count`、
   `[hyperbus] size-mb`、`[gpib] address`。セクションのみで binding 宣言
   (またはその逆) はエラー。
3. **123 件の実モデル** (manifests 1226→1349):
   * display 35: VGA CRT/LCD モニタ 15 (Sony CPD-G520/GDM-FW900、NEC
     MultiSync、IBM P260、iiyama Vision Master Pro 514、EIZO T965 など)、
     DVI モニタ 10 (Dell 2001FP、Apple Cinema Display 20、EIZO S2000 など)、
     LVDS ノートパネル 10 (Samsung LTN154X3、LG LP150X07、AUO B154EW02、
     BOE Hydis HV150UX1、IDTech N150U3 など)
   * net 18: SDIO Wi-Fi 10 (BCM43438/43455、Marvell 88W8686/8787、
     ESP8089、RTL8189ES/8821CS、CYW4343W/43439、TI WL1835)、ISA NIC 3
     (RTL8019AS、3C509B、EtherExpress 16)、PC Card ネット 5
   * audio 12: PDM マイク 8 (ST MP34DT01/IMP34DT05、Knowles SPH0641、
     TDK ICS-43434、Infineon IM69D130、Bosch Akustica AKU240)、ISA サウンド
     カード 4 (Sound Blaster 16/AWE64、Gravis UltraSound ACE、Ad Lib Gold)
   * misc 30: PWM ファン/サーボ 12 (Noctua、ARCTIC、Corsair ML120、
     Sunon MagLev、Delta、TowerPro SG90/MG996R、Futaba S3003、Hitec
     HS-311)、GPIB 計測器 12 (HP 34401A/3458A/33220A/54600B/E3631A、
     Keithley 2000/2400、Tektronix TDS 210、Rigol DM3068、Fluke 45、
     Agilent 34410A)、HyperRAM 3 (S27KS0641、W958D6FB、APS6404L)、
     IBM Microdrive 340
   * flash 24: NAND 10 (Samsung K9F1G08U0E/K9F2G08U0M、Micron MT29F、
     Toshiba TC58BVG0S3、hynix HY27UF、Spansion S34ML01G2、ST NAND01GR3B、
     Winbond W29N01HV、Macronix MX30LF1G18AA)、UFS 8 (Samsung KLUDG4U1DB/
     KLUDG8U1DA、SK hynix HN8T13AAH/HN8TC18AJ、Micron MTFC、KIOXIA
     THGAF9G8T2、WD iNAND 7250)、HyperFlash 3 (S26KL512S/S26KS512S、
     W25Q256JW)
   * char 2、nvram 1 (ISA モデム、PC Card モデム、Pretec SRAM)
4. **origin 39 件追加** (NEC、三菱電機、iiyama、EIZO、HannStar、Chunghwa、
   IDTech、Chi Mei、Knowles、TDK、Noctua、Futaba、Hitec、US Robotics、
   3Com、Xircom、Lucent、NETGEAR、KIOXIA 系 WD、AP Memory、Keithley、
   Tektronix、Rigol、Fluke、Agilent、Tseng Labs、S3、Ad Lib、ATI など)。

## 実装

* contract 12 モジュール (`crates/contract/src/{vga,dvi,lvds,sdio,pdm,
  pwm,nand,isa,pcmcia,ufs,hyperbus,gpib}.rs`)、bus 12 ディレクトリ
  (`crates/bus/src/<name>/{mod,core}.rs`)。各 core は NoopCtx による
  happy path + 空バスエラーの単体テストを持つ (bus 62→96 テスト)。
* xtask: binding 許可リスト 12 件追加 + セクション検証 12 件追加
  (整数は非負検証、`pcmcia.slot` は列挙検証)。
* 既存の `midi binding has no idi] section` タイポを修正。

## 負の影響と緩和策

* **Placeholder スタブ**: 新モデルはカタログ語彙として即利用可能だが、
  応答は共有機能コアの昇格待ち (ADR 0026 step 2 と同じ戦略)。今回の
  123 件は `hdmi_sink_core` 系の共有コアで昇格する想定。
* **ISA/PCMCIA の DMA・割り込み線**: この step ではポートサイクルと
  アトリビュート空間のみ。DMA 転送と IRQ 線は次 step (8237 DMA コント
  ローラ IP との組み合わせ) で扱う。

## 検証

* `cargo build --workspace --all-features` — 緑。
* `cargo test --workspace --lib` — 20 スイート全緑。
* `cargo run -p mosaic-xtask -- check` — OK (manifest ミラー・起源・
  セクション検証込み)。
