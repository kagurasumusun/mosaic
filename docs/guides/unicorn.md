# Unicorn ARM 開発用アダプタ

## 動かす

```sh
cargo run --locked -p mosaic-apps-cli --features unicorn -- unicorn-smoke
```

CLI の `unicorn` feature は明示的な opt-in です。通常の CLI ビルドにはリンクしません。
ただし workspace 全体の build / test では engine クレートもビルドし、CI は all-features を検証します。
ビルドには C/C++ コンパイラ、CMake、pkg-config、libclang が必要です。Actions がこれらを用意します。

smoke は nested Region から FlatView を作り、RX コード領域と RW データ領域をマップして、
A32 命令列（MOV / ADD / STR / LDR）を 4 命令実行します。r0=42、r1=r3=43、
PC=0x1010、メモリ 0x2000=43 を確認します。期待出力は tests/unicorn-arm.stdout にあります。
命令列は手で符号化したテスト入力であり、外部ファームウェアのコピーではありません。

## 構造

- `contract/cpu-engine`: 予算、停止理由、レジスタと初期 memory API。vendor 型は含めない。
- `isa/arm`: Mosaic の ARM register ID（R0..PC）。
- `engine/unicorn`: vendor ID 変換、private memory、コード cache 無効化。1 core = 1 instance。
- `engine/conformance`: backend 非依存の A32 スモーク検証と予算分割検証。
- `apps/cli`: runtime FlatView と engine を組み合わせる harness。L3 相互依存は作らない。

CPU catalog モデル、SoC、ボード、ファームウェアの起動は未実装です。
A32 スモークの成功は ARM ISA 全体や実在 MCU への適合性を意味しません。

## 制限

- ページは 4 KiB 単位、32 bit アドレス範囲、engine ごとに private。
- 1 回の命令予算は 1..=1,000,000。ループもこの予算で停止させ、wall clock timeout は使わない。
- run が返す数は pre-instruction hook 数。fault の最後のフックを retire と扱わない。
- 未マップ・permission・命令障害は Fault。精密な例外分類・例外配送は未実装。
- guest write / execute permission と loader/debug write を区別する。
- loader/debug read/write は単一のマップ区間に収まることが必要。空の転送は no-op。
- MMIO / DMI / AddressView / IRQ / engine snapshot / shared RAM / SMP は未実装。
- instruction_count は true、precise_exceptions / exclusive_monitor は false。
- native handle・vendor enum・Context・user hook は公開しない。

Send の不変条件と 1 箇所の unsafe impl については [ADR 0007](../adr/0007-unicorn-arm.md) を参照してください。

## ライセンスと配布

導入した Rust crate の申告ライセンスは **GPL-2.0** です。
依存をクレート境界に閉じ込めても、ライセンス条件やリンクした配布物の扱いが自動的に変わるわけではありません。
プロジェクト自身のライセンス、DCO / CLA、配布形態の判断は引き続き ADR 0006 の未決事項です。
`publish = false` や feature による opt-in は法的な適合性の保証ではありません。
今回は CI でビルド・実行を検証し、release / バイナリ公開 / artifact 配布は追加しません。
