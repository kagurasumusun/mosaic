# Mosaic

Rust 主体の仮想マシン基盤。リポジトリ名を暫定のプロジェクト名・クレート接頭辞として使用しています。

[構成設計書](docs/大規模仮想マシン%20構成設計書.md) に沿って、現在は **P0（骨格と強制）の初期実装**です。
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

### 次の実装

P1 の最初の ISA・MCU・ボードを決め、Region / FlatView、イベントキュー、
エンジンアダプタ、UART とタイマ、SoC / ボード宣言を縦につなぎます。
実機モデルを増やす前に、決定論と MMIO の machine テストを整備します。

詳細は [開発ガイド](docs/guides/development.md) と [ADR 一覧](docs/adr/README.md) を参照してください。

## ライセンス・貢献規約

**オーナー判断待ちです。** ライセンスをエージェントが選択することはせず、全クレートを
`publish = false` にしています。Unicorn は未導入です。
公開・配布と外部貢献の受け入れ前に [ADR 0006](docs/adr/0006-license.md) を解決してください。
