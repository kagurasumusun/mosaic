# 0011: In-memory runtime checkpoint (P4 initial slice)

状態: 暫定採用。根拠: 設計書 §13.5、ADR 0004。

## 判断

1. `Machine::save_snapshot` / `load_snapshot` は runtime の EventQueue、RAM/ROM、全 snapshot 対応デバイス State を束ねる。
2. State の type erasure は L3 runtime 内だけで行う。モデルは `Snapshot` と自身の具体 State を提供し、
   Unicorn・ランタイム共通の opaque state 型を catalog に漏らさない。
3. 復元は side-effect-free validation を先行し、topology / queue / backing / 全 device State が有効な場合だけ開始する。
   各デバイスの `load` は失敗時に自己状態を保つ。途中の失敗では既に適用した device State を rollback する。
4. 一部だけ保存した状態を完全 snapshot と呼ばない。未対応 device または SignalSink を持つ Machine は
   `SnapshotError::CapabilityMissing` で拒否する。
5. 初版は**同一プロセス内の typed checkpoint**。永続 codec、State migration、CPU engine、SignalSink/IRQ mailbox、
   external shared memory mirror、host I/O はこの ADR の範囲外。
6. Topology の fingerprint は安定した FNV-1a 64-bit の便宜的チェックサムとし、exact topology も保持・比較する。
   これは cryptographic hash ではなく、永続形式の安全性・完全性に使わない。

## 理由

現行 `Snapshot` trait はモデルごとの版付き State と EventQueue を表せるが、Machine の構成を越えて復元する
まとまりがなかった。まず machine runtime のみに境界を限り、復元前検証と再実行をテストできる段階を設ける。
RAM の初版は full-copy とし、COW / dirty-page tracking はベンチと lifecycle が固まってから導入する。

Unicorn Engine 2.1.5 の Rust binding は `context_init` / `context_save` / `context_restore` を持つが、
backend `Context` は opaque かつ `!Send` で、異なる engine schema / mode への共有はできない。
したがって Context を byte blob とみなして MachineSnapshot に入れず、engine-owned checkpoint boundary を
別途設計する。CPU 実行は引き続き Unicorn に委譲し、自前 CPU は P5 の最終段階で扱う。

## 退出条件 / 次の作業

- 各実利用モデルに State validation と snapshot registration を順次追加する。
- signal level、IRQ mailbox、各 CPU engine の文脈と共有ページを一緒に checkpoint する driver-level coordinator を作る。
- persistent versioned codec、migration、IR hash、untrusted input limits、COW RAM を設計・試験する。
- 記録再生、gdbstub、trace、fault injection は Machine snapshot の境界を使って後続実装する。

検証と API は [Snapshot ガイド](../guides/snapshot.md)、実行手順は
[runtime ガイド](../guides/runtime.md)を参照。
