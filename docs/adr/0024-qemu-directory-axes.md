# 0024: QEMU 型ディレクトリ軸への再編成(機能軸 `hw/`・アーキ軸 `machine/`)

状態: 採用。リポジトリのディレクトリ構造とクレート粒度を、設計書 §6 の
「framework/catalog の 2 分割 + 種別→出自の軸」から、QEMU の実際の構成
(機能別ディレクトリとアーキ別ディレクトリの併存)へ置き換える。

根拠: オーナー指示(2026-10-27)「大規模汎用仮想マシンとして見ると汎用に
なっていない」「大量にモデルを追加した時に発生するモデル管理の複雑さに
対処できていない。mxs のような安直な分け方をすると機能などが混合して
構造が分からなくなる。しっかり考える必要がある」「lib.rs 単体で終わらせ
ているものが多く実装がしづらい」。参照モデルは QEMU そのもの。

## 1. 旧構成(種別 → 出自)のどこが悪かったか

| # | 問題 | 実例 |
| --- | --- | --- |
| P1 | **出自(ベンダー)軸は「何の機能か」を答えない** | UART を探すのに `catalog/ip/arm/pl011`、`catalog/ip/st/usart-f1`、`catalog/ip/synopsys/dw-apb-i2c`(I2C)と 3 つの木を行き来する。QEMU なら `hw/char/` に全部並ぶ |
| P2 | **1 モデル = 1 クレートが lib.rs 1 ファイルを生む** | 71 クレート中、モデル系クレートの大多数が `src/lib.rs` 1 ファイル(最大 1010 行)。モデルが育つ(レジスタ表・bind・state に分かれる)場所が最初から無い |
| P3 | **SoC 名による分割は機能を混合させる** | `soc/<origin>/<family>/<chip>` 方式は、mxs 的な 1 ディレクトリに UART・タイマ・割込みコントローラ・ブートコードが同居する道。QEMU がやらない理由と同じ(mixing の M1/M5 が再発する) |
| P4 | **framework/catalog の 2 分割が「部品倉庫」と「VM 本体」を分断した** | 機械実装が apps に逃げる構造欠陥は ADR 0023 で部分的にしか直らず、`apps/` に縦スライス用デモバイナリが 5 本残ったまま |

設計書 §25 は「案 A 機能別(QEMU/Renode 型)不採用」としていたが、これは
「機能 1 軸に種別・出自・バス役割を詰め込むから混ざる」という理由による。
QEMU を実際に観察すると、QEMU は機能 1 軸に詰め込んで**いない**。
デバイス(再利用部品)と機械(SoC/ボード)で**軸そのものを分けている**。
本 ADR は「機能軸 + アーキ軸の使い分け」として案 A を修正採用する。

## 2. QEMU の実際(参照)

```
qemu/
├─ hw/                     # 全ハードウェアモデル
│  ├─ char/ intc/ timer/ rtc/ gpio/ ssi/ i2c/ sensor/ misc/ mailbox/
│  │  nvram/ pci/ ide/ virtio/ block/ net/ display/ dma/ usb/ sd/ …
│  │  # ← 機能軸: 再利用デバイス。ベンダー名はディレクトリでなく
│  │  #    ファイル名接頭辞(imx_serial.c, stm32f2xx_gpio.c)
│  ├─ arm/ i386/ m68k/ riscv/ …
│  │  # ← アーキ軸: SoC とボードの機械モデル(hw/arm/fsl-imx25.c 等)
│  └─ core/                # qdev・バス・loader など基盤
├─ target/                 # ISA と CPU モデル(ARM926 は target/arm の中)
│                          # ← Mosaic では isa/。cargo が target/ を
│                          #    ビルド出力ディレクトリとして予約しているため
├─ accel/                  # 実行エンジン(tcg/kvm/…)
├─ block/                  # ディスクイメージ形式
├─ system/                 # system emulation 本体(vl.c, runstate, memory)
├─ util/                   # 汎用ライブラリ
├─ pc-bios/                # ファームウェア blob
└─ qemu-system-<arch>      # 単一フロントエンド(vl.c は機械コードを持たない)
```

機械の追加は `hw/<arch>/*.c` にファイルを 1 つ足し `type_init` で登録
するだけで `vl.c` は 1 行も変わらない。デバイスの追加は `hw/<function>/`
に 1 ファイル。**モデル数千件でも破綻しない管理方式として実証済み**。

## 3. 決定

### 3.1 新しいトップレベル(QEMU との対応)

| QEMU | Mosaic(新) | 旧 | 中身 |
| --- | --- | --- | --- |
| `util/` | `util/` | `framework/prim/bits`, `framework/modelkit/fifo`, `core/types/fnv` | bits, fifo, fnv |
| `include/exec` 系語彙 | `exec/` | `framework/core/types` | Addr, VTime, Attr, RegId, BusFault, Snapshot |
| (契約はヘッダ) | `contract/` | `framework/contract/*` 12 クレート | **1 クレート 12 モジュール**(mmio, signal, clock, cpu_engine, gpio, i2c, spi, uart_line, can, net, stream, model) |
| `system/`(softmmu) | `runtime/` | `framework/core/runtime` | アドレス空間, アリーナ, イベント, クロックツリー, Machine |
| `target/` | `isa/{arm,aarch64,x86,riscv}` | `framework/isa/*` + `catalog/cpu/*` | **ISA 語彙と CPU モデルを同居させる**(QEMU と同じ)。Mosaic ではディレクトリ名を `isa/` とする(cargo が `target/` をビルド成果物の置き場所として予約しているため。QEMU の `target/` と同じ役割)。旧 `isa/x86`+`isa/x86-64` は `isa/x86` に統合 |
| `accel/` | `accel/{unicorn,armv7m,conformance}` | `framework/engine/*` | Unicorn アダプタが TCG 相当 |
| `hw/<function>/` | `hw/` 1 クレート | `catalog/ip/*` + `catalog/parts/*` + `framework/interconnect/*` | 機能別サブディレクトリ(char, intc, timer, rtc, gpio, ssi, i2c, can, sensor, misc, mailbox, nvram, pci, ide, virtio)。**1 モデル = 1 ファイル/1 ディレクトリ**(bme280 のような多バスモデルは `bme280/{mod,i2c,spi}.rs`)。バス実体(i2c-bus 等)は `i2c/bus.rs` のように QEMU の `hw/i2c/core.c` と同じ位置に置く |
| `hw/<arch>/`(機械) | `machine/` 1 クレート | `catalog/soc/*` + `catalog/board/*` + `apps/stm32f103` | アーキ別サブディレクトリ(`arm/`, `i386/`)。SoC とボードとアーキ共通ブート経路(`-kernel` のプロトコル)がここに住む |
| `block/` | `block/` | `framework/modelkit/iso9660` | イメージ形式(将来 raw/qcow2) |
| (pack / IR) | `describe/`, `assemble/` | 同名(維持) | manifest→MachineGraph、graph→実体 |
| `pc-bios/` | `firmware/` | `catalog/firmware` | ファームウェアの記述 |
| (データカタログ) | `manifests/` | `catalog/*.toml` | **コードと同じ形にミラーした manifest 木**(下記 3.3) |
| `qemu-system-*` | `mosaic/`(bin `mosaic`) | `apps/system` + `apps/cli` | 実行入り口は 1 本だけ |
| `tests/functional` | `tests/` | `tests/`(維持) | 実イメージ起動テスト(ADR 0022) |
| `chardev/ ui/ monitor/` 等 | `host/ ops/ bridge/`(プレースホルダ) | 同名(維持) | 将来層 |

### 3.2 軸の再定義(設計書 §5 の改訂)

* ディレクトリが表す軸は**「領域(domain)→ 機能(function)または
  アーキテクチャ(arch)」**とする。種別(kind)は manifest の ID と
  コードの配置規則に残るが、木の上位を占めない。
* **出自(ベンダー)はディレクトリの軸から外れ、識別子の名前空間に
  なる**(QEMU と同じ: `imx_serial.c` の `imx` 接頭辞)。
  `ip:arm/pl011` という ID はそのまま維持され、`origins.toml` も
  `manifests/` 直下に残る。
* 機能ディレクトリの語彙は QEMU の `hw/` に合わせる(char, intc, timer,
  rtc, gpio, ssi, i2c, can, sensor, misc, mailbox, nvram, pci, ide,
  virtio, …)。**`misc` は `hw/src/misc` のみ許可される例外**とする
  (設計書 N2 の禁止語リスト改訂)。

### 3.3 manifest は `manifests/` にコードと同じ形で置く

```
manifests/
├─ origins.toml
├─ isa/arm/{arm926ej-s,cortex-m3}.toml         # cpu.toml
├─ target/riscv/e31.toml
├─ hw/char/pl011.toml                           # ip.toml/part.toml は機能ミラー
├─ hw/intc/{pl190,pic-8259,ioapic,gic-v2,scs}.toml
├─ hw/{timer,rtc,gpio,ssi,i2c,can,sensor,misc,mailbox,nvram,pci,ide,virtio}/…
└─ machine/
   ├─ arm/stm32f103/soc.toml                    # machine/src/arm/stm32f103/ と対応
   └─ i386/pc/board.toml                        # machine/src/i386/pc/ と対応
```

* `xtask lint-catalog` はこの木を走査し、**manifest パスとコードパスの
  ミラー**(kebab↔snake 変換)を検査する。manifest と実装の隣接性は
  パス構造で機械的に保証される。
* ID(`ip:arm/pl011` 等)・`origins.toml`・describe のスキーマは変更しない。

### 3.4 クレート粒度(lib.rs 問題への回答)

* **契約・モデル・機械は「多数の小クレート」から「少数の大きなクレート
  + モジュール木」へ統合する**(71 → 18 クレート)。
  1 モデル = 1 ファイル(または 1 ディレクトリ)は QEMU と同じで、
  モデルが育ったらファイルがディレクトリに昇格する(bme280 が既に実例)。
* 層の依存規則はクレート境界で機械的に強制し続ける(設計書 P6 は維持):
  `hw`(デバイス)は `machine`(機械)を決して依存できず、その逆のみ
  許される。旧「種別 glue 依存行列」(ADR 0023 の R2/R5 改訂)は
  この 2 クレート分割に置き換わる。

### 3.5 削除するもの(実イメージ方針・ADR 0022 との整合)

| 削除対象 | 理由 |
| --- | --- |
| `apps/{bringup,vic-thermal,sensorbus,multicore}` | 合成ファームウェアを回す縦スライス用デモバイナリ。ADR 0022「自作合成イメージを使わない」と矛盾し、「入り口は 1 本」(ADR 0021)を侵害する。デバイスの単体テストは `hw/` モジュール内に残る |
| `apps/cli` | unicorn-smoke 開発用ツール。同等の検証は `accel/unicorn` の単体テストと CI の cargo test が担う |
| `catalog/soc/arm/versatile/…` と `catalog/board/arm/versatile-pb/` | 実機 Versatile/PB の忠実なメモリマップではなく、**合成テストトポロジー**であることが manifest 自体に明記されている。実機の versatile-pb は将来 TRM に基づき `machine/src/arm/` に本当の姿で書く |
| `apps/stm32f103` の合成ゴールデンファーム | `FIRMWARE_WORDS` 由来の自作イメージ。**STM32F103 は機械として `-M stm32f103` に昇格**し、`-kernel <実イメージ>` だけを受け付ける(下記 3.6) |

### 3.6 機械レジストリ(QEMU の `MODULE_INFO` 相当)

`machine::machines()` が `-M help` の一覧の正本。各機械は
`MachineInfo { name, summary, architectures }` を公開し、フロントエンドは
機械の追加時にディスパッチ 1 arm を増やす(QEMU の vl.c が
`type_init` 登録を集計するのと同じ分担。ADR 0021/0023 の方針を継承)。
`stm32f103` を登録する: 実チップ RM0008 のメモリマップを持ち、
`-kernel` でユーザ提供の実イメージを起動する(in-tree に検証用
ファームはないため、CI の実機テスト対象は引き続き `pc` のみ)。

### 3.7 CPU モデルの居場所(QEMU と同じ)

`catalog/cpu/*`(arm926ej-s, cortex-m3, sifive-e31)は `target/<arch>`
に移り、ISA 語彙と同居する(QEMU の ARM926 が `target/arm` にあるのと
同じ)。CPU モデル = ISA + エンジン設定という設計書 §12.1 の 4 層の
うち、「ISA とコア固有」を target が、「エンジン」を accel が持つ構成は
不変。

## 4. 層ポリシーの新しい依存行列(xtask が強制)

```
tooling(xtask, L8) / front(mosaic, L7) / tests(L7)
        │ 依存はすべて許可(front は machine, block のみで足りる)
        ▼
machine(L6)  → hw, accel, target, runtime, contract, exec, util, block, describe, assemble
assemble(L5) → contract, exec, runtime, describe, accel(unicorn 経由は feature)
describe(L4) → (外部依存なし)
runtime(L3)  → contract, exec
accel(L3)    → target, contract, exec          # hw/runtime と相互依存禁止
hw(L3)       → contract, exec, util            # machine は依存禁止(逆方向のみ)
target(L2)   → contract, exec
contract(L2) → exec
block(L2)    → (依存なし)
exec(L1)     → util
util(L0)     → (依存なし)
```

外部クレートは `unicorn-engine` が `accel/unicorn` のみ、`toml` が
`machine`/`tests`/`tooling` のみ、という許可リストで維持する。

## 5. モデル追加の定型作業(大量追加への回答)

| 追加するもの | 作業 |
| --- | --- |
| 機能デバイス(PL011 的な 1 モデル) | `hw/src/<function>/<model>.rs` を 1 ファイル作り、`hw/src/<function>/mod.rs` に 1 行、`manifests/hw/<function>/<model>.toml` を 1 ファイル |
| 多バス部品(BME280 的) | `hw/src/<function>/<model>/{mod.rs, i2c.rs, spi.rs}` のディレクトリ |
| SoC | `machine/src/<arch>/soc/<chip>.rs`(+ 必要なら manifests に soc.toml) |
| ボード機械 | `machine/src/<arch>/<board>.rs` か `machine/src/<arch>/<board>/`、レジストリとディスパッチに 1 arm |
| CPU モデル | `target/<arch>/src/cpu/<core>.rs` と manifest |
| 実行エンジン | `accel/<name>/` 1 クレート |
| イメージ形式 | `block/src/<format>.rs` |

いずれも「既存ファイルを触るのは集約 mod.rs かレジストリの 1 行だけ」
(QEMU の meson 追加 1 行と同じ性質)であり、機能が既存のどこかの
ディレクトリに溶け込むことはない。

## 6. 影響と移行

* `tests/`(実イメージ起動テスト)は入り口を `target/debug/mosaic` に
  向け直すだけで、台帳(images.toml)・ハーネス・期待値は無変更。
* CI はデモアプリ起動ステップを削除し、`mosaic` の front-end 実機
  テスト・`mosaic-runtime` の examples・`cargo test --workspace` に
  集約する。
* ADR 0023 の「機械モデルは `catalog/board/<origin>/<board>/glue`」は
  本 ADR により置換(機械は `machine/src/<arch>/`)。ADR 0021 の
  「入り口は 1 本」は強化維持(バイナリ名 `mosaic`)。
* 設計書(architecture.md)§5・§6・§8・§9・§15・§25 は本章に合わせて
  改訂する。
