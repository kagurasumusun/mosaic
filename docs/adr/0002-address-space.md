# 0002: Region 木と視点別 FlatView

状態: 暫定採用（実装は P1）。根拠: 構成設計書 §12, §13.1。

Region 木を唯一の正本にし、(AddressSpace, view) ごとに FlatView を生成する。
エンジン側の RAM / MMIO マッピングは派生物とし、変更時は対応範囲を無効化する。
CPU / DMA / security の視点は CPU 実装と独立させる。モデルには相対オフセットだけを渡す。

Addr は u64。初期実装は `addr_unit_bits = 8` のみを受理し、他の単位を黙ってバイトに変換しない。
Attr は master、security、privilege、access kind、exclusive、byte enable、endian を保持する。
debug read は副作用を起こさない。

未実装: Region、優先度・alias、FlatView、DMI、動的無効化。
検証: 境界・オーバーフロー・重なり・ROM overlay・CPU 別 view・DMA のテストを P1 で追加する。
