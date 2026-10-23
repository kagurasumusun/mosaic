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
