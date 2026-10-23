# 0002: Region 木と視点別 FlatView

状態: 暫定採用（実装は P1）。根拠: 構成設計書 §12, §13.1。

Region 木を唯一の正本にし、(AddressSpace, view) ごとに FlatView を生成する。
エンジン側の RAM / MMIO マッピングは派生物とし、変更時は対応範囲を無効化する。
CPU / DMA / security の視点は CPU 実装と独立させる。モデルには相対オフセットだけを渡す。

Addr は u64。初期実装は `addr_unit_bits = 8` のみを受理し、他の単位を黙ってバイトに変換しない。
Attr は master、security、privilege、access kind、exclusive、byte enable、endian を保持する。
debug read は副作用を起こさない。

実装済み: container / leaf の Region 木、兄弟の優先度 overlay、symbolic FlatView、
範囲・permission・device offset の解決。上端は u128 の exclusive end で検査し、Addr::MAX の 1 byte も扱う。
同優先度の兄弟は宣言範囲自体の重なりを拒否する（container の穴同士だけの重なりも含む）。
高優先度 container の未マップの穴は低優先度の兄弟へ fall through する。
別々の FlatView を各視点用に生成できるが、security/master 属性による自動 view 選択は未実装。

未実装: alias / IOMMU、RAM backing、DMI、動的無効化、MMIO dispatcher。
転送は単一 FlatView segment に収まるものだけを受け付け、境界をまたぐ転送は Unmapped とする。
alignment / endian / Attr の最終処理はバス・デバイス適合層の責務として残す。
検証: 境界・オーバーフロー・重なり・ROM overlay・CPU 別 view・DMA のテストを P1 で追加する。
