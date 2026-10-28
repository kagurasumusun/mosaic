# Mosaic

Rust 主体の大規模汎用仮想マシン。リポジトリ名を暫定のプロジェクト名・クレート接頭辞として使用しています。

CPU 実行は Unicorn Engine のみ（ADR 0030）。ディレクトリ構成は QEMU を
参照モデルとしつつ、`hw` の内側は責務別に分ける（ADR 0031: CPU が
アドレス空間から直接叩く `hw/src/ip/<function>/`、ファブリック経由で
しか到達できない `hw/src/part/<domain>/`、2 ドメイン以上から使う共有
コア `hw/src/modelkit/`）。モデルの接続面は「ポート面」に統一
（ADR 0032: 1 つの部品が i2c + gpio×16 + irq のように複数の面を持ち、
ボードは `[[part.face]]` で面ごとに配線する。部品もカタログから
spawn され、fabric エッジは結合パスで必ず解決される）。テストは
実イメージのみ（ADR 0022）。

## システムエミュレータとして使う

`mosaic` が実行の入り口である（ADR 0021/0024）。QEMU と同じ語彙で
マシンを選び、実イメージを付けて起動する。

```sh
# 実 CD イメージからブートする（ISOLINUX が画面にバナーを出すまで実行）:
cargo run --locked -p mosaic --features unicorn -- \
    -M pc -cdrom crates/tests/assets/tinycore-16-core.iso -m 64M -display text

# カーネルだけを ISO から取り出して直接起動する（-kernel/-initrd/-append）:
cargo run --locked -p mosaic --features unicorn -- \
    -kernel crates/tests/assets/tinycore-16-core.iso\!boot/vmlinuz \
    -initrd crates/tests/assets/tinycore-16-core.iso\!boot/core.gz \
    -append "console=ttyS0,115200 console=tty0" -serial stdio

# フロッピイメージからブートする（FreeDOS の KERNEL.SYS まで実行）:
cargo run --locked -p mosaic --features unicorn -- \
    -fda test-images/freedos-1.3-boot.img -boot a -display text

# Cortex-M 機械に実イメージを直接載せる（ベクタテーブルから SP/PC を起動）:
cargo run --locked -p mosaic --features unicorn -- \
    -M stm32f103 -kernel my-firmware.bin -serial stdio
```

`-M help`、`-cpu help` で選択肢一覧。`--report -` は JSON の実行報告書を
出す（停止理由・命令数・画面・シリアル・未処理割込み・例外）。

## ゲスト OS のテスト

実イメージだけを扱う（ADR 0022）。台帳は `crates/tests/images.toml`、
ハーネスは OS を知らない 1 本のループ:

```sh
cargo xtask images fetch        # 実イメージ（Alpine/SliTaz/Debian/ReactOS/Android-x86…）を取得
cargo test --locked -p mosaic-tests-boot --features unicorn -- --nocapture
```

目標の網羅表（Linux 全アーキ / Android 1.6-4.4 / Windows 系 / RTOS）は
[docs/os-targets.md](docs/os-targets.md) を参照。

## 開発を始める

Rust 1.85.0（`rust-toolchain.toml`）、rustfmt、Clippy が必要です。
初回は crates.io から開発ツール用の依存を取得します。

```sh
cargo fmt --all -- --check
cargo xtask check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo xtask doctor
```

必要な native ビルド依存、GPL-2.0 と制限は [Unicorn ガイド](docs/guides/unicorn.md)
を参照してください。

## 構造（責務別分類、ADR 0031）

```
crates/            ワークスペースクレートはすべてこの下（ADR 0028）
    mosaic/        実行入り口は 1 本（QEMU の system/vl.c 相当）
    util/          汎用部品（bits, fifo, fnv）           … QEMU util/
    exec/          共通語彙（Addr, VTime, RegId, …）      … QEMU include/exec 系
    contract/      全ポート契約（契約ごとに 1 モジュール）… QEMU include/hw/*.h
    bus/           接続方式の基盤（ADR 0025）             … QEMU hw/*/core.c の集約
        56 fabric: gpio/ i2c/ i3c/ spi/ can/ uart/ usb/ net/ pci/ sd/ …
    boot/          boot 基盤 + イメージ読み込み基盤（ADR 0025、decode 専用）
        loader/ ELF・uImage・PE・SREC/IHEX・multiboot・raw（自動判別）
        format/ ISO9660+El Torito・MBR/GPT・FAT12/16/32・initramfs cpio
        proto/  x86・arm32・arm64・riscv Linux・Cortex-M・multiboot・FDT
    runtime/       アドレス空間・イベント・クロック・Machine … QEMU system/
    isa/{arm,aarch64,x86,riscv}/  ISA 語彙 + CPU モデル   … QEMU target/ *
    accel/{unicorn,conformance}/  実行エンジンは Unicorn 1 本（ADR 0030）
    hw/            ハードウェアモデル。責務別 3 軸（ADR 0031）
        ip/<function>/  CPU がアドレス空間経由で直接叩くレジスタモデル
            char/ spi/ i2c/ gpio/ intc/ timer/ rtc/ dma/ sd/ usb/ net/
            pci/ ide/ nvram/ mailbox/ wdt/ virtio/ display/ audio/ input/
            clock/ platform/
        part/<domain>/  ファブリック経由でしか到達できない部品（正体軸）
            sensor/ memory/ storage/ expander/ mux/ rtc/ adc/ dac/ audio/
            display/ input/ net/ power/ motion/ fan/ led/ security/ nfc/
            instrument/ serial/ can/ i3c/ ec/
        modelkit/   2 ドメイン以上から使う共有コア
            primecell_id・i2c_pointer・pwm_device_core・unmapped_policy
    machine/       機械モデル（SoC とボード）。アーキ別     … QEMU hw/<arch>/
        arm/stm32f103/    i386/pc/
    describe/      manifest → MachineGraph（IR、clk は block.port 参照可）
    assemble/      IR → 実行時インスタンス（engine 結合・HostAttachments）
    tests/ xtask/  実イメージ boot テスト・CI 検査ツール
    host/ ops/ bridge/ ffi/  将来層のプレースホルダ（chardev/ui/monitor 等）
manifests/         manifest データ木（コードと同じ形にミラー:
                   hw/ip/<fn>/・hw/part/<domain>/・hw/modelkit/）
firmware/          ファームウェアの記述                    … QEMU pc-bios/
docs/ third_party/ ci-reports/
```

\* QEMU の `target/` に相当しますが、cargo が `target/` をビルド出力として
予約しているため `isa/` という名前です。

**分類の規則（ADR 0031）**: 部品の居場所は「何であるか」で決まり、どの
バスで話すか（`bindings`）は属性に過ぎない。多バス部品（BME280 型）は
1 モデル + bind 複数で表し、同一チップの別モデル化は禁止。`misc` のような
受け皿は存在しない — どこにも置けない部品が出たらドメインを ADR で追加する。

**モデル追加は定型作業**（ADR 0024 §5 + ADR 0025 + ADR 0031 + ADR 0032）:
アドレス空間に載るデバイスは `hw/src/ip/<function>/<model>.rs` 1 ファイル +
`ip/<function>/catalog.rs` 1 行 + `manifests/hw/ip/<function>/<model>.toml`、
ファブリック部品は `hw/src/part/<domain>/<model>.rs` + spawn 登録 +
`[ports]` 付き manifest。機械は manifest（部品・面ごとの wire・
`rcc.gate-apb2-iopa` 形式のクロック配線）+ 固有情報（どの UART がコンソールか等）
のみで、モデルの生成知識（IRQ・クロック・パラメータ・面）はカタログ側が持つため
ボードに再利用される。既存のどこかのディレクトリに機能が溶け込むことはありません。
複数インターフェースを持つ部品（MCP23017 型: I2C ターゲット + GPIO 16 本 +
INT、TCA9548A 型: 上流 I2C + 下流 8 セグメント、BME280 型: I2C/SPI 両対応）は
1 モデル + 複数ポート面として宣言・spawn・配線され、面ごとに別モデルや別管理
方法が生えることはありません。

### 実装済み

- 19 クレートの Cargo workspace、層メタデータ、（エンジン境界以外の）unsafe 禁止。
- `xtask check`: 層の依存方向（下向きのみ、`hw` は `machine` を依存禁止）、
  禁止語・kebab-case・クレート名、manifest のスキーマ/ID 一意性/参照解決、
  **manifest パスとコードパスのミラー検査**。
- `Addr` / `VTime` / `VDelta` / `Attr` / `BusFault` / 型付き Snapshot の境界。
- MMIO、signal、clock、GPIO、I2C、SPI、UART、CAN、stream、CPU-engine の各契約。
- 決定論的イベントキュー、ノード専用 Ctx、遅延 signal、型付きキュースナップショット。
- Region 木 → FlatView、優先度 overlay、境界・permission・device offset 解決。
- Unicorn アダプタ（2.1.5、ARM/AArch64/x86/RISC-V、x86 例外エントリと
  ポート I/O、CPU モデル固定）と、エンジン契約の回帰フィクスチャ（conformance）。
- `EngineBus` callback と `mmio_map` による engine 側 MMIO region、バス故障の記録と Fault 停止。
- デバイスアリーナ（take-out と再入拒否）、RAM/ROM バッキング、FlatView 経由の
  MMIO ディスパッチ、タイマ・signal の配線と配送を持つ `Machine`。
- P4 初期 runtime snapshot: queue + RAM/ROM + snapshot 対応 device State を束ねた
  in-memory checkpoint、topology fingerprint / 完全一致検証（[Snapshot ガイド](docs/guides/snapshot.md)）。
- L5 `mosaic-assemble`: FlatView から engine ページと MMIO region を導出する
  `bind_flat_view`、命令量子とイベント配送を交互に行う `MachineDriver`、
  固定順ラウンドロビンの `MulticoreDriver`（コア別ビュー・共有 RAM ミラー同期）。
- バスとピン（ADR 0025 の `bus` 基盤）: GPIO ネット、I2C（アドレス解決 + ビット
  単位デコーダと bit-bang 経路）、SPI（CS 配線）、CAN（決定性アービトレーション）、
  UART ペア、I3C（DAA・CCC・SDR）、USB（ポート・SET_ADDRESS・転送ルーティング）、
  Ethernet セグメント、PCI コンフィグ空間ルーティング、SD/eMMC コマンドポート。
- boot 基盤（ADR 0025 の `boot` 基盤、decode 専用）: loader 6 形式（ELF32/64 両
  エンディアン・uImage legacy/multi・PE32/PE32+・SREC・Intel HEX・multiboot、
  bzImage 認識と raw 代替）、format（ISO9660+El Torito・MBR 拡張チェーン・GPT
  CRC 検証・FAT12/16/32 LFN・initramfs cpio 連結）、proto 8 種（x86 Linux
  boot_params・ARM32 Linux+ATAGS・AArch64 Image header・RISC-V・Cortex-M リセット
  列・multiboot 情報構造・FDT/DTB 解析と in-place fixup）。
- モデルカタログ（ADR 0025 + ADR 0031）: 各 `ip` 機能ディレクトリと
  `part` ドメインの `catalog.rs` が全モデルを自己登録（QEMU TYPE_INFO 相当、
  コード側カタログと manifest 側カタログの一致を CI が検査）。
  `ModelEntry::outputs` が RCC 43 ゲート等のベンダー出力表を公開し、
  宣言的 `clk` 配線を可能にする。stm32f103 ボードはファクトリクロージャ
  0 個で manifest とカタログだけで組まれている。
- 複合部品の実モデル（ADR 0031 §3 + ADR 0032 の実証）: TCA9548A
  8 チャネル I2C スイッチ（上流 i2c ターゲット面 + 下流 `down0..7`
  イニシエータ面、part/mux）、MCP23017 16 ビット GPIO エキスパンダ
  （i2c 面 + gpio 16 ライン面 + int 出力面、GPINTEN/DEFVAL/INTCON に
  従う割り込みオン・チェンジ、part/expander）、BME280（i2c/spi 両
  ターゲット面）、LM75A（i2c 面 + os サーモスタット出力）— いずれも
  カタログ spawn + `[[part.face]]` 宣言的配線で組み込まれ、ボードは
  生成コードを書かない。
- `ip:st/i2c-f1` を RM0008 §27 の実装に置換（旧: 16 バイトのスクラッチ
  RAM）: CR1/CR2/OAR1/OAR2/DR/SR1/SR2/CCR/TRISE、SB/ADDR/BTF/TxE/RxNE
  のマスタ送受信・リスタート付き読み出し、EV/ER の 2 本の IRQ 出力、
  RCC APB1 クロックゲート、`i2c` イニシエータ面。STM32F103 SoC マップに
  I2C1（0x40005400、NVIC 31/32）を追加。
- 個別モデル: NS16550A（FIFO・モデム線・ループバック）、VGA（レジスタ・
  VRAM アパーチャ・VBE）、8042 + PS/2 キーボード/マウス、HPET、8237A DMA、
  PL031 RTC、CFI NOR フラッシュ、SD カード（SD/eMMC 両プロトコル）+ SDHCI +
  PL181、LAN9118、UHCI + USB HID、I3C ターゲット、DW APB I2C、PL022、
  PIC/PIT/RTC/PL011/PL061/SP804/PL190/GICv2/SCS ほか。
- 機械: `-M pc`（BIOS INT サービス、ISA ポート空間、VGA text、El Torito・
  ブートセクタ・bzImage 直接起動）、`-M stm32f103`（RM0008 マップ、
  `-kernel` のベクタテーブル起動）。
- 実イメージ実績（ADR 0022 の台帳参照）: Tiny Core / Alpine / SliTaz /
  Debian / Android-x86 が ISOLINUX バナー以降、FreeDOS はカーネル完全起動、
  ReactOS は FreeLoader、Tiny Core カーネル直接起動はメモリマップ受理まで。

```sh
cargo run --locked --package mosaic-runtime --example events   # イベントと signal の決定論
cargo run --locked --package mosaic-runtime --example machine  # MMIO・タイマ・BusFault
cargo run --locked --package mosaic-runtime --example snapshot # checkpoint / replay
```

これらはライブラリの使用例であり、VM 全体の起動ではありません。

### 次の実装

機械の追加（`virt` AArch64、`goldfish` Android、実機忠実の `versatile-pb`）、
part 側カタログの spawn 対応と board からの宣言的 `conn` 接続の拡充、
catalog 全モデルの State 化と checkpoint 対象拡大、永続化と State migration、
命令数と VTime を結ぶ clock tree、Unicorn 2.1.x に無い外部割り込み注入、
Linux/Android/Windows の各機械での実イメージ到達点更新
（[docs/os-targets.md](docs/os-targets.md)）。

詳細は [開発ガイド](docs/guides/development.md) と [ADR 一覧](docs/adr/README.md) を参照してください。

## ライセンス・貢献規約

**オーナー判断待ちです。** ライセンスをエージェントが選択することはせず、全クレートを
`publish = false` にしています。Unicorn は明示的な導入指示に基づき開発・CI 検証へ導入しましたが、
プロジェクトライセンスや配布条件の確認が完了したという意味ではありません。
公開・配布と外部貢献の受け入れ前に [ADR 0006](docs/adr/0006-license.md) を解決してください。
