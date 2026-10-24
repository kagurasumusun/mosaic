# 0013: 型付きマニフェストと正規化 MachineGraph

状態: 初期実装を採用。既存の手書き composition root は当面維持する。

## 背景

SoC/board の TOML と実行アプリの手書き配線が並存していると、宣言と実際の構成が
乖離しても検出できない。ランタイムや catalog 実装に parser 依存を持ち込まず、
マニフェストを検証可能な中間表現へ下げる層が必要である。

## 決定

- `mosaic-describe` は format-independent な `SocManifest` / `BoardManifest`、
  semantic validation、endpoint parser、正規化 `MachineGraph` を提供する。
- TOML の読み込み、ファイル走査、global catalog ID 解決は `xtask` に置く。
  framework crate は TOML parser に依存しない。
- `cargo xtask lint-catalog` は canonical ID/path、参照、領域サイズと重複、MMIO
  target/placement、IRQ endpoint、board/part/bus endpoint などを検証し、
  `MachineGraph` の node/edge/region 整合性も検査する。
- `MachineGraph` は runtime `NodeId`、trait object、host handle を保持しない。
  これにより構成検証と runtime instantiation を独立にテストできる。
- 現段階では registry 解決と `MachineGraph -> mosaic-assemble -> Machine` の自動構築は
  未実装であり、既存アプリの手書き composition を自動的に置き換えたとはみなさない。

## 受け入れ基準

- `cargo xtask lint-catalog` が checked-in catalog 全体に対して成功する。
- 既知の typo、重複 ID、無効な address range、未知の参照、誤った port/region target を
  否定テストで検出する。
- 同じ宣言の配列順を変えても、正規化 graph が同じになる。
- framework crate に parser 固有依存を追加しない。
