# 0001: 同期 contract と Ctx

状態: 暫定採用（P0）。根拠: 構成設計書 §8, §10, §13.4。

- contract は型・trait のみ。`Send` を要求し `Sync` は要求しない。
- モデルの MMIO callback は `&mut self` と `&mut dyn Ctx` を受け取る。
- Ctx は現在のノードに束縛する。モデルは他モデルの参照、配線、ベースアドレスを持たない。
- signal の通知はキューへ積み、callback 終了後に処理する。同一モデルへの再入を拒否する。
- 初期 Ctx は時間・イベント・signal のみ。DMA / RNG / host / trace は P1 でユースケースとともに追加する。
- 動的ディスパッチを用いても contract を object-safe に保つ。

検証: P1 でアリーナ + take-out を試作し、再入と借用競合をテストする。
2 つ目のモデル・実装が通るまでは API を凍結しない。破壊的変更時は本 ADR を更新する。

## イベント基盤での具体化

`ModelContext` をコールバックのスコープでノードと出力 signal 集合に束縛します。
Ctx の schedule / cancel はそのノードのタイマだけを扱い、set_signal は許可された
出力だけを現在時刻のイベントとしてキューへ積みます。出力先ノードへの参照は渡しません。
`cancel` は自ノードの pending イベントに限定し、他ノード・消費済み ID には false を返します。

Ctx に object-safe な既定メソッド `schedule_after` を追加しました。時刻加算の overflow は
`TimeOverflow` で失敗し、キューを変更しません。絶対時刻予約・signal 通知とも、その場では
dispatch しません。

## マシン dispatch での具体化

`contract/model` に object-safe な `EventTarget::on_timer` を追加しました。ノードが予約した
タイマイベントは、キューから pop されたあとでだけ callback へ届きます。callback 内の
`Ctx` 効果はすべてキューへ積まれ、callback 中の再入は発生しません。

runtime の `DeviceArena` は `NodeId` で索引付けた box を保持し、MMIO callback の間だけ
デバイスを take-out します。取り出し中の同一ノードへの入れ子アクセスは
`BusFault::Reentrant` です。`Rc<RefCell>` は使いません。`MachineDevice`
（`MmioTarget + EventTarget` のランタイム合成 trait）により、1 個の box がバスとタイマの
両 callback を担い、モデルの状態が trait object 間で分割されることはありません。
DMA 経由の実再入シナリオは Ctx の DMA サービスと合わせて後続で検証します。
