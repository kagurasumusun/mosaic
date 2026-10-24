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

- `contract/cpu-engine`: 予算、停止理由、レジスタ、初期 memory API、MMIO 用の
  `EngineBus` callback 契約と `EngineBusFault`。vendor 型は含めない。
- `isa/arm`: Mosaic の ARM register ID（R0..PC）。
- `engine/unicorn`: vendor ID 変換、private memory、コード cache 無効化、`mmio_map`
  callback region。1 core = 1 instance。
- `engine/conformance`: backend 非依存の A32 スモーク検証と予算分割検証。
- `assemble`（L5、`unicorn` feature）: `bind_flat_view` が FlatView から engine の
  ページ構成を導出し、Ram/Rom は backing をコピー、Mmio は `MachineBus` 経由で
  runtime の `Machine` へ転送する。`MachineDriver` が命令量子とイベント配送を協調させる。
- `apps/cli`: assemble の結合を使う harness。L3 相互依存は作らない。

CPU catalog モデル、SoC、ボード、ファームウェアの起動は未実装です。
A32 スモークの成功は ARM ISA 全体や実在 MCU への適合性を意味しません。

## MMIO の結合

engine は runtime の型を知りません。MMIO は次の経路で一意に解決されます。

1. `MachineBus`（assemble）が `Arc<Mutex<Machine>>` を共有し、`EngineBus` を実装する。
   実行は単一スレッドで、mutex は C callback との所有権境界を満たすためだけに使う。
2. `bind_flat_view` が FlatView の全リーフを検査する。ページ境界でないリーフは
   `BindError::NotPageAligned` で構築時に失敗する。
3. ゲストが Mmio region をアクセスすると Unicorn の hook が region 基点だけを捕捉した
   closure から `EngineBus` を呼ぶ。Unicorn は region 相対 offset を渡すため、engine 側で
   基点を足し戻した絶対アドレスで `Machine::bus_read/write` へ転送し、FlatView 解決・
   device callback・イベント予約を処理する。
4. device が `BusFault` を返すと hook が `emu_stop` し、`run` は `Fault`、
   `take_bus_fault` で故障したアドレス・幅・理由を取り出せる。
5. `MachineDriver::step` は命令量子の実行後、現在時刻までのイベントを配送する。
   MMIO 副作用で予約された signal は callback 復帰後に届き、engine へ再入しない。

## 制限

- ページは 4 KiB 単位、32 bit アドレス範囲、engine ごとに private。
- 1 回の命令予算は 1..=1,000,000。ループもこの予算で停止させ、wall clock timeout は使わない。
- run が返す数は pre-instruction hook 数。fault の最後のフックを retire と扱わない。
- 未マップ・permission・命令障害は Fault。精密な例外分類・例外配送は未実装。
- guest write / execute permission と loader/debug write を区別する。
- loader/debug read/write は単一のマップ区間に収まることが必要。空の転送は no-op。
- MMIO callback は実装済み（上記「MMIO の結合」）。DMI（host pointer）/ IRQ line 契約 /
  engine snapshot / shared RAM / SMP / 動的 view 更新は未実装。
  runtime 側には FlatView 経由の MMIO ディスパッチと RAM バッキングがあります（[runtime ガイド](runtime.md)）。
- バス故障時の読み出し値は実ゲストには届かない（`emu_stop` 後に Fault へ遷移）。
  精密な例外配送は未実装で、`precise_exceptions = false` の表明どおりです。
- instruction_count は true、precise_exceptions / exclusive_monitor は false。
- native handle・vendor enum・Context・user hook は公開しない。

## M-class（Cortex-M3）構成

`UnicornCortexM3` は `UC_MODE_MCLASS`（CPU モデルは Cortex-M33）で構成した
ARMv7-M 用アダプタです。Unicorn 2.1.x では `UC_MODE_THUMB` がデッドコード
で、M-profile のリセットは未マップのベクタテーブルから T ビット 0 で起動
するため、アダプタは構築直後に R15 経路（モードバンクに触らない唯一の
経路）で T ビットを立て、`run()` の begin に `pc | 1` を渡して T32 を
維持します（ADR 0012）。

- PC 書込みは半語整列（偶数）。A32 側は 4 バイト整列を維持し、奇数値を
  `InvalidValue` で拒否します（黙って A32 実行するのを防ぐ）。
- `set_line` は `IrqLine(0)`（maskable exception 入力）のみ受理します。
  ARMv7-M に FIQ 入力はありません。
- ISA 語彙は `mosaic-isa-arm` を共用します。

採用の経緯と他案は [ADR 0012](../adr/0012-mclass-t32-execution.md) を
参照してください。STM32F103 スライスが end-to-end の検証を担います。

Send の不変条件と 1 箇所の unsafe impl については [ADR 0007](../adr/0007-unicorn-arm.md) を参照してください。

## ライセンスと配布

導入した Rust crate の申告ライセンスは **GPL-2.0** です。
依存をクレート境界に閉じ込めても、ライセンス条件やリンクした配布物の扱いが自動的に変わるわけではありません。
プロジェクト自身のライセンス、DCO / CLA、配布形態の判断は引き続き ADR 0006 の未決事項です。
`publish = false` や feature による opt-in は法的な適合性の保証ではありません。
今回は CI でビルド・実行を検証し、release / バイナリ公開 / artifact 配布は追加しません。
