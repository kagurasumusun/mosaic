# Architecture Decision Records

設計書 §23 の初日項目を追跡します。0001–0005 は **P0 の暫定採用**であり、
2 つ目の実装で検証するまで contract を安定 API とみなしません。
0006 はオーナー判断待ちです。P0 の構築は最初の実機・ISA を決定しません。

| ADR | 判断 | 状態 |
| --- | --- | --- |
| [0001](0001-contract.md) | 同期 callback + Ctx + 遅延イベント | 暫定採用 |
| [0002](0002-address-space.md) | Region 木 → FlatView、視点と Attr | 暫定採用、実装は P1 |
| [0003](0003-virtual-time.md) | u64 ピコ秒、決定論既定 | 暫定採用 |
| [0004](0004-snapshot.md) | 版付き State、復元前検証 | 暫定採用、一部のみ実装 |
| [0005](0005-manifest.md) | TOML → MachineGraph、識別子の統一 | 暫定採用、実装は P1 |
| [0006](0006-license.md) | ライセンス・貢献規約 | 未決定・配布をブロック |
| [0007](0007-unicorn-arm.md) | Unicorn ARM、実行契約、Send 境界 | 暫定採用 |
| [0008](0008-serial-pin-contracts.md) | シリアルバス・ピン・ネット契約とポート注入 | 暫定採用 |
| [0009](0009-cpu-irq-line-contract.md) | CPU 割り込みラインの contract と受信経路 | 暫定採用 |
| [0010](0010-multicore-heterogeneous.md) | ヘテロジニアス・マルチコア実行（per-master ビュー、共有ホスト RAM、固定順ラウンドロビン） | 暫定採用 |
| [0011](0011-runtime-snapshot.md) | runtime State の初期 P4 checkpoint と復元検証 | 暫定採用、engine / sink / 永続化は未対応 |
