# 0015: Checkpoint identity, clock dynamics, and catalog State

状態: 暫定採用。根拠: 設計書 §10 / §13.5、ADR 0003 / 0004 / 0011。

## 判断

1. `Snapshot::fingerprint(version, state) -> u64` を必須にする。実装は FNV-1a 64（`Fnv64`）で、モデルの emulated state だけを畳む。ホストポート、配線、`TypeId`、型名は含めない。
2. `MachineSnapshot` の schema を 2 にする。追加するのは clock-tree dynamics と `state_hash` である。`state_hash` は queue、backing bytes、登録デバイスの `(node, version, fingerprint)`、clock dynamics を畳む。topology は exact 比較のままにし、hash には入れない。
3. `load_snapshot` は device `validate` の後、mutation の前に `state_hash` を再計算し、不一致を `InvalidState` とする。これは改竄検出であり、cryptographic digest ではない。永続 codec の完全性には使わない。
4. clock tree は runtime が所有する。構造（kind、source Hz、親）は topology で、gate enable / mux select / divisor は dynamics である。catalog モデルは runtime に依存せず、周期は合成根が `instruction_period` で解決して渡す。gated / 1 ps 未満は `ClockError` とし、0 周期にしない。
5. `Ctx::schedule_after` は `VDelta` を取る。呼び出し側が `VTime` を遅延と取り違えないためである。
6. catalog の機能モデルは emulated state の版付き checkpoint を持つ。Byte stream、SPI/I2C initiator、GPIO ポート、clock 入力配線、downstream target はホスト接続として checkpoint に入れない。FIFO の中身と、queue が再発行する `EventId` は入れる。GPIO の `odr` 復元はネットを再駆動しない。
7. IRQ mailbox は engine binding とは別に pending edge を保存・復元する。Unicorn `Context` は `!Send` のまま `MachineSnapshot` に入れない。

## 理由

ADR 0011 は runtime の初期境界を切ったが、clock と catalog State が抜けると再実行が分岐する。hash を型消去した値から再計算できるようにしないと、load は「同じバイト列を戻した」ことしか保証できない。clock を catalog に漏らすと層境界（catalog は runtime を知らない）が壊れる。

## 退出条件 / 次の作業

- `MachineGraph` から clock tree と snapshot device を組み立てる（ADR 0013 の未実装）。
- driver coordinator が mailbox edge と device 出力レベルを同時に合わせる。CPU context は engine-owned boundary のまま別途設計する。
- 永続 codec、migration、cryptographic digest、untrusted input limit、COW RAM はまだ範囲外。

検証は [Snapshot ガイド](../guides/snapshot.md)。
