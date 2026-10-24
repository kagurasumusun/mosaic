# 0004: State とスナップショットの版管理

状態: 暫定採用。根拠: 構成設計書 §10, §13.5。

各モデルは State とスキーマ版を持つ。`Snapshot` trait は型付き State の save / load を定義する。
load は版・内容を検証してから変更し、失敗時に既存状態を壊さない。
旧版への対応は暗黙ではなく、モデルの明示的な移行として追加する。

将来の保存ヘッダ: エンティティ識別子、State 版、IR ハッシュ。
構成不一致は復元前に拒否する。ランタイムの時刻・イベント・RNG・RAM・CPU 状態も保存対象。
ファイル・ソケットなどホストハンドルは保存しない。復元時にポートから再接続する。

P0 は trait / error 型から開始。バイト形式、ハッシュアルゴリズム、移行 API、永続化、
差分メモリ保存は未実装であり、現時点でファイル形式の互換性は約束しない。


## EventQueue の型付き State（版 1）

仮想時刻、次の EventId、pending イベント（所有ノード・時刻・種別・token / signal）を保存します。
取り消し用インデックスは派生データとして復元時に再構築します。capacity は構成値であり、
復元先の上限を超える State は拒否します。

load は版、過去イベント、重複 ID、未割り当て ID、容量を検査し、両インデックスが完成してから
置き換えます。失敗時は元の時刻・カウンタ・イベントを保持します。入力配列の順序に依存せず、
保存形式は (時刻, ID) 順に正規化します。

復元は ID 空間を含むタイムライン全体を置換するため、復元前の EventId を持ち越してはいけません。
これはキュー単体の snapshot であり、モデル状態・RAM・CPU・IR ハッシュ等の全機械検証は未実装です。

## P4 初期実装: runtime checkpoint

`Machine::save_snapshot` / `load_snapshot` は EventQueue、RAM/ROM backing、
`register_snapshot_device` で登録した全デバイス State を一つの in-memory checkpoint に束ねます。
`Snapshot::validate` を side-effect-free な事前検査として追加し、graph / queue / backing / 全 device State
の検査が済むまで変更しません。各 device のロードが contract 違反で失敗した場合、先に成功した device を
保存済みの State へ戻します。queue と backing は検査後の staged commit です。

Graph fingerprint は決定論的 FNV-1a 64-bit の互換チェックサムです。衝突は可能なため、checkpoint は
typed topology も保持し、同一プロセス内の復元では fingerprint だけでなく完全な値を比較します。
暗号用途や永続ファイルの識別子には使いません。

**対応範囲の制約**: この checkpoint は Rust の in-memory 値であり、Device State の `TypeId` に依存します。
全 device が snapshot 対応でない場合、および SignalSink が存在する場合、部分 snapshot を作らず拒否します。
CPU engine context、IRQ sink/mailbox、Unicorn の shared-memory mirrors、host port、永続化 codec、State 移行、
差分 RAM はまだ対象外です。詳細と次段階は [Snapshot ガイド](../guides/snapshot.md)。
