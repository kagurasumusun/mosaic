# Mosaic

Rust 主体の仮想マシン基盤。リポジトリ名を暫定のプロジェクト名・クレート接頭辞として使用しています。

[構成設計書](docs/大規模仮想マシン%20構成設計書.md) に沿って、**P0 の骨格・検査基盤と P1 のイベント実行基盤**を構築しています。
**ファームウェアや実機のエミュレーションはまだ実行できません。**

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

`cargo run --locked -p mosaic-apps-cli` は状態表示だけを行う仮の CLI です。

## 構造

- `framework/`: 共通語彙、契約、ランタイム、実行エンジン、記述・組立の境界。
- `catalog/`: 種別 → 出自で配置するモデルと宣言。現在、モデルは未登録です。
- `apps/`: 実行アプリケーション。
- `xtask/`: Cargo の依存宣言に基づく層・命名の検査。
- `docs/adr/`: 初日の設計判断と未決事項。

### 実装済み

- 15 クレートの Cargo workspace、層メタデータ、unsafe 禁止。
- `lint-layers`: framework の逆依存、L3 間依存、catalog 横依存、glue の依存を検査。
- `lint-names`: 禁止語、kebab-case、クレート名、正規の出自を検査。
- `check` / `doctor`、fast CI（fmt、構成 lint、Clippy、単体テスト）。
- `Addr` / `VTime` / `Attr` / `BusFault` / 型付き Snapshot の境界。
- 最小限の model context、MMIO、signal、clock、CPU-engine contract（暫定 API）。
- 決定論的イベントキュー、ノード専用 Ctx、遅延 signal、型付きキュースナップショット。
- 同時刻順序・取り消し・境界・不正な復元・再実行一致のテストと、期待出力を比較する実行例。

CPU やファームウェアを使わないイベント実行例:

```sh
cargo run --locked --package mosaic-core-runtime --example events
```

同時刻イベントと遅延 signal のトレースを表示し、保存状態からの再実行が一致することを確認します。
これは VM 全体の起動・スナップショット機能ではありません。

### 次の実装

P1 の最初の ISA・MCU・ボードを決め、Region / FlatView、モデルのディスパッチ、
エンジンアダプタ、UART とタイマ、SoC / ボード宣言をイベント基盤へ縦につなぎます。
実機モデルを増やす前に、決定論と MMIO の machine テストを整備します。

詳細は [開発ガイド](docs/guides/development.md) と [ADR 一覧](docs/adr/README.md) を参照してください。

## ライセンス・貢献規約

**オーナー判断待ちです。** ライセンスをエージェントが選択することはせず、全クレートを
`publish = false` にしています。Unicorn は未導入です。
公開・配布と外部貢献の受け入れ前に [ADR 0006](docs/adr/0006-license.md) を解決してください。
