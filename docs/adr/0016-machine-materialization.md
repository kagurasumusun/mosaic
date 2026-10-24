# 0016: MachineGraph から Machine を実体化する

状態: 初期実装を採用。全アプリの手書き composition を置き換えたとはみなさない。

## 背景

ADR 0013 は宣言を正規化 `MachineGraph` まで下げたが、識別子からモデルを生成して
配線する段は未実装のままだった。その結果、実行アプリはアドレスと `SignalId` を
ソースに書き、宣言と実行構成が別に進化する。構成設計書 §14.1 の
「Registry で識別子を引く → 生成 → 配線 → リセット → 実行」が、VM のコアとして
まだ存在しない。

assemble は catalog 実装に依存してはならない（設計書 §7.1 / §8）。factory は
composition root（アプリ、または将来の catalog-all）が供給する。

## 決定

- `mosaic-assemble` は catalog ID から `DeviceFactory` を引く `DeviceRegistry` と、
  `materialize` を提供する。graph のノード ID をソートした順で `NodeId` を割り当て、
  RAM/ROM backing と FlatView を region から作り、IRQ エッジを `Machine::connect` する。
- 出力ピンの `SignalId` は materializer がポート名順に割り当て、factory は
  `InstantiateCtx::output` の値だけを使う。factory が別の ID を出すと、runtime は
  その出力を未配線として扱う。
- 1 本の出力ピンを複数シンクへファンアウトするエッジは、runtime の配線が
  1 対 1 であるためエラーにする。黙って片方を捨てない。
- CPU ノードはデバイスではない。CPU 行きの IRQ エッジは `CpuLine` として返し、
  エンジンの `set_line` へは composition root がメールボックス経由で渡す
  （ADR 0009）。アドレス空間エッジは合成関係であり、信号配線ではない。
- クロックゲートやホストポート（UART ペア、GPIO プローブ）はモデルの公開定数と
  factory のキャプチャで結ぶ。graph に無い接続を materializer が推測しない。
- 未対応のエッジ種別（I2C / SPI / stream）は失敗させず `deferred` に残す。
  対応したことにしない。
- CPU catalog モデルはディスクリプタとライン語彙だけを持つ。コア固有の
  システムレジスタを偽の MMIO 窓にしない。NVIC / SysTick は `ip:arm/scs` に置く
  （設計書 §12.1）。`EngineArch::ArmV7mLe` はその語彙の追加であり、既存の
  非網羅マッチを壊さない。

## 受け入れ基準

- catalog 実装に依存せず、登録した factory だけで MMIO と IRQ が届く。
- factory が無い catalog ID は実体化時に失敗する。
- `soc:st/stm32f103` の宣言から、RCC ゲート・USART1・NVIC 入力 37・GPIO ピンが
  手書きと同じデバイス連鎖を再現する。
- STM32F1 USART は RM0008 の RDR / シフトレジスタ / ORE を持ち、送信専用の
  スタブのままにしない。
