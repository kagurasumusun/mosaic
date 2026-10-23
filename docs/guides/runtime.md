# 決定論的イベント基盤

`mosaic-core-runtime` は CPU やホスト実時間に依存しない単一スレッドのイベント基盤を提供します。
モデルの実行エンジンや全機械スケジューラではありません。

## キューと時刻

```rust
use mosaic_core_runtime::{EventKind, EventQueue};
use mosaic_core_types::{NodeId, VTime};

let mut queue = EventQueue::new(128);
let id = queue.schedule(NodeId(1), VTime(10), EventKind::Timer { token: 42 })?;
// 現在時刻はまだゼロ。予約時にはモデルを呼び出さない。
let event = queue.pop_next(); // ここで時刻 10 へ進む。
```

この断片の `?` は `ContextError` を返す呼び出し元を想定しています。
完全な実行例は `framework/core/runtime/examples/events.rs` にあります。

- 時間は u64 ピコ秒。同じ時刻では EventId（予約順）が小さいイベントから返します。
- ID はキュー内で一意。取り消し・dispatch 後も再利用しません。
- EventId カウンタ、時刻の加算、capacity は暗黙の wrap や飽和を行いません。
- 失敗した予約・取り消しは時刻も ID カウンタも変更しません。
- capacity は pending イベント数（signal を含む）の上限です。ゼロも有効で、予約をすべて拒否します。
- `pop_next` はイベントのある時刻まで進み、空なら時刻を変更しません。
- `pop_due(until)` は until 以下の次の 1 件を返します。None まで繰り返すことでその区間を処理できます。
  None の場合は時刻を until へ進めます。until 自体が過去なら状態を変えずに失敗します。
- event は呼び出し側へ返すだけです。callback の実行、エラー処理、実行予算は dispatcher の責務です。

キューは時刻・ID の順序付き map と、取り消し用の ID → 時刻インデックスを持ちます。
予約・取り消し・pop は O(log n)、先頭時刻の取得は map の先頭を参照します。

## callback 用 Ctx

`ModelContext` は、一時的なキューの可変借用・現在ノード・許可された出力 signal ID 集合から作ります。
モデルには `&mut dyn Ctx` だけを渡します。

- `schedule(at, token)`: 自ノードのタイマを絶対仮想時刻で予約。
- `schedule_after(delay, token)`: checked add による相対時間予約。overflow は `TimeOverflow`。
- `cancel(id)`: 自ノードの pending イベントだけを取り消し。外国ノードの ID や消費済み ID は false。
- `set_signal(id, high)`: 許可された出力 signal を現在時刻のイベントとして追加。未知の出力は `UnknownSignal`。

signal ID はこの API では出力の識別子であり、sink への参照ではありません。
実際の宛先解決は将来の配線・dispatcher が `(owner, signal)` に対して行います。
Ctx はその場で宛先を呼ばないため、callback 中に signal 経由の再帰は発生しません。
この性質は現在の Ctx の範囲のもので、将来の DMA / MMIO に対する take-out と再入検出は別途必要です。

callback をスコープから抜けて Ctx を解放したあと、dispatcher が次のイベントを取り出します。
同時刻への再予約は可能なので、無限のゼロ時間ループを止めるには dispatcher のイベント予算が必要です。
ここでは実行スレッド、ホストタイマ、wall clock、乱数を使用しません。

## キューの snapshot

`Snapshot` trait の `save` は `QueueState` を返します。イベント配列は時刻・ID 順に正規化されます。
`load` は次を検査してから、キュー全体を原子的に置き換えます。

- スキーマ版が 1 であること。
- イベント数が復元先の capacity 以下であること。
- pending の時刻が State の現在時刻以降であること。
- ID が一意で、すべて next_id より小さいこと。

入力配列の並び順は問いません。ID → 時刻インデックスは復元時に再生成します。
復元前の timeline で保持していた EventId は、State に保存されたモデル状態とともに置き換えてください。
古い handle と復元後の handle を混用することはサポートしません。

この State はメモリ内の型であり、バイト列の永続化形式ではありません。
外側の機械識別子・IR ハッシュ・ノードと出力の存在検証は、今後の全機械 snapshot 層の責務です。
不正な State の拒否はテストしますが、信頼できない巨大ファイルのパース上限はまだ対象外です。

## 動く例と CI

```sh
cargo run --locked --package mosaic-core-runtime --example events
```

二つのノードのタイマと出力 signal を予約し、タイマ処理からの追加予約も含めたトレースを作ります。
キューの保存状態を別インスタンスに復元して同じ処理を行い、出力と最終 State の一致を検証します。
GitHub Actions は `tests/runtime-events.stdout` との比較も行います。
この例は CPU、ファームウェア、実在デバイスの動作や精度を検証するものではありません。

## 仕様との対応

| 設計書 | 実装・テスト |
| --- | --- |
| §10, §13.4: Ctx、ノード分離、遅延 signal | `context.rs` の object safety / ノード所有 / signal 順序・拒否テスト |
| §13.2, ADR 0003: 仮想時間と決定論 | `events.rs` の時刻順・同時刻順・相対 overflow・反復一致テスト |
| §13.5, ADR 0004: 版付き State、復元前検証 | `events.rs` の往復・不正 State・入力順序・ID 枯渇テスト |
| §19 T8: 同じ入力と実行区間分割 | `identical_inputs_produce_identical_traces` / `time_window_partition_does_not_change_event_trace` |

残りの主要な P1 基盤は Region / FlatView、デバイスのアリーナと dispatch、CPU 実行予算、
最初の ISA・MCU の選定、UART / タイマ、実ファームウェアの machine テストです。
