# Mosaic

Rust 主体の仮想マシン基盤。リポジトリ名を暫定のプロジェクト名・クレート接頭辞として使用しています。

[構成設計書](docs/architecture.md) に沿って、P0–P3 の縦スライスを構築し、**P4 の in-memory runtime checkpoint**を開始しました。
P4 は全 catalog モデル・CPU engine・signal sink・永続形式までは未完です。
`bringup`・`sensorbus`・`multicore` の各アプリケーションが Unicorn 上で実ファームウェアを起動し、期待 UART 出力を検証します。

## システムエミュレータとして使う

`mosaic-system` が実行の入り口である（ADR 0021）。QEMU と同じ語彙で
マシンを選び、実イメージを付けて起動する。

```sh
# 実 CD イメージからブートする（ISOLINUX が画面にバナーを出すまで実行）:
cargo run --locked -p mosaic-apps-system --features unicorn -- \
    -M pc -cdrom tests/assets/tinycore-16-core.iso -m 64M -display text

# カーネルだけを ISO から取り出して直接起動する（-kernel/-initrd/-append）:
cargo run --locked -p mosaic-apps-system --features unicorn -- \
    -kernel tests/assets/tinycore-16-core.iso\!boot/vmlinuz \
    -initrd tests/assets/tinycore-16-core.iso\!boot/core.gz \
    -append "console=ttyS0,115200 console=tty0" -serial stdio

# フロッピイメージからブートする（FreeDOS の KERNEL.SYS まで実行）:
cargo run --locked -p mosaic-apps-system --features unicorn -- \
    -fda test-images/freedos-1.3-boot.img -boot a -display text
```

`-M help`、`-cpu help` で選択肢一覧。`--report -` は JSON の実行報告書を
出す（停止理由・命令数・画面・シリアル・未処理割込み・例外）。

## ゲスト OS のテスト

実イメージだけを扱う（ADR 0022）。台帳は `tests/images.toml`、
ハーネスは OS を知らない 1 本のループ:

```sh
cargo xtask images fetch        # 実イメージ（Alpine/SliTaz/Debian/ReactOS/Android-x86…）を取得
cargo test --locked -p mosaic-tests-boot --features unicorn -- --nocapture
```

目標の網羅表（Linux 全アーキ / Android 1.6-4.4 / Windows 系 / RTOS）は
[docs/os-targets.md](docs/os-targets.md) を参照。

## 開発を始める

Rust 1.85.0（`rust-toolchain.toml`）、rustfmt、Clippy が必要です。
初回は crates.io から開発ツール用の依存を取得します。

```sh
cargo fmt --all -- --check
cargo xtask check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo xtask doctor
```

開発用 CLI（`apps/cli`）は Unicorn の FlatView 結合 smoke test を持つ:

```sh
cargo run --locked -p mosaic-apps-cli --features unicorn -- unicorn-smoke
```

これは FlatView のコード・データ領域で A32 の 4 命令を実行し、レジスタとメモリを検査する smoke test です。
必要な native ビルド依存、GPL-2.0 と制限は [Unicorn ガイド](docs/guides/unicorn.md) を参照してください。

## 構造

- `framework/`: 共通語彙、契約、ランタイム、実行エンジン、記述・組立の境界。
- `catalog/`: 種別 → 出自で配置するモデルと宣言。ARM IP（pl011 / sp804 / pl022 / pl061 / mhu /
  dw-apb-i2c）、ARM926EJ-S・SiFive E31 の CPU 宣言、Bosch / TI の部品、
  SoC・ボード宣言を登録済みです。
- `apps/`: 実行アプリケーション。`apps/system`（`mosaic-system`）が VM の
  実行入り口。`bringup`・`sensorbus`・`multicore` は縦スライスの実証アプリ。
- `tests/`: 機械レベルのブートテスト（実イメージの台帳と汎用ハーネス）。
- `xtask/`: Cargo の依存宣言に基づく層・命名の検査。
- `docs/adr/`: 初日の設計判断と未決事項。

### 実装済み

- 44 クレートの Cargo workspace、層メタデータ、unsafe 禁止。
- `lint-layers`: framework の逆依存、L3 間依存、catalog 横依存、glue の依存を検査。
- `lint-names`: 禁止語、kebab-case、クレート名、正規の出自を検査。
- `lint-catalog`: CPU/IP/part/SoC/board manifest のスキーマ、catalog ID／参照、I²C アドレス／SPI CS、IRQ 複数ルート、正規化 MachineGraph の port/pin/role/index を検査。TOML パーサは tooling 層に隔離。
- `check` / `doctor`、fast CI（fmt、構成 lint、Clippy、単体テスト）。
- `Addr` / `VTime` / `VDelta` / `Attr` / `BusFault` / 型付き Snapshot の境界。
- 最小限の model context、MMIO、signal、clock、CPU-engine contract（暫定 API）。
- 決定論的イベントキュー、ノード専用 Ctx、遅延 signal、型付きキュースナップショット。
- Region 木 → FlatView、優先度 overlay、境界・permission・device offset 解決。
- ARM のみの Unicorn アダプタ（2.1.5）、ISA の register ID と初期 engine 契約、backend 非依存の A32 検証。
- `EngineBus` callback と `mmio_map` による engine 側 MMIO region、バス故障の記録と Fault 停止。
- デバイスアリーナ（take-out と再入拒否）、RAM/ROM バッキング、FlatView 経由の MMIO ディスパッチ、
  タイマ・signal の配線と配送を持つ `Machine`。
- P4 初期 runtime snapshot: queue + RAM/ROM + snapshot 対応 device State を束ねた in-memory checkpoint、
  topology fingerprint / 完全一致検証、事前検証、再ロード・replay の golden test。catalog の初期対応は `mhu`。
  CPU engine、signal sink / IRQ mailbox、shared mirror、host I/O、永続 codec は未対応で、未対応ノードを含む
  Machine は部分 snapshot を拒否します（[Snapshot ガイド](docs/guides/snapshot.md)）。
- L5 `mosaic-assemble`（`unicorn` feature）: FlatView から engine ページと MMIO region を導出する
  `bind_flat_view`、命令量子とイベント配送を交互に行う `MachineDriver`。
- 同時刻順序・取り消し・境界・不正な復元・再実行一致のテストと、期待出力を比較する実行例。
- P1 垂直スライス: catalog IP `pl011` / `sp804`、`arm926ej-s` SoC と `versatile-pb` ボードの宣言、
  Unicorn 上で A32 ファームウェアを起動して UART 出力を検証する `mosaic-apps-bringup`。
- P2 バスとピン: interconnect `i2c-bus`（GpioNet 上のビット単位デコーダと bit-bang 経路）、
  IP `dw-apb-i2c` / `pl022` / `pl061`、部品 `bme280`（I2C・SPI 二束ね）と `tca9548a` マルチプレクサ、
  `SpiBus` と CS 配線。
- P2 終了条件の証明: `mosaic-apps-sensorbus` が 461 語の手書き ARM ファームウェアで同一の BME280 を
  「I2C コントローラ経由」「GPIO ピン操作の bit-bang 経由」「SPI 経由」の 3 経路から読み、
  bit-bang が書き込んだ `ctrl_meas=0x25` をコントローラ経路が同じ部品から読み返すことを
  CI の golden test（`ci-reports/sensorbus.log`）で検証します。
- P3 マルチコア・ヘテロジニアス: `framework/assemble` の `MulticoreDriver` が固定順ラウンドロビンの
  量子スケジューラを提供し、`Machine` はマスターごとの FlatView（コア別ビュー）を解決します。
  共有 RAM は正本バッファと各エンジンのミラーを持ち、量子境界で同期します
  （Unicorn 2.1.x の `uc_mem_map_ptr` は**ゲストから読み取り可能だが書き込みは
  fault する**ことを実験で確認したため、ADR 0010 に記録）。
  ISA 語彙 crate は ISA ごとに追加でき（`framework/isa/arm`, `framework/isa/riscv`）、
  エンジンは `arch-*` feature で ISA 集合を絞ります（設計書 §12.1/§12.3/§12.4）。
- P3 終了条件の証明: `mosaic-apps-multicore` が **ARM926EJ-S ×2 + RV32I ×1** の異種 AMP 構成を
  1 つの共有ページと MHU ドアベル（`catalog/ip/arm/mhu`）で起動し、SMP コアと
  異種コプロセッサがドアベルハンドシェイクを完了し、コンソールに `smp=61` / `amp=bf` を
  出力します。観測系列（コンソール・共有 RAM・MHU 状態・コア別 IRQ エッジ）は
  量子 64×48 ラウンドと 512×6 ラウンドで同一です（CI golden test、`ci-reports/multicore.log`）。

```sh
cargo run --locked --package mosaic-apps-bringup --features unicorn
cargo run --locked --package mosaic-apps-sensorbus --features unicorn
cargo run --locked --package mosaic-apps-multicore --features riscv
```

CPU やファームウェアを使わないイベント実行例:

```sh
cargo run --locked --package mosaic-core-runtime --example events
```

同時刻イベントと遅延 signal のトレースを表示し、保存状態からの再実行が一致することを確認します。
これは VM 全体の起動・スナップショット機能ではありません。

MMIO・タイマ・signal を縦に通すマシン実行例:

```sh
cargo run --locked --package mosaic-core-runtime --example machine
```

Rom/Ram 権限、MMIO 経由のタイマ予約、signal 配送、BusFault と決定論をトレースします。
CPU エンジンとの連携や実ファームウェアの起動はまだ含みません。

初期 P4 の runtime checkpoint / replay 例:

```sh
cargo run --locked --package mosaic-core-runtime --example snapshot
```

RAM、typed device State、pending timer の復元を確認します。CPU engine context 等を含む VM 全体の
snapshot ではありません。詳細は [Snapshot ガイド](docs/guides/snapshot.md)を参照してください。

### 次の実装

P2（バスとピン）と P3（マルチコア・ヘテロジニアス）の終了条件は CI 上で証明済みです。
P4 は runtime checkpoint の初期 slice を実装しました。次は catalog の全モデル State 化、CPU engine
context / IRQ sink / shared mirror の checkpoint、永続化と State migration、状態ハッシュ一致を進めます
（設計書 §13.5, §22 T4）。その後に記録再生、gdbstub、トレース、故障注入（§13.6 の tap 点）が目標です。
残る技術項は命令数と VTime を結ぶ clock tree（現在はコアごとの固定周期）、
Unicorn 2.1.x に無い外部割り込み注入（`EngineCaps::line_injection = false`）、
キャッシュ/TLB 無効化の粒度、書き込み可能な共有 DMI です。

詳細は [開発ガイド](docs/guides/development.md) と [ADR 一覧](docs/adr/README.md) を参照してください。

## ライセンス・貢献規約

**オーナー判断待ちです。** ライセンスをエージェントが選択することはせず、全クレートを
`publish = false` にしています。Unicorn は明示的な導入指示に基づき開発・CI 検証へ導入しましたが、
プロジェクトライセンスや配布条件の確認が完了したという意味ではありません。
公開・配布と外部貢献の受け入れ前に [ADR 0006](docs/adr/0006-license.md) を解決してください。
