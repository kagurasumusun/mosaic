# ADR 0032: 多面モデルのポート面 — ip / part を貫く統一接続面

* 状態: 採用
* 日付: 2026-10-28
* 関連: ADR 0025 (bus/boot 基盤、カタログ), ADR 0031 (hw 責務別分類),
  設計書 §10 (モデル), §13 (配線)

## 背景

ADR 0031 は「部品の居場所は正体で決まり、バスは属性に過ぎない」ことを
ディレクトリ木に確立した。しかしオーナー指示 (2026-10-28)
「現在の構造だと複数のインターフェースを使うモデルのことを考えられて
いないので構造を考え直して」が指すとおり、**居場所** の整理だけでは
不十分だった。複数インターフェースを持つモデルは、**接続面** の構造に
4 つの非対称を残したままだった:

1. **宣言の非対称**。`ip` のマニフェストは `[ports]` を持つ (mmio・irq・
   clock・`i2c = { contract = 'i2c/initiator' }` 等) が、`[part]` は
   `bindings`（トランスポート名のリスト）とレジスタ表しかない。そのため
   MCP23017 (I2C ターゲット + GPIO 16 本 + INT 出力) の GPIO 面と割り込み面、
   TCA9548A の下流 8 セグメント (それぞれが独立した I2C バス) は、
   マニフェストに宣言する語彙自体が存在しない。
2. **配線文法の非対称**。SoC 側 `[[block]]` は `at`（アドレス配置）+
   `irq`（複数出力）+ `clk`（複数入力）と多画面に配線できる一方、
   ボード側 `[[part]]` は `bind`（文字列 1 本）+ `conn.bus`（エンドポイント
   1 個）しかない。1 つの部品インスタンスを 2 つ以上のファブリックに
   同時につなぐ書き方が存在しない。
3. **カタログの非対称**。`part/*/catalog.rs` は `MODELS: &[ModelEntry] = &[]`
   — 部品は spawn 登録されず、モデル生成知識は各ボードのグルーコードが
   手書きする前提だった。「モデルの生成知識（IRQ・クロック・パラメータ）は
   カタログ側が持つ」(ADR 0025) という原則が ip 側にしか適用されていない。
4. **結合の非対称**。`assemble::materialize` が扱うエッジは `irq` /
   `clock` / `address-space` のみで、i2c・spi・gpio などファブリック系の
   エッジはすべて `DeferredEdge`（意図的な未結合の記録）に落ちる。
   宣言しただけでは何も繋がらない。

## 決定

### 1. 接続面の語彙は「ポート面 (port face)」に統一する

モデル（ip ・ part を問わず）の外界との接続点は、すべて **ポート面** として
表現する。ポート面は (名前, 種別, 役割) を持つ:

| 種別 (`kind`) | 役割 | 意味 | 例 |
| --- | --- | --- | --- |
| `i2c` | target | どこかの I2C バス上のターゲット | MCP23017 の上流面、BME280 |
| `i2c` | initiator | 自分がマスタする I2C バス（セグメント） | i2c-f1 コントローラ、TCA9548A の `down0..7` |
| `spi` | target / initiator | SPI の同型 | BME280 の SPI 面 |
| `gpio` | provider | 外へ出す GPIO ライン（番号付き） | MCP23017 の GPA0..GPA7/GPB0..GPB7 |
| `irq` | source | 割り込み出力（ピン名付き） | MCP23017 の `int` |
| `clock` | endpoint | クロック入力 | センサの心拍入力等 |
| (`mmio`) | target | アドレス空間配置 — ip 専用、`at` のまま | 従来どおり |

`bindings`（トランスポートの列ポートの列挙）は**廃止しない**が属性に格下げし、
実態は `[ports]` の target 面から機械検査する。居場所の規則 (ADR 0031:
`ip/<function>`・`part/<domain>`・`modelkit`) は不変 — 本 ADR は
**居場所**ではなく**接続面**の統一であり、ディレクトリは動かさない。

### 2. `[part]` マニフェストに `[ports]` を導入する（ip と同型）

```toml
[part]
id = 'part:microchip/mcp23017'
...

[ports]
i2c  = { contract = 'i2c/target', role = 'target' }
gpio = { contract = 'gpio/port', role = 'provider', count = 16 }
irq  = { role = 'source', pins = ['int'] }
```

* 多バス部品 (BME280) は target 面を 2 つ（i2c・spi）宣言する。**1 モデル
  1 ディレクトリ・複数面**（ADR 0031 §2 の方針を接続面に拡張）。
* mux 型部品 (TCA9548A) は target 面（上流）と initiator 面 `down`（count 8、
  実名 `down0..down7`）を宣言する。

### 3. ボード配線は `[[part.face]]`（面ごとの配線）

```toml
[[part]]
name = 'ioexp'
use = 'part:microchip/mcp23017'

[[part.face]]
port = 'i2c'
to = 'soc.i2c1:i2c'        # 相手はイニシエータ面（コントローラ or mux セグメント）
address = 0x20

[[part.face]]
port = 'gpio'
index = 0
to = 'wire:ioexp-gpa0'     # GPIO ネット（電気的ネット）
pull = 'up'

[[part.face]]
port = 'irq'
pin = 'int'
to = 'wire:ioexp-int'      # シグナルシンク（wire: またはコントローラ入力）
```

* 1 つの `[[part]]` が面をいくつでも持てる。**面の数 = 結合の単位**。
* `to` は常に「相手側エンドポイント」。エッジ方向は kind から導出する:
  * `i2c` / `spi`: イニシエータ（`to` の参照先）→ 部品（target）
  * `gpio`: 部品（provider ライン index）→ ネット（`wire:`）
  * `irq`: 部品（`pin`）→ シンク
  * `clock`: ソース → 部品
* 面の一意性: (port, index) と (port, pin) は部品内で重複禁止。
* `conn.bus` / `bind` の旧文法は廃止する（使用例が無いため移行は不要）。

### 4. グラフ (describe) は面をエッジに正規化する

* i2c/spi 面: `from = イニシエータ (node, port)` / `to = 部品 (node, port)` /
  `kind = 'i2c'|'spi'`、パラメータ（address 等）はそのまま乗る。
* gpio 面: `from = 部品` / `to = wire:外部ノード` / `kind = 'gpio'`、
  パラメータに `index`・`pull`。
* irq 面: 既存の `irq` エッジと同じ形（既存のシグナル割り当て・配線機構が
  そのまま部品に効く）。
* clock 面: 既存の `clock` エッジと同じ形。
* `ip` 側 (`[[block]]`) への面文法拡張は本 ADR では**開かずに置く**
  （`[[block]]` の irq/clk は既に多画面であり、必要になった時点で
  同じ文法を block に広げる）。ip のイニシエータ面は部品面の `to` から
  参照されるだけで良い。

### 5. カタログ (hw) は部品も spawn 登録する

* `ModelEntry` に `faces: &'static [FaceDesc]`（名前・種別・役割・個数）を
  追加し、`--catalog` と lint が接続面を検査できるようにする。
* 部品ドメインの `catalog.rs` に spawn を登録する。spawn は:
  * 自分の target 面（i2c/spi）を `ctx.expose_*` で露出する
  * 自分が持つイニシエータ面（mux の下流など）を `ctx.offer_i2c_segment`
    で登録する
  * GPIO ライン面を `ctx.expose_gpio_line` で露出する
  * IRQ 出力は従来どおり `ctx.output(pin)` でシグナルを受け取る
* **部品は Machine のデバイスアリーナに登録されない**（MMIO もタイマも
  持たないため）。部品の生存はファブリックが所有する面ハンドルが担う。
  タイマを持つ部品が必要になった時点で State 対応と合わせて ADR を起こす。

### 6. assemble にファブリックストアと面結合パスを足す

```
materialize:
  allocate nodes → assign irq outputs → incoming wires → views
  → instantiate（各 spawn が面の own/offer/expose を行う）
  → connect_wires（irq/clock — 従来）
  → connect_faces（新規: i2c/spi/gpio エッジを結合）  ← 今回追加
  → attach cpu views → verify
```

* `FabricStore` が `(node, port) → Arc<Mutex<I2cBus>>`（イニシエータ面）と
  `wire 名 → GpioNet`（電気的ネット）と `(node, port, index) → GPIO ライン面`
  （露出された provider 面）を持つ。
* コントローラの spawn は `ctx.i2c_segment(port)` で（必要なら生成して）
  自分のマスタ面のバスを受け取る。グラフがこの面に部品を配線しているかは
  `ctx.face_wired(port)` で判定でき、ホストアタッチメント優先の従来経路
  （dw-apb-i2c + pc グルー）と両立する。
* 面結合パスは i2c エッジごとに: イニシエータ面のバスを解決し、
  部品が露出した target 面をパラメータ付きでバスに attach する。
  GPIO エッジごとに: ネットを解決（または生成）し、`attach(pull)` で
  端子を作り、部品のライン面に `join(net, node)` を渡す。
* 結合できなかった面エッジは `DeferredEdge` ではなくエラー。未結合の
  成功功扱いは廃止する（設計書の「Presence here is not success」を
  i2c/spi/gpio にも適用）。

### 7. 責務と依存の変化

* `describe` はファブリック型の意味を知らない（kind 文字列とパラメータの
  まま）。依存は増えない。
* `assemble` は `bus` の i2c/gpio ファブリックを直接扱う（従来から
  HostAttachments で依存済み）。新しい依存は無い。
* `hw` の部品は `contract`（I2cTarget・GpioSink 等）と `bus`（GpioNet）
  のみを話し、`machine` を知らない。ADR 0031 の依存規則は不変。
* ボード（`machine`）は部品の生成知識を持たない。**配線だけ**を持つ。

## 実証（本 ADR と同じコミットで入れるもの）

* `ip:st/i2c-f1` を RM0008 §27 相当の実装（CR1/CR2/OAR1/OAR2/DR/SR1/SR2/
  CCR/TRISE、SB/ADDR/BTF/TxE/RxNE のマスタ送受信シーケンス、EV/ER の
  2 本の IRQ 出力、APB1 クロックゲート、`i2c` イニシエータ面）に置き換える。
  旧実装は 16 バイトのスクラッチ RAM（プレースホルダ）だった。
* `ip:st/i2c-f1` を STM32F103 SoC マップに追加（RM0008: 0x40005400、
  EV=31/ER=32、`rcc.gate-apb1-i2c1`）。
* 多面部品 4 種を spawn 化: MCP23017（i2c + gpio×16 + irq、INT を
  GPINTEN/DEFVAL/INTCON に従って発生）、TCA9548A（i2c + 下流 8）、
  BME280（i2c / spi の 2 target 面）、LM75A（i2c）。
* 機械 crate にボードマニフェストのパーサと、SoC + ボード（部品 5 個・
  mux 経由の多段 I2C・GPIO ネット・INT シグナル）を組み立てる統合テスト。
  このテストはレジスタシーケンスの合成検証であり、ゲスト OS の起動検証
  ではない（ADR 0022 の実イメージ台帳とは別物）。

## 負の影響と緩和策

* **`[[part]]` の旧文法（bind/conn）の削減**: 使用例が無いことを確認済み。
  describe のテストは新文法に更新する。
* **部品のスナップショット未対応**: 部品はアリーナ外なので checkpoint に
  入らない。State 対応は別 ADR（catalog 全モデルの State 匃と一緒に）。
* **gpio-f1（ip 側 GPIO）と GpioNet の未統一**: ip 側 GPIO コントローラは
  シグナル直結で、ネット型ファブリックとまだ繋がらない。INT 線を実機同様
  EXTI 経由にするにはこの統一が必要。本 ADR の範囲外とし、統合テストでは
  INT をプローブシンクで観測する。
* **マスタ受信のバースト近似**: RM0008 の ACK 制御による 1 バイト単位の
  受信完了は、fabric がトランザクション単位 (`read`/`write_read`) のため、
  「初回 DR 読み出しでバースト読み出して以降 DR から供給」という機能近似に
  なる。ポインタ再設定を毎回行う通常のドライバ動作には正確であり、
  モデルの doc comment に境界を明記する。

## 検証

* `cargo build --locked --workspace --all-features` — 緑。
* `cargo test --locked --workspace --all-features` — 全パッケージ緑
  （describe の面スキーマ、assemble の面結合、i2c-f1 の RM0008 シーケンス、
  機械の多面統合テストを含む）。
* `cargo run -p mosaic-xtask -- check` — 部品 `[ports]` の lint 込みで OK。
