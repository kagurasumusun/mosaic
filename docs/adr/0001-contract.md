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
dispatch しません。アリーナ + take-out による実際のモデル dispatch と再入検出は未実装です。
