# 0008: シリアルバス・ピン・ネットの contract とポート注入

状態: 暫定採用（P2）。根拠: 構成設計書 §8, §11, §13.3, §22。
ADR 0001 の同期 callback + Ctx 原則を継承します。既存 contract の破壊的変更は
ありません（R8 の ADR 更新義務は新規 contract の設計判断の記録として本書を置きます）。

## 追加する L2 contract（1 プロトコル 1 クレート）

| crate | 主な型・trait | 備考 |
| --- | --- | --- |
| `mosaic-contract-gpio` | `PinLevel`, `PinConfig`, `GpioPort`, `GpioSink` | ピンの入出力語彙。電気的解決は持たない |
| `mosaic-contract-net` | `NetDrive`, `NetPull`, `NetLevel`, `DigitalNet` | プル抵抗・オープンドレイン・tristate・衝突検出の語彙 |
| `mosaic-contract-i2c` | `I2cAddr`, `I2cPhase`, `I2cInitiator`, `I2cTarget` | 7/10bit アドレス、Ack、速度モード、ピン/トランザクション両対応 |
| `mosaic-contract-spi` | `SpiMode`, `SpiCs`, `SpiInitiator`, `SpiTarget` | CPOL/CPHA と CS を転送ごとに明示 |
| `mosaic-contract-stream` | `ByteStream`, `FrameStream`, `StreamError` | UART のバイト列と Ethernet 系のフレーム |
| `mosaic-contract-uart-line` | `UartConfig`, `Parity`, `UartLine` | 1対1 ペア接続用。設定検査をコンストラクタで |
| `mosaic-contract-can` | `CanId`, `CanFrame`, `CanController`, `CanReceiver` | 11/29bit ID、DLC 検査、調停用優先度 |

すべて `#![no_std]` + `#![forbid(unsafe_code)]`、trait は `Send` を要求し
`Sync` は要求しません。外部クレートは持ちません。

## ポート注入（catalog が interconnect を知らない構造）

R2 で catalog から参照できる framework は prim / core-types / contract /
modelkit / isa だけです。interconnect（L3）も runtime（L3）も catalog からは
見えません。そのため配線は次の形にします。

1. モデルはコンストラクタで **contract の port trait object**
   （例: `Box<dyn I2cInitiator>`、`Box<dyn GpioPort>`）を受け取り、状態として保持する。
2. 実体（i2c-bus, gpio-net などの interconnect）は composition root
   （assemble / app / テストハーネス）だけが構築・配線する。
3. モデルは他モデルへの参照を持ちません。port は「モデル参照」ではなく
   ADR 0001 の Ctx と同じく契約上の接点です。

これで §10 の「モデルは他モデルへの参照を持たない」を保ったまま、
MMIO を持たない外部部品（センサ等）をバス経由で駆動できます。

## I2C のピンレベルとトランザクションレベル

§11 の要求どおり、初めから両方の表現を契約に含めます。

- トランザクションレベル: `I2cInitiator::{write, read, write_read}` と
  `I2cTarget::{target_write, target_read}`。
- ピンレベル: `I2cPhase`（Start / RepeatedStart / Stop / Address / Data /
  Acknowledge）を共通語彙とし、SDA/SCL のエッジ列からの復号は
  interconnect の i2c-bus が受け持ちます。
- したがって同一の `I2cTarget` 実装が、コントローラ経由でも GPIO bit-bang
  経由でも動作します（P2 の出口条件）。

## 同期 dispatch と忠実度

functional 忠実度では、MMIO callback 内からの port 呼び出しは既存の
Machine MMIO dispatch と同様に同期で実行します。clock stretching、調停の
時間的挙動、マルチ initiator の同時実行は型（`I2cError::ClockStretch` 等）を
契約に用意し、詳細なタイミング実装は後続フェーズです。CAN の調停は
identifer 数値順の決定論ルールを interconnect 側で固定します。

## 検証と暫定扱い

- I2C: BME280 と TCA9548A の 2 実装が `I2cTarget` を実装し、コントローラ経路と
  bit-bang 経路の同一結果を app の統合テストで確認します。
- SPI: PL022 コントローラと BME280 の bind-spi で initiator/target の両側を検証。
- gpio/net / uart-line / can / stream: interconnect の conformance ユニットテスト
  （contract trait のモック実装）で検証。実デバイス 2 実装による確認は後続です。
- 2 つ目の実装で契約が破綻した場合は本 ADR を更新し、破壊的変更として扱います。
