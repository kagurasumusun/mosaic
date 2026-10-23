# 0009: CPU 割り込みラインの contract と受信経路

状態: 暫定採用（P1）。根拠: 構成設計書 §8, §10, §12.3, §13.3。
ADR 0001 の同期 callback 原則と ADR 0008 のポート注入原則を継承します。
本書は `CpuEngine` trait へのメソッド追加という破壊的変更を伴うため、
R8 に基づき判断を記録します。第 2 実装が契約を検証するまで暫定扱いです。

## 背景

P1 のボードでは PL011 と SP804 が combined IRQ を出力し、配線は runtime の
signal 配線表（`Machine::connect` と `SignalSink`）で表現します。しかし
CPU エンジン側には割り込みラインを受ける語彙がなく、signal を CPU 例外入力へ
橋渡しできません。Unicorn 2.1.x の公開 C API（タグ 2.1.4 と master の
`include/unicorn/unicorn.h` を確認）には ARM の外部割り込み注入 ctl が存在せず、
`UC_HOOK_INTR` は SVC などゲスト自身の例外命令の通知に限られます。このため
契約と後端能力を分離し、注入能力は capability として表明できるようにします。

## 決定

### 1. contract の語彙（`mosaic-contract-cpu-engine`）

- 論理ライン番号を表す newtype `IrqLine(pub u32)` を追加します。番号の意味付け
  （ARM の IRQ/FIQ 等）は isa 語彙またはボード構成側が定数で与え、contract は
  アーキ固有の番号を持ちません。
- `CpuEngine` に required メソッドを追加します。

  ```rust
  fn set_line(&mut self, line: IrqLine, high: bool) -> Result<(), EngineError>;
  ```

  新しい後端は無視する場合でも明示的に実装を書き、能力の表明を強制します。
- `EngineError` に `LineNotSupported` を追加します。未配線や未知のラインは
  ゲスト入力による故障ではなく制御プレーンの構成誤りとして扱います。
- `EngineCaps` に `line_injection: bool` を追加します。`true` は `set_line` が
  ゲストの例外入力まで到達することを保証し、`false` はライン状態の記録に留まる
  ことを意味します。`#[non_exhaustive]` により外部 crate の構造体リテラルは
  壊れません。

### 2. Unicorn アダプタの忠実度

`UnicornArm::set_line` はラインごとのレベルを内部テーブルに記録して `Ok(())`
を返します。`line_injection` は `false` です。外部割り込み注入が Unicorn の
公開 API で利用可能になった時点で、契約を変えずに実装だけを差し替えます。
P1 の出口条件（ポーリングによる UART 出力ゴールデンテスト）は注入能力に
依存しません。割り込み駆動ファームウェアの実行は注入能力を持つ後端を
必要とし、ベンチマーク結果と共に capability で判別できます。

### 3. signal から engine への受信経路（assemble のみ）

catalog と runtime は engine 型を知りません。橋渡しは composition root である
assemble に置きます。

- `IrqMailbox` は `(IrqLine, high)` のエッジを FIFO で保持する共有ボックスです。
- `EngineIrqSink` は 1 ラインにつき 1 個を runtime の sink ノードへ登録します。
  `SignalSink::set_level` は対応するラインのエッジを mailbox に積むだけで、
  engine を同期呼び出ししません。
- `MachineDriver` は各 step のイベント配送完了後に mailbox を FIFO 順で
  `engine.set_line` へ流します。engine 呼出しは quantum とイベント配送の間、
  つまり ADR 0001 / 設計 §12.3 が許す境界でのみ発生し、コールバックの再入は
  起きません。
- 配送順は signal イベントのキュー順序で決まり、決定論的です。同一ラインの
  連続する同一レベルも間引かず全エッジを記録・配送します。後端側での
  合体は後端の責務です。

## 影響

- 破壊的変更: `CpuEngine` 実装は `set_line` を必須実装します。現状の実装は
  作業空間内の `UnicornArm` の 1 個だけです。
- `EngineCaps` の構造体リテラル（`COUNTED` 定数）を更新します。
- runtime と catalog には変更がありません。既存の signal 配線表と
  `SignalSink` 契約をそのまま使い、CPU も「sink ノードを持つ 1 ノード」として
  配線できます。
- 検証: assemble のユニットテストで、デバイスの signal エッジが mailbox 経由で
  engine の記録テーブルへ FIFO 順に到達すること、未知ラインが構成誤りになる
  ことを確認します。実例外注入の検証は注入能力を持つ後端の導入時に行います。
