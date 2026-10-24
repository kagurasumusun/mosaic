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

この QueueState 単体はメモリ内の型であり、バイト列の永続化形式ではありません。
より上位の `Machine::save_snapshot` は queue に RAM/ROM と snapshot 対応デバイス State を束ね、
アドレス view / node / wiring の一致を検証します。これは初期 P4 のランタイム checkpoint であり、
CPU engine context、signal sink、外部共有 RAM、永続化・IR ハッシュ互換性までは含みません。
[Snapshot ガイド](snapshot.md)を参照してください。

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

## マシン: アリーナ・RAM バッキング・MMIO ディスパッチ

`Machine` は FlatView・デバイスアリーナ・RAM/ROM バッキング・イベントキュー・
signal 配線を 1 つにまとめた、CPU を含まない単一スレッドの実行基盤です。
ファームウェア起動の前段となる決定論・MMIO の machine テストをこの層で整備します。

```rust
use mosaic_core_runtime::{bus_attr, FlatView, Machine, Region};
use mosaic_core_types::{Addr, AccessSize, Mapping, MemoryKind, NodeId, Permissions, SignalId};

let view = FlatView::build(&[/* Rom / Ram / Mmio のリーフ */]).unwrap();
let mut machine = Machine::new(view, /* queue capacity */ 32);
machine.add_backing(NodeId(1), 0x1000).unwrap();
machine.register_device(NodeId(3), Box::new(model)).unwrap();
machine.register_sink(NodeId(4), Box::new(sink)).unwrap();
machine.connect(NodeId(3), SignalId(1), NodeId(4)).unwrap();
machine.verify().unwrap();

let attr = bus_attr(NodeId(2));
machine.bus_write(Addr(0x2000), AccessSize::Word, 3, &attr).unwrap();
machine.run_until(VTime(1000), 32).unwrap();
```

- **アリーナと take-out**: デバイスは `NodeId` で索引付けられ、MMIO callback の間だけ
  アリーナから取り出されます。取り出し中の同一ノードへの入れ子アクセスは
  `BusFault::Reentrant` です。`Rc<RefCell>` は使いません（設計書 §20、ADR 0001 の検証項目）。
- **MMIO ディスパッチ**: `bus_read` / `bus_write` は FlatView でアドレスを解決し、
  Ram/Rom はバッキングブロック、Mmio はデバイスの device-local offset へ転送します。
  Attr は変更せず callback へ渡り、Debug 読み出しの副作用禁止は契約上モデルが守ります。
  アライメント違反はバス層で `BusFault::Misaligned` にします。
- **RAM/ROM バッキング**: FlatView は記号的なままで、バイト列は `BackingStore` が所有します。
  `loader_write` は Rom も初期化できますが、view のゲスト permission は緩めません。
  エンディアンは Attr に従います。
- **イベントと signal**: モデルは `EventTarget::on_timer` を実装します。`MachineDevice` は
  `MmioTarget + EventTarget` のランタイム合成 trait で、1 個の box が両 callback を担います。
  signal は配線表 `(source, signal) -> sink` に従い、callback 復帰後に `SignalSink` へ届きます。
  未接続の出力はオプションポートとして無視し、配線ミスは `connect` / `verify` が構築時に失敗させます。
- **実行予算**: `run_until(deadline, budget)` は期限までイベントを配送します。
  同時刻の無限再予約は `budget` で止まり `MachineError::EventBudgetExceeded` を返します。
  キューが空なら deadline まで時刻を進めます。
- **runtime snapshot（初期 P4）**: `save_snapshot` / `load_snapshot` は queue、RAM/ROM、
  `register_snapshot_device` で登録した全デバイス State を一緒に復元します。
  未対応デバイスまたは SignalSink がある Machine は部分保存せず拒否します。
  CPU engine / sink / shared mirror と永続形式は未対応です。

```sh
cargo run --locked --package mosaic-core-runtime --example machine
```

この例は Rom/Ram 権限、MMIO 経由のタイマ予約、signal 配送、各種 BusFault をトレースし、
同一構成を 2 回実行して結果が一致することを確認します。CI は
`tests/machine-mmio.stdout` と出力を比較します。実ファームウェアや実在デバイスの検査ではありません。
手動のスループット測定は `mmio_throughput_probe`（`-- --ignored`）です。

### Machine snapshot と replay

```sh
cargo run --locked --package mosaic-core-runtime --example snapshot
```

RAM、snapshot 対応デバイスの State、未配送 timer をまとめて保存し、全てを変更した後に復元します。
同じ typed checkpoint から timer を再配送できることを表示し、CI は `tests/machine-snapshot.stdout` と比較します。
API の契約・制限・今後の P4 作業は [Snapshot ガイド](snapshot.md)を参照してください。

### 仕様との対応（マシン基盤）

| 設計書 | 実装・テスト |
| --- | --- |
| §10, §13.4, ADR 0001: アリーナ + take-out、再入拒否 | `arena.rs`、`taken_device_reports_reentrant_and_is_restored_afterward` |
| §13.1, ADR 0002: FlatView 解決、permission、Attr 透過 | `machine.rs` の権限・Attr・アライメントのテスト |
| §19 T8: 決定論、checkpoint 復元 | 同一入力トレース、queue checkpoint replay のテスト |
| §13.3: signal の遅延配送と配線 | timer→signal の順序テストと machine 実行例 |

### CPU エンジンとの結合（L5 assemble）

runtime と engine は同じ L3 で相互依存しません。連結は L5 の `mosaic-assemble`
（`unicorn` feature）が担います。

- `MachineBus` は `EngineBus`（contract/cpu-engine）を実装し、`Arc<Mutex<Machine>>` の
  MMIO callback を `bus_read` / `bus_write` へ転送します。Attr は CPU マスタ用の
  正規属性を結合層で合成します。
- `bind_flat_view` は FlatView だけから engine のマップを導出します。Ram/Rom は
  native page として backing を書き込み、Mmio は callback region として登録します。
  ページ境界外のリーフは構築時に `BindError::NotPageAligned` で拒否します。
- `MachineDriver::step` は命令量子を実行し、そのあと現在時刻までのイベントを配送します。
  device の MMIO callback が予約した signal は engine へ再入せず、量子の終了後に届きます。
  バス故障は `DriverError::Bus(EngineBusFault)` として返ります。

残るのは命令数と VTime を結ぶ clock tree、IRQ line（`set_line` 相当）の契約、
最初の MCU・ボードの選定、catalog 上の UART / タイマ IP、実ファームウェアの machine テストです。

Region / FlatView と開発用 Unicorn ARM アダプタについては
[Unicorn ガイド](unicorn.md) と ADR 0002 / 0007 を参照してください。
