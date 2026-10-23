# Mosaic

Rust 主体の仮想マシン基盤。リポジトリ名を暫定のプロジェクト名・クレート接頭辞として使用しています。

[構成設計書](docs/大規模仮想マシン%20構成設計書.md) に沿って、**P0 の骨格・検査基盤と P1 のイベント・アドレス・CPU 実行基盤**を構築しています。
**SoC・ボードを組み立てたファームウェア起動はまだできません。**

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

通常の CLI は開発用ヘルプを表示します。Unicorn を使用する場合だけ明示的に opt-in します。

```sh
cargo run --locked -p mosaic-apps-cli --features unicorn -- unicorn-smoke
```

これは FlatView のコード・データ領域で A32 の 4 命令を実行し、レジスタとメモリを検査する smoke test です。
必要な native ビルド依存、GPL-2.0 と制限は [Unicorn ガイド](docs/guides/unicorn.md) を参照してください。

## 構造

- `framework/`: 共通語彙、契約、ランタイム、実行エンジン、記述・組立の境界。
- `catalog/`: 種別 → 出自で配置するモデルと宣言。現在、モデルは未登録です。
- `apps/`: 実行アプリケーション。
- `xtask/`: Cargo の依存宣言に基づく層・命名の検査。
- `docs/adr/`: 初日の設計判断と未決事項。

### 実装済み

- 16 クレートの Cargo workspace、層メタデータ、unsafe 禁止。
- `lint-layers`: framework の逆依存、L3 間依存、catalog 横依存、glue の依存を検査。
- `lint-names`: 禁止語、kebab-case、クレート名、正規の出自を検査。
- `check` / `doctor`、fast CI（fmt、構成 lint、Clippy、単体テスト）。
- `Addr` / `VTime` / `Attr` / `BusFault` / 型付き Snapshot の境界。
- 最小限の model context、MMIO、signal、clock、CPU-engine contract（暫定 API）。
- 決定論的イベントキュー、ノード専用 Ctx、遅延 signal、型付きキュースナップショット。
- Region 木 → FlatView、優先度 overlay、境界・permission・device offset 解決。
- ARM のみの Unicorn アダプタ（2.1.5）、ISA の register ID と初期 engine 契約、backend 非依存の A32 検証。
- デバイスアリーナ（take-out と再入拒否）、RAM/ROM バッキング、FlatView 経由の MMIO ディスパッチ、
  タイマ・signal の配線と配送を持つ `Machine`。
- 同時刻順序・取り消し・境界・不正な復元・再実行一致のテストと、期待出力を比較する実行例。

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

### 次の実装

最初の MCU・ボードを決め、catalog 上の UART とタイマ IP、SoC / ボード宣言を
イベント・アドレス・engine 基盤へ縦につなぎます。そのために CPU エンジンの MMIO hook と
AddressView / DMI 結合、CPU とイベントの協調スケジューリングが必要です。
現在の Unicorn memory は instance ごとの private page であり、全機械の共有メモリではありません。
実機モデルを増やす前に、決定論と MMIO の machine テストを引き続き整備します。

詳細は [開発ガイド](docs/guides/development.md) と [ADR 一覧](docs/adr/README.md) を参照してください。

## ライセンス・貢献規約

**オーナー判断待ちです。** ライセンスをエージェントが選択することはせず、全クレートを
`publish = false` にしています。Unicorn は明示的な導入指示に基づき開発・CI 検証へ導入しましたが、
プロジェクトライセンスや配布条件の確認が完了したという意味ではありません。
公開・配布と外部貢献の受け入れ前に [ADR 0006](docs/adr/0006-license.md) を解決してください。
