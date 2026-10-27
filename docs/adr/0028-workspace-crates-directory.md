# ADR 0028: リポジトリルートの整理 — ワークスペースクレートを `crates/` へ集約

* 状態: 採用
* 日付: 2026-10-28
* 関連: ADR 0024 (QEMU 型ディレクトリ軸), ADR 0025 (bus/boot 基盤)

## 背景

ADR 0024 は「領域ごとの大分割」をルート直下に置く設計だった: `util/`,
`exec/`, `contract/`, `runtime/`, `isa/`, `accel/`, `hw/`, `machine/`,
`bus/`, `boot/`, `describe/`, `assemble/`, `mosaic/`, `tests/`, `xtask/`
に加え、将来層のプレースホルダ (`host/`, `ops/`, `bridge/`, `ffi/`)
とデータ木 (`manifests/`, `firmware/`, `docs/`, `third_party/`)、
生成物 (`ci-reports/`, `target/`) が並んだ。ファブリック拡張 (ADR
0025〜0027) でモデルが 1226 件・ファブリックが 44 種に育つ一方で、
ルート直下のディレクトリは 25 個まで増え、「どれがクレートで、どれが
データ木か」がひと目で分からなくなった。これは architecture.md §0 が
構造的に防ぐとした「ディレクトリがめちゃめちゃで、管理も実装もしづらい」
状態の初期症状である。

## 決定

1. **ワークスペースクレートはすべて `crates/` の下に置く**。Rust の
   ワークスペース慣行 (rust-lang/cargo, servo, rust-vmm 系) と同じ形:
   `crates/util`, `crates/exec`, `crates/contract`, `crates/runtime`,
   `crates/isa/{arm,aarch64,x86,riscv}`, `crates/accel/{unicorn,armv7m,
   conformance}`, `crates/hw`, `crates/machine`, `crates/bus`,
   `crates/boot`, `crates/describe`, `crates/assemble`,
   `crates/mosaic`, `crates/tests`, `crates/xtask`。将来層の
   プレースホルダ (`host/`, `ops/`, `bridge/`, `ffi/`) も同じく
   `crates/` の下に予約する (クレートになるものはクレート置き場に置く)。
2. **ルートに残すのは非クレートだけ**: ワークスペース設定
   (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`)、`README.md`、
   データ木 (`manifests/`, `firmware/`, `third_party/`, `docs/`)、
   生成物 (`ci-reports/`, `target/`)。レビュー文書 `IMPROVEMENTS.md`
   は `docs/IMPROVEMENTS.md` へ移した。
3. **軸の語彙は不変**: ADR 0024 の「領域 → 機能/アーキ」2 軸、
   ADR 0025 の bus/boot 基盤、manifest ミラー (`manifests/hw/<fn>/`
   ↔ `crates/hw/src/<fn>/`) はそのまま。`crates/` は居場所の整理で
   あって分類軸の追加ではない。ディレクトリが表す軸は引き続き 1 つ。
4. **パス検証は実パスで行う**: xtask の層検証 (`validate_layer`) と
   外部依存の境界検査 (`external_dependency_allowed`) は
   `crates/<domain>/...` の実パスを語彙として検証する。正規化や
   接頭辞の暗黙ストリップは `crate_name` (クレート名導出) に限定する。

## 移行

* `git mv` 19 クレート + 4 プレースホルダ → 履歴保持。
* ルート `Cargo.toml` の members を `crates/...` に更新。
* xtask: `workspace_root()` は `ancestors().nth(2)`、manifest ミラー
  検査は `crates/hw/src/...` / `crates/isa/...` / `crates/machine/...`、
  `crate_name` は `crates/` 接頭辞を剥がしてから名前導出、
  `lint_names` の kebab-case 検査は `crates/<root>` とデータ木
  `<root>` の両方を同じ語彙表で検査、hw 機能ディレクトリ特例は
  `crates/hw/src` (4 階層) と `manifests/hw` (3 階層) の両対応。
* tests クレート: `workspace_root()` を `ancestors().nth(2)` に、
  `ASSETS_DIR` を `crates/tests/assets` に更新。
* CI (`ci.yml`) と README のアセットパスを `crates/tests/assets/...`
  に更新。

## 負の影響と緩和策

* **外部ドキュメント・肌肉記憶との不一致**: ルート直下 `hw/` を期待する
  参照は壊れる。ADR と README の木構造を更新し、architecture.md に
  v0.4 の改訂注記を追加した。
* **パス語彙の二重性**: policy は `crates/hw` を語彙とし、manifest 側は
  `manifests/hw` を語彙とする。両者はミラー規則 (xtask lint_catalog)
  で結ばれているため、判断の分岐は残らない。

## 検証

* `cargo build --workspace --all-features` — 緑。
* `cargo test --workspace --lib` — 全パッケージ緑 (xtask のパス
  検証テスト、tests クレートの root 発見テストを含む)。
* `cargo run -p mosaic-xtask -- check` — manifest ミラー・層検証・
  名前 lint すべて OK。
