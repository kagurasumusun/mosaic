# ADR 0031: ハードウェアモデルの責務別分類 — `hw` を `ip / part / core` へ再編する

* 状態: 採用
* 日付: 2026-10-28
* 関連: ADR 0024 (QEMU 型ディレクトリ軸), ADR 0025 (bus/boot 基盤),
  ADR 0026〜0029 (ファブリック拡張と大量モデル), ADR 0028 (crates/ 集約)

## 背景

ADR 0024 は `hw/src/<function>/` の QEMU 型機能軸を導入し、ADR 0026〜0029
が 56 ファブリック・1349 モデルへ一気に拡張した。その結果、機能軸が
3 つの構造問題を露呈させた。いずれもオーナー指示 (2026-10-28)
「モデルの分類では、I²C、GPIO、I3C などのファブリックやインターフェース
による分類だけに依存しないでください」「同一の部品モデルが複数の場所に
重複して実装されたり、複数の分類にまたがることで管理方法が不統一に
なったりしないようにしてください」が指摘する問題である。

### P1: 機能ディレクトリが「ファブリック」と「機能」を混ぜた

`i2c` バインディングを持つ部品は 12 の機能ディレクトリに散在し、
`spi` は 13 ディレクトリに散在する。配置先の判断基準が部品ごとに異なる:

| 部品 | 実装場所 (旧) | 判断の揺れ |
| --- | --- | --- |
| MCP23017 (I2C GPIO エキスパンダ) | `gpio/` | GPIO コントローラ (PL061) と同居 |
| LM75A (I2C 温度センサ) | `sensor/` | 機能で分類 |
| TCA9548A (I2C バススイッチ) | `i2c/` と `misc/` に二重 | ファブリックで分類、さらに重複 |
| DS1307 (I2C RTC) | `rtc/` | 機能で分類 |
| IS25WP016 (SPI NOR) | `eeprom/` | 一方 W25Q16 (SPI NOR) は `flash/` |

`gpio/` は GPIO コントローラ IP 17 件とエキスパンダ部品 33 件が混在し、
`i2c/` はコントローラ IP 18 件とバススイッチ部品が混在する。MCP2515
(SPI 接続の外付け CAN コントローラ) は `can/` に置かれ、コントローラ IP
と外付けチップの区別がどこにも現れない。

### P2: `misc` が受け皿として肥大した

`misc/` は 210 モデル・次のような異質な集合になった: 温度・電圧・電流
モニタ (ADM1021, LM78/85/90, MAX6697)、バッテリ管理 (BQ24/27/40)、
LED ドライバ (LP55xx)、モータドライバ (TMC5160, DRV8301)、PWM ファン
(Noctua, Arctic)、GPIB 計測器 (HP 34401A, Keithley 2400)、TPM・Secure
Element (ATECC608A)、NFC (PN532)、UART エキスパンダ (SC16IS752)、
1-Wire 部品 (DS2431, DS2408)、クロック IP (RCC-F1, RP2040 clocks)、
PC プラットフォームグルー (P2SB, PMC, CSME, iTE EC)。QEMU の
`hw/misc` と同じ罠 (設計書 M9) が 210 件規模で再発した。

### P3: 同一チップの重複実装

* `part:ti/tca9548a` (`i2c/`) と `part:ti/tca9548a-mux` (`misc/`) —
  同一チップの二重カタログ登録 (misc 版の summary が自ら "(misc
  variant)" と名乗る)。
* `bme680` と `bme680-i2c`、`bme688` と `bme688-i2c` — 同一チップの
  I2C/SPI 両対応を別モデルに分割。BME280 は 1 モデル + `bindings =
  ['i2c', 'spi']` という正しい形になっている (設計書 §10 ルール 4)。
* `efm32-usart` と `efm32-usart-spi` — 同一ペリフェラルの同期モードを
  別モデルに分割。

## 決定

### 1. `hw` 内の第一軸は **責務 (responsibility)**、第二軸は機能/ドメイン

```
crates/hw/src/
├── ip/        # CPU がアドレス空間経由で直接叩くモデル (MMIO/PortIO/PCI)
│   └── <function>/    # 機能: char, i2c, spi, gpio, intc, timer, ...
├── part/      # ファブリック経由でしか到達できないチップモデル
│   └── <domain>/      # 部品としての正体: sensor, memory, mux, ...
└── core/      # 複数ドメインから使われる共有機能コアとモデル作者の道具
```

* **`ip` (device)**: ボードの `at` でアドレス空間に置かれ、CPU が直接
  読み書きするレジスタブロック。SoC 内蔵ペリフェラル、PCI/ISA カードの
  コントローラ、ポート I/O デバイス。カタログ ID は `ip:<origin>/<model>`。
* **`part` (leaf part)**: それ自体はアドレス空間に現れず、他のデバイスの
  背後のファブリック (I2C/SPI/USB/PS2/SD/1-Wire/GPIB/ビデオシンク…)
  でだけ到達できるチップ。ボードは `conn` 配線で取り付ける。カタログ ID
  は `part:<origin>/<model>`。
* **`core`**: 2 以上のドメインから使われる共有実装のみ (Rule of two,
  設計書 P12)。単一ドメイン専用のファミリコアは当該ドメインの中に
  `*_core.rs` / `i2c_pointer.rs` 形で置く (ADR 0026 step 2 の継承)。

この区別は既に存在する語彙そのものである: manifest の `[identity]` /
`[part]` スキーマ、ID の `ip:` / `part:` 接頭辞、カタログの
「レジスタモデルは spawn 登録、バス媒介部品はボードがファブリック経由で
接続」という説明 (ADR 0025)。本 ADR は **ディレクトリ木を ID 語彙と
一致させる** だけであり、新しい概念を足さない。

### 2. ファブリックは分類軸から除外する

* `i2c/`, `ssi/`, `can/`, `usb/` のようなファブリック名ディレクトリを
  **コントローラ IP の機能名** として `ip/` 側にだけ残す (`ip/i2c`,
  `ip/spi`, …)。コントローラの機能名がファブリック名と一致するのは
  「I2C ホストコントローラという機能」だからである (QEMU が
  `hw/i2c/imx_i2c.c` を置くのと同じ意味)。
* `part/` 側にファブリック名のドメインを作らない。部品の居場所は
  **部品としての正体** (何を測るか、何を保持するか、何を駆動するか) で
  決まり、どのバスで話すか (`bindings`) は属性であって居場所ではない。
  多バス対応部品 (BME280 型) は 1 モデル 1 ディレクトリ + `bindings`
  複数 + バインド別モジュール (`{mod,i2c,spi}.rs`) で表す。

### 3. `part` ドメイン語彙 (部品の正体)

| ドメイン | 定義 | 旧配置 |
| --- | --- | --- |
| `sensor` | 物理・環境量の測定チップ (温度/湿度/IMU/光/磁気/カメラ/電流電圧監視含む) | `sensor/` + `misc/` のモニタ群 |
| `memory` | メモリチップ: EEPROM/FRAM/SPI NOR/Raw NAND/UFS/HyperFlash/HyperRAM/SRAM | `eeprom/` + `flash/` (統合) |
| `storage` | ストレージ媒体・ドライブ (SD カード, Microdrive, SBP-2 ディスク) | `sd/sd-card` + `misc/` |
| `expander` | GPIO/IO エキスパンダ (I2C/SPI/1-Wire) | `gpio/` の部品 + `misc/` の DS2408/2413 |
| `mux` | バススイッチ・マルチプレクサ・ブリッジ (TCA9548A, DS28E17) | `i2c/` + `misc/` |
| `rtc` | 外付け RTC チップ (DS1307, BQ32000, ISL1208/1219) | `rtc/` の部品 + `misc/` |
| `adc` / `dac` | 外付けコンバータチップ | `adc/` + `misc/` の DS2450 |
| `audio` | オーディオ系チップ・機器 (コーデック, アンプ, FireWire オーディオ I/F) | `audio/` の部品 + `misc/` |
| `display` | パネル・モニタ・バックライト以外の表示系チップ | `display/` の部品 |
| `input` | 入力デバイス (キーボード, マウス, タッチ, ビデオキャプチャ) | `input/` の部品 + USB HID |
| `net` | PHY・無線モジュール・モデムカード | `net/` の部品 |
| `power` | 電力の供給・計量・管理 (PMIC, 充電器, ゲージ, レギュレータ, ホットスワップ, 電力計測) | `misc/` |
| `motion` | モータドライバ・サーボ (TMC5160, DRV8301, RC サーボ) | `misc/` |
| `fan` | ファン (PWM 制御) | `misc/` |
| `led` | LED ドライバ・バックライト (LP55xx, ADP8860) | `misc/` |
| `security` | TPM・Secure Element (ATECC, ST33) | `misc/` |
| `nfc` | NFC コントローラ・タグ (PN532, ST25DV) | `misc/` |
| `instrument` | 計測器 (GPIB) | `misc/` |
| `serial` | UART エキスパンダ (SC16IS7xx, MAX3100, XR20M1172) | `misc/` |
| `can` | 外付け CAN コントローラ (MCP2515) | `can/` |
| `i3c` | I3C バス上のターゲットデバイス | `i3c/` |

境界の判断に迷う部品は「主たる機能」で置き、manifest の note に判断を
記録する。**どのドメインにも属せない部品が出たら新ドメインを ADR で
追加する。`misc` のような受け皿は `ip`/`part` 両側で禁止する** (設計書
N2 の `misc` 例外は本 ADR で撤回する)。

### 4. `ip` 機能語彙 (コントローラ側)

QEMU `hw/` の語彙を維持しつつ、実態を明確化する:
`char, i2c, spi (旧 ssi), gpio, can, intc, timer, rtc, dma, sd, usb, net,
pci, ide, nvram, mailbox, wdt, virtio, display, audio, input, clock,
platform`。

* `ip/clock` — クロック生成・分配・ゲート IP (RCC-F1/F4/H7, RP2040
  clocks, LPC11xx CGU, CM-AM335x)。旧 `misc/` にいた RCC 系の居場所。
* `ip/platform` — CPU からアドレス空間経由で見えるプラットフォーム
  グルー (P2SB, PMC, PMT, CSME, iSH, PC スピーカ, pc-testdev,
  PrimeCell ID ブロック, EC/SIO/BMC の LPC/eSPI レジスタモデル)。
* `ip/spi` — `ssi/` から改名。契約 (`contract/src/spi.rs`) とバス基盤
  (`bus/src/spi/`) の語彙と揃える。

### 5. 重複の禁止 (同一チップ = 1 カタログ ID)

* 同一チップの複数登録を禁止する。`tca9548a-mux` は `tca9548a` に
  統合 (misc 版を削除)。
* 同一チップの複数バス対応は別モデルにしない。`bme680-i2c`,
  `bme688-i2c` を `bme680`, `bme688` に統合し、`bindings = ['i2c',
  'spi']` とする。`efm32-usart-spi` を `efm32-usart` に統合する
  (同期モードは同一レジスタブロックの能力)。
* インターフェース違いが **別製品** (MCP23017 I2C 版と MCP23S17 SPI 版
  のような実在の別型番) である場合は別 ID を維持してよい。

### 6. `core` の語彙

* `core/primecell_id.rs` (旧 `misc/primecell_id.rs`) — PrimeCell 周辺
  ID 規約。PL011/PL061/PL190/PL022/SP804 が使用。
* `core/unmapped_policy.rs` — 未対応オフセットのポリシー。旧
  `char/pl011.rs` 内の `UnmappedPolicy` (PL011 以外の 6 モデルが
  参照していたので共有物に昇格)。
* `core/i2c_pointer.rs` (旧 `sensor/i2c_pointer.rs`) — ポインタレジスタ
  形式のコア。sensor/adc/power の 3 ドメインから使用。
* `core/pwm_device.rs` (旧 `misc/pwm_device_core.rs`) — PWM 駆動部品の
  コア。fan と motion の 2 ドメインから使用。
* 単一ドメインのファミリコア (`ds1307_core`, `vga_monitor_core`,
  `ps2_keyboard_core`, `nand_flash_core`, `gpib_instrument_core`) は
  各ドメイン内に留める。**`core` に置いてよいのは 2 ドメイン以上から
  使われるものだけ** とし、これを ADR 0026 step 2 の昇格規則とする。

### 7. manifest ミラーと lint

* `manifests/hw/ip/<function>/<model>.toml` ↔ `crates/hw/src/ip/
  <function>/<model>.rs`、`manifests/hw/part/<domain>/<model>.toml` ↔
  `crates/hw/src/part/<domain>/<model>.rs`。
* xtask は次を機械検査する: (a) `ip:` ID の manifest は `manifests/hw/
  ip/` に、`part:` ID は `manifests/hw/part/` にあること (種別と経路の
  不一致はエラー)、(b) 機能/ドメイン名が語彙表内であること、
  (c) stem と ID 名の一致 (kebab↔snake)、(d) コード側ミラーの存在。
* `[part]` スキーマ (bindings + レジスタ表) は簡易リーフ部品用、
  `[identity]` スキーマ (ports + params) は spawn 登録モデル用という
  使い分けは不変。ただし `part:` ID を `[identity]` 形で書くことを
  認める (USB HID や SD カードのような「ポートを持つ部品」のため)。

## 責務と依存の全体系 (再確認)

```
util(L0) ← exec(L1) ← contract(L2) ← bus(L3) ← runtime(L4)
                                    ← isa(L2)
                                    ← accel(L4) ← unicorn のみ (ADR 0030)
           contract/exec/runtime/assemble(L6) を下に見る hw(L7)
                                                ↑ machine(L8) だけが依存
           boot(L2) は decode 専用、describe(L5) は依存なし
           front(mosaic, L9) は machine + boot のみ
```

`hw` の中の `ip` / `part` / `core` は層ではなく **同一層内の居場所分け**
である。依存方向は従来どおり下位層のみ (`contract`, `exec`, `util`,
`bus`, `runtime`, `assemble`)、`machine` への依存は禁止。

## 移行

* git mv でコード・manifest を一括移動 (約 1400 ファイル)。
* `mod.rs` / `catalog.rs` は新木構造から再生成 (カタログ登録内容は
  維持)。`hw/src/catalog.rs` の集約表を `ip` / `part` 両対応に更新。
* 移動に伴うパス修正: `machine` (pc: `pic_8259` / `pit_8254` /
  `mc146818`、stm32f103: `gpio_f1`)、hw 内 `use crate::…` の全面書き
  換え。
* 種別の誤りを訂正: `ps2-mouse`, `ps2-touchpad`, `hid-keyboard`,
  `hid-mouse`, `sd-card`, `i3c-target` を `part:` へ、
  `usr-sportster-14400-int` を `ip:` へ (ISA ポート空間経由のため)。

## 負の影響と緩和策

* **パス変更の波及**: 外部から `mosaic_hw::<function>::…` を参照する
  コードは `mosaic_hw::ip::<function>::…` へ更新が必要 (影響は
  machine/mosaic の 5 箇所のみ)。
* **語彙の学習コスト**: 22 の part ドメインを覚える必要がある。ただし
  各ドメインは自己記述的 (sensor, memory, fan, led…) であり、
  受け合わせの暗黙ルール (`misc` へ投げる) より判断が機械的になる。
* **misc 廃止**: 将来「どこにも置けない」部品が出た場合はドメイン追加
  の ADR が必要になる。これは意図的な摩擦である (分類の先送りを
  構造的に防ぐ)。

## 検証

* `cargo build --locked --workspace --all-features` — 緑。
* `cargo test --locked --workspace --all-features` — 全パッケージ緑
  (カタログ ID 一意性・名前空間検査を含む)。
* `cargo run -p mosaic-xtask -- check` — 種別/経路一致・ミラー・語彙
  検査込みで OK。
