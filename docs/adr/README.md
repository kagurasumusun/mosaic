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
| [0012](0012-mclass-t32-execution.md) | T32 実行のための M-class エンジン構成（Cortex-M3、半語整列 PC） | 暫定採用 |
| [0013](0013-machine-graph-validation.md) | 型付き manifest、catalog validation、正規化 MachineGraph | 初期実装を採用、registry/assembly は未実装 |
| [0014](0014-multicore-time-semantics.md) | マルチコア時間モデルの保証範囲 | 暫定採用、serial-slice と lockstep を区別 |
| [0015](0015-checkpoint-identity.md) | checkpoint identity、clock dynamics、catalog State | 暫定採用、永続 codec と CPU context は未対応 |
| [0016](0016-machine-materialization.md) | MachineGraph の実体化と CPU 宣言の境界 | 初期実装、未対応エッジは deferred |
| [0017](0017-native-armv7m-engine.md) | ARMv7-M 自前エンジン（Thumb、精密例外、幅を保つ MMIO） | 初期実装、全命令は未対応 |
| [0018](0018-ia32-backends.md) | IA-32 の 16/32 ビットバックエンド、ポート I/O、割り込み入口 | 初期実装、例外とページングは未対応 |
| [0019](0019-software-interrupt-services.md) | ソフトウェア割り込みのサービス化と PC ファームウェア暫定 | 暫定採用、カーネル起動は未到達 |
| [0020](0020-x86-boot-core-resumability.md) | x86 ブートコアと再開可能性の限界 | 暫定採用、例外配送は実装済み・コンソールは未対応 |
| [0021](0021-system-front-end.md) | システムフロントエンド（QEMU 型 CLI、`mosaic-system`、`-cpu` モデル） | 採用 |
| [0022](0022-guest-image-testing.md) | ゲストイメージによるテスト方針（実イメージのみ、台帳駆動） | 採用 |
| [0023](0023-machine-model-placement.md) | 機械モデルの居場所（catalog board、`-kernel` は機械の経路、イメージ形式は modelkit、R2/R5 修正） | 採用 → 居場所は 0024 で移設 |
| [0024](0024-qemu-directory-axes.md) | QEMU 型ディレクトリ軸への再編成（機能軸 `hw/`・アーキ軸 `machine/`、manifest ミラー、クレート統合、入り口 1 本） | 採用 |
| [0025](0025-reuse-foundations.md) | 再利用基盤 (bus/boot) とモデルカタログ | 採用 |
| [0026](0026-fabric-expansion-and-mass-models.md) | ファブリック拡張 (9→31 種) と実モデル大量実装 (1000+ 件) | 採用 |
| [0027](0027-fabric-expansion-step3-video-audio-input.md) | ファブリック拡張 step 3 — ビデオ/オーディオ/入力の接続方式 12 種 (HDMI/DP/DSI/CSI/PS2/S/PDIF/eSPI/SoundWire/SLIMbus/FireWire/MIDI/GamePort) + 178 件の新モデル | 採用 |
