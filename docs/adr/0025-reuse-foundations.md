# ADR 0025: 再利用基盤 (bus/boot) とモデルカタログ

Status: accepted
Date: 2026-10-27
Supersedes: none (extends ADR 0024)

## Context

ADR 0024 はディレクトリ軸を QEMU 型(機能軸 `hw/<fn>/`・アーキ軸
`machine/<arch>/`)へ置換したが、運用して 3 つの構造欠陥が残った。

1. **個別モデルがボードから再利用できない**: ボード (stm32f103/board.rs)
   はファクトリクロージャで各モデルの生成・IRQ 番号・クロックゲート接続
   を全部握っていた。モデル追加のたび「モデル実装 + ボード側クロージャ +
   手配線」の 3 ヵ所を書き、しかもその知識は次のボードに 1 行も再利用
   されない (QEMU の `TYPE_INFO` 登録と対極)。
2. **接続方式の基盤がデバイスの中に埋まっている**: I2C バス本体は
   `hw/src/i2c/bus.rs`、GPIO ネットは `hw/src/gpio/net.rs`、UART ペアは
   `hw/src/char/pair.rs` と、ファブリック(配線基盤)が機能別ディレクトリ
   に散在。I3C・USB・Ethernet・PCI・SD の語彙はどこにも無く、これらを
   追すとさらに散在する。
3. **boot とイメージ読み込みが機械ごとに閉じている**: Linux boot プロト
   コル (`machine/i386/pc/linux_boot.rs`) と Cortex-M リセット列
   (stm32f103 の vector_entry) は機械モジュールの私有物で、他の x86/arm
   機械は再利用できない。QEMU は `hw/core/loader.c` (ELF/uImage/生) と
   `hw/<arch>/boot.c` (プロトコル) を全機械で共有している。

## Decision

### 1. `bus/` — 接続方式基盤 1 クレート (QEMU `hw/i2c/core.c` 等の集約)

バス本体・アドレッシング・アービトレーションを 1 クレート `bus` に、
「接続方式」1 ディレクトリ 1 ファブリックで集約する:

```
bus/src/
  gpio/   レベル解決ネット (プルアップ・オープンドレイン・衝突)
  i2c/    7bit アドレス + トランザクション + ビットバング復号
  i3c/    DAA・CCC・SDR (MIPI I3C Basic)
  spi/    チップセレクト全二重
  can/    ID アービトレーション (決定性キュー)
  uart/   Point-to-point ペア (バックプレッシャ)
  usb/    ポート・アドレッシング・転送ルーティング (SET_ADDRESS はバスの物)
  net/    Ethernet セグメント (フレーム交換・フィルタ)
  pci/    コンフィグ空間ルーティング (bus/device/function)
  sd/     SD/eMMC コマンドポート (カード↔ホスト間の基盤)
```

規則: ファブリックは `contract` の語彙のみ実装し、デバイスモデル
(`hw`)・機械 (`machine`)・合成 (`assemble`/`runtime`) に依存しない。
デバイス側の語彙 (I3cTarget, UsbDevice, EthernetPort, PciFunction,
SdCardPort) は `contract` に追加した。

### 2. `boot/` — boot 基盤 + イメージ読み込み基盤 (完全実装)

QEMU の `hw/core/loader.c` + `block/` + `hw/<arch>/boot.c` 相当を
decode-only で 1 クレートに:

```
boot/src/
  loader/  ELF32/64 (両エンディアン) / uImage (legacy+multi) / PE32・PE32+ /
           SREC / Intel HEX / multiboot / bzImage 認識 / raw (自動判別)
  format/  ISO9660 + El Torito / MBR (拡張含む) + GPT (CRC 検証) /
           FAT12/16/32 (LFN) / initramfs cpio (newc, 連結対応)
  proto/   x86 Linux (bzImage + boot_params) / ARM32 Linux + ATAGS /
           AArch64 Linux (Image header) / RISC-V Linux / Cortex-M リセット列 /
           multiboot 情報構造 / FDT (DTB) 解析 + in-place fixup
```

規則: バイト列を入力に型付き構造を返す decode のみ。合成も機械も持た
ない。reject-never-guess: 全フォーマット検査は失敗理由つきエラー。
機械 (`machine`) は配置とレジスタ規約のみを持つ。

### 3. `hw/src/catalog.rs` — モデルカタログ (QEMU `TYPE_INFO`)

各機能ディレクトリの `catalog.rs` がモデルを自己登録する:

```rust
pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:st/usart-f1", ...,
    outputs: &[...],                 // ベンダー出力表 (RCC のゲート 43 個等)
    spawn,                           // 既定生成: IRQ出力は宣言 edge から、
                                     // クロックは host から、パラメータは manifest から
}];
```

- `device_registry()` が全登録を集約 (QEMU `MODULE_INIT` 相当)。
  ボードは `materialize(&graph, &device_registry(), &hosts, ..)` を呼ぶ
  だけ。ファクトリクロージャ 0 個。
- `ModelEntry::outputs` がベンダー出力表 (シグナル番号) を公開し、
  `clk = 'rcc.gate-apb2-iopa'` 形式の宣言的クロック配線を可能にした。
- ホスト接続 (UART コンソール・I2C/SPI/USB/net/SD ファブリック) は
  `HostAttachments` として合成時に注入: ボードの固有物は「どの UART が
  コンソールか」「バスにどの部品を繋ぐか」だけ。

### 4. ボード = manifest + 固有情報のみ

stm32f103 ボードは宣言 (部品・領域・`rcc.gate-apb2-iopa` → `gpioa.clock`
クロック wire・USART1→NVIC37 / RCC CIR→NVIC5 の IRQ wire) + コンソール
endopoint + PA5 プローブだけになった。RCC ゲート接続・USART クロック
バインド・SCS IRQ 番号などの知識は全てカタログ側に移動した。

### 5. 依存方向 (level)

```
util(0) exec(1) contract/isa/boot(2) bus(3) runtime/accel(4)
describe(5) assemble(6) hw(7) machine(8) front/tests(9) tooling(10)
```

- `bus` は hw/machine/assemble/runtime/describe/accel を参照禁止。
- `boot` は decode-only: hw/machine/assemble/runtime/accel/isa/bus/
  contract/exec を参照禁止。
- `hw` は accel/isa を参照禁止 (デバイスは機関語彙を見ない)。

## Consequences

- モデル追加 = `hw/src/<fn>/<model>.rs` + `<fn>/catalog.rs` 1 行 +
  manifest 1 ファイル (CI がミラー検査)。ボード側は 0 行。
- 機械追加 = manifest (部品・wire) + 固有物のみ。同じ SoC を載せた別
  ボードは wire の manifest を書くだけで全モデルが再利用される。
- 接続方式の追加 (例: 1-Wire, CAN-FD) は `bus/<method>/` に 1
  ファブリック + `contract` に語彙、で済む。
- `block` クレートは `boot` に吸収され削除 (ISO9660 は
  `boot/src/format/`)。CI の layer 表は上記 5. へ更新。
- 既知の未完: PC 機械の firmware interim (内蔵 Serial16550/VgaRegisters)
  は `hw` の ns16550a/vga への置換が次工程。 PCI IDE/virtio 等の
  カタログ spawn はストレージ基盤の host attachment 待ち。
