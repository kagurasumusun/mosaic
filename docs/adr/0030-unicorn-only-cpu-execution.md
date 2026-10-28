# ADR 0030: CPU エミュレーションは Unicorn Engine に一本化する

* 状態: 採用
* 日付: 2026-10-28
* 関連: ADR 0007 (Unicorn ARM), ADR 0012 (M-class T32), ADR 0017 (自前
  ARMv7-M エンジン), ADR 0018 (IA-32 バックエンド), 設計書 §12.1

## 背景

設計書 v0.2 は「CPU 実行は初期 Unicorn Engine、最終的に自前実装へ置換」
を前提とし、ADR 0017 はその第一歩として ARMv7-M 自前インタプリタ
(`crates/accel/armv7m`, 1641 行) を追加した。しかし自前実装は
Thumb-1 の部分実装にとどまり、Unicorn が既に持つ ISA 群 (ARM/AArch64/
x86/RISC-V ほか) に対して同等のカバレッジを自力で再構築するコストは、
ハードウェアモデルの大量実装という本体の目的と競合する。

また 2 エンジン体制は維持コストを実質 2 倍にする。`accel/conformance`
の差分検証、機械側の `unicorn` / `native` 二重 feature、CI の
ネイティブビルド依存 (cmake, libclang)、同じボードハーネスの二重管理
(`unicorn_run.rs` / `native_run.rs`) がすべてこの体制の維持費である。

オーナー指示 (2026-10-28): 「CPU エミュレーションについては、自前実装を
削除し、Unicorn Engine のみを使用する構成にしてください。」

## 決定

1. **CPU 実行エンジンは Unicorn Engine 1 本だけ**とする。`CpuEngine`
   契約 (`contract/src/cpu_engine.rs`) は不変のまま維持する。将来
   Unicorn が対応しない ISA (Xtensa, AVR, Z80 等) が必要になった時点で
   あらためて ADR を起こす。設計書冒頭の「最終的に自前実装へ置換」は
   本 ADR で **撤回** する。
2. **`crates/accel/armv7m` を削除する**。これに伴い:
   * `mosaic-machine` の `native` feature、`machine/src/arm/stm32f103/
     native_run.rs` を削除する。
   * ワークスペース members から `crates/accel/armv7m` を外す。
   * `accel/NATIVE.md` を削除する。
3. **`accel/conformance` は残す**。エンジン間差分検証のためではなく、
   「`CpuEngine` 契約に適合する実行コア」のバックエンド独立
   フィクスチャ (A32 smoke) として、Unicorn アダプタの回帰テストに
   使い続ける。エンジンが 1 つでも、契約を満たすことを独立に検証する
   価値は失われない。
4. **GPL 境界は不変** (ADR 0006 未決定のまま): `unicorn-engine` 依存は
   `crates/accel/unicorn` に限定し (R7 許可リスト)、エンジン境界以外の
   `unsafe` 禁止も維持する。

## 移行

* `stm32f103` 機械の `run()` は Unicorn パスのみになった (feature
  `unicorn`)。
* `xtask` の層検査・`check` に変更なし (accel ファミリは `crates/accel/
  <engine>` の実パス語彙のまま)。
* ADR 0017 の状態を「破棄 (0030 で置換)」へ、ADR 0012 のエンジン構成
  は Unicorn 1 本へ読み替える。

## 負の影響と緩和策

* **Thumb-1 精密例外の喪失**: 自前エンジンが持っていた命令境界での
  停止・精密な例外入口は Unicorn のフック粒度に依存する。M-class の
  ブート検証は `-M stm32f103` + 実イメージの bring-up 台帳
  (ADR 0022) で扱い、精度が必要になった時点で ADR を起こす。
* **将来 ISA の追加経路が閉じる**: Xtensa 等は Unicorn が対応しない
  (設計書 §2 の調査どおり)。必要になった時は KVM 等のハード支援か
  新しい自前エンジンの ADR で再検討する。

## 検証

* `cargo build --locked --workspace --all-features` — 緑。
* `cargo test --locked --workspace --all-features` — 全パッケージ緑。
* `cargo run -p mosaic-xtask -- check` — OK。
