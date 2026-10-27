# 大規模仮想マシン 構成設計書

ドラフト v0.2 ／ 2026-10-27 改訂 ／ 前提: Rust 主体(C/C++ 併用)。CPU 実行は初期 Unicorn Engine、最終的に自前実装へ置換。

> **v0.2 の改訂(ADR 0024)**: ディレクトリの軸を「種別 → 出自」から
> **QEMU 型(領域 → 機能/アーキ)**へ置換した。§1・§5・§6・§8・§9・§15・§25 を
> 改訂済み。契約/実装の分離、振る舞いと配線の分離、IR 一本化、ctx、決定論、
> スナップショットといった設計原則は不変。

> **凡例** 【確認済】今回の調査で一次情報を確認した事実 ／ 【既知】筆者の既存知識(今回は再確認していない) ／ 【要検証】設計上の仮定で、実装前に小さな実験で確かめること。プロジェクト名は未定のため `<proj>` と表記する。

## 0. この文書について

**目的**: 大規模な VM プロジェクトが陥りがちな「ディレクトリがめちゃめちゃで、管理も実装もしづらい」状態を、最初の構成で構造的に防ぐ。

**範囲**: ディレクトリ/クレート構成、層と依存規則、命名、モデル内部の構造、SoC/ボードの記述方式、マルチ CPU・異種構成・カスタム SoC/ボード・多機械・協調シミュレーションなどの想定ケース、ビルド/CI での強制、テスト、ライセンス、ロードマップ。

**範囲外**: 個別 ISA の実装詳細、GUI の詳細設計。

## 1. 結論(要点)

1. **大分割は領域ごと(QEMU 型)**: `util / exec / contract / runtime / isa / accel / hw / machine / block / describe / assemble / mosaic(入り口)`。**デバイスは機能軸(`hw/char`, `hw/intc`, …)、機械はアーキ軸(`machine/arm`, `machine/i386`)**の 2 軸を使い分け、出自(ベンダー)はディレクトリでなく識別子の名前空間に持たせる(ADR 0024。旧案の framework/catalog 2 分割は構造欠陥のため置換)。
2. **ディレクトリが表す軸は「領域 → 機能 または アーキ」だけ**。出自・バス上の役割・忠実度・実装言語などは manifest のタグに持たせ、多軸の一覧は**生成**する。ディレクトリ木は 1 つの軸しか表せないので、多軸の属性を木に詰め込まない。
3. **契約(contract)と実装(hw・accel・runtime)を分け、モデルは 1 モデル 1 ファイル**。多数の単 file クレートは実装が育たないため、契約は 1 クレート多モジュール、デバイスは `hw` 1 クレートに機能別ディレクトリで収める(QEMU の `hw/` と同じ)。
4. **モデル内部を logic / bind / state に分ける**。「i2c の下の各モデルが混ざる」問題の最後の砦はここ。レジスタ定義・状態機械・バス適合・ホスト入出力を 1 ファイルに入れない。
5. **振る舞いと配線を分ける**。モデルはベースアドレス・IRQ 番号・接続先を知らない。SoC/ボードは宣言的記述が主体で、コードは本当に固有な部分だけ。宣言もコードも同じ中間表現(MachineGraph)に落とす。
6. **プロセッサは常にグラフ上のただのノード**。CPU モデル = ISA + 実行エンジン + コア固有部分。エンジンは「1 コア = 1 インスタンス」を前提にし、SMP・AMP・異種・多機械はスケジューラ側の責務にする。
7. **カスタム SoC/ボード/機密モデルは pack で提供する**。in-tree の catalog も「first-party pack」として同じレイアウトにし、外部 pack と区別しない。
8. **依存方向はクレートグラフで機械的に強制する**(xtask の層 lint + cargo-deny 等)。ルールは文書でなく CI に置く。
9. **初日に決めるべき不可逆事項は 6 つ**: contract の形、アドレス空間モデル、時間モデル、スナップショットのスキーマ、manifest スキーマ、ライセンス方針(§23)。
10. **Unicorn は GPL-2.0**【確認済】。自前エンジンへ置換する前提でも、配布形態と貢献者合意を初日に決める(§21)。

## 2. 既存プロジェクトの調査

| 対象 | 構造の特徴 | 学ぶ点 | 避ける点 |
| --- | --- | --- | --- |
| QEMU (C) | QOM(型階層 + プロパティ)でデバイス生成と配線を行う【既知】。`hw/` は機能別(i2c, sensor, misc…)とアーキ別(arm…)が併存し、ボードと SoC が同じ階層に置かれやすい【既知】。Rust 対応は実験段階で、PL011 と HPET が `rust/hw/` 下の参照クレート【確認済】。異種システムのため複数ターゲットを 1 バイナリにする single-binary 作業が進行中【確認済】 | 型 + プロパティで配線を外部化する発想。「1 ターゲット = 1 バイナリ」が異種システムの障壁になるという教訓 | `misc` のような受け皿の肥大。ボード/SoC/IP の同居 |
| Renode (C#) | 宣言的な `.repl`。ペリフェラルを「型・名前・接続先(sysbus/I2C/SPI/GPIO)・パラメータ」で記述し、`using` で取り込み、`->` で IRQ/GPIO を接続【確認済】。CPU を指定した専用メモリで非共有アドレス空間を記述でき、複数マシンも扱える【確認済】 | 宣言的記述 + include、接続演算子、CPU 別アドレス空間、複数マシン | 実装が機能別の名前空間(UART, I2C…)に並ぶ【既知】。多軸の分類を機能 1 軸に押し込む |
| MAME (C++) | チップ(デバイス)とマシン(ドライバ)が分離。`machine_config` はタグ付きデバイス木で、device\_add / replace / remove ができる。slot device はオプション名からデバイス型を選んで差し替える【確認済】 | デバイス木 + タグ、差し替え可能なスロット(カートリッジ/カード/拡張) | マシン定義が製造者・年代別に巨大化【既知】 |
| Simics | 物理構造を反映する階層コンポーネント(階層名前空間 + Python 構成)【確認済】。レジスタバンク中心の DML【確認済】。モジュール IF は C ABI で言語非依存・安定【確認済】。複数ターゲットを同一プロセスで並列実行でき、SystemC も取り込める【確認済】 | 階層構成、レジスタ記述 DSL、安定 ABI、異種モデルの統合 | 商用で内部非公開。思想のみ参照 |
| rust-vmm | 小さなクレート群(vm-memory, vm-device, vm-fdt, vm-superio, event-manager など)を組み合わせる思想【確認済】。vm-superio は状態構造体とシリアライズ版を別クレートに分離【確認済】 | trait で提供者と消費者を分離、状態の分離、クレートを小さく保つ | KVM 向けで、異種多 CPU・精密タイミング・細粒度 MMIO は想定外 |
| Unicorn | Rust クレート `unicorn-engine` は 2.1.x、ライセンスは GPL-2.0、`arch_*` features で ISA を選択、`dynamic_linkage` あり【確認済】。ISA は ARM / ARM64 / M68K / MIPS / PPC / RISCV / S390X / SPARC / TriCore / X86、設計上スレッドセーフ、MMIO と uc\_ctl(CPU モデル・TB キャッシュ制御)あり【確認済】 | 初期エンジンとして十分。API の形が「自前エンジンの最小要件」の叩き台になる | Xtensa・Z80・AVR などはリストに無い【確認済】ので ISA 追加は自前エンジン前提。GPL-2.0 |
| Linux DT / Zephyr / CMSIS-SVD | ノード + `compatible` 文字列でバインド、include / overlay。SVD は多数 MCU のレジスタ記述資産【既知】 | `compatible` 的な識別、overlay、SVD からのレジスタ定義の生成・取込 | — |

**共通する教訓**: (a) 成功している系は「デバイス」と「マシン(配線)」を分けている。(b) 失敗しやすい系は、機能別ディレクトリに種別・出自・バス役割が溶け込む。(c) 異種・多機械・協調シミュレーションは後付けが難しく、最初から IR(中間表現)とアドレス空間モデルに織り込む必要がある。

## 3. 「混ざる」の正体

ディレクトリ木は 1 つの軸でしか分類できないのに、モデル 1 個は少なくとも 8 個の属性(§5)を持つ。既存の構成は、この多軸を暗黙に 1 つの木へ詰め込むので混ざる。混ざり方は次の 10 種類に整理でき、それぞれに構造的な対策を割り当てる。

| # | 混ざり方 | 例 | 対策(章) |
| --- | --- | --- | --- |
| M1 | 種別の混在 | SoC・ボード・IP が同じ階層 | 種別を最上位の軸にする(§6) |
| M2 | 契約と実装の混在 | i2c のプロトコル定義とコントローラ実装が同居 | contract と model を別クレート(§7, §11) |
| M3 | 振る舞いと配線の混在 | モデルにベースアドレスや IRQ 番号が直書き | 配線は soc/board の宣言へ(§14) |
| M4 | 多重所属 | BME280 は sensor でもあり i2c でも spi でもある | 機能はタグ。ディレクトリは出自(§5, §11) |
| M5 | モデル内部の混在 | レジスタ・状態機械・バス適合・ホスト I/O が 1 ファイル | logic / bind / state(§10) |
| M6 | SoC 固有の癖の漏出 | 汎用 IP の中に SoC 名の分岐 | パラメータ・variant・glue、SoC 名禁止 lint(§10) |
| M7 | 忠実度の混在 | 機能モデルとサイクル精度モデルが 1 つに | 忠実度を manifest に宣言し別実装(§17) |
| M8 | ホスト依存の混入 | モデル内で実時間・乱数・ファイルを直接使う | ホスト境界を ctx と host 層に限定(§13) |
| M9 | 受け皿ディレクトリ | misc, common, utils | 禁止語 lint と命名規則(§9) |
| M10 | 言語境界の混在 | C/C++ の FFI があちこちに散在 | bridge と ffi に隔離(§17) |

## 4. 設計原則

- **P1 軸は 1 つだけ木にする**: ディレクトリは「種別 → 出自」。他はタグと生成ビュー。
- **P2 同一性は実世界の名前で決める**: `pl011` や `bme280` のように、用途名ではなく規格名・型番で呼ぶ。
- **P3 契約と実装を分ける**: 契約は trait と型だけ。実装(バス実体、コントローラ、デバイス)は別クレート。
- **P4 振る舞いと配線を分ける**: モデルは自分の外を知らない。配線は宣言(IR)にだけ存在する。
- **P5 モデル同士は直接参照しない**: すべて ctx(後述)と contract のポートを介する。デバイス間の `&mut` 参照を持たない。
- **P6 依存は一方向で、機械が検査する**: 人のレビューに頼らない。
- **P7 プロセッサも普通のノード**: CPU 専用の特別扱いを作らない。
- **P8 ホスト依存は境界へ押し出す**: 時間・乱数・I/O はコアのサービス経由。これが決定論とスナップショットの前提。
- **P9 状態は宣言的・バージョン付き**: 全モデルの状態をスナップショット可能な構造体にまとめる。
- **P10 横断関心はタップ点で**: トレース・デバッグ・故障注入をモデル内部に埋め込まない。
- **P11 拡張は pack で**: first-party も third-party も同じレイアウト。
- **P12 再利用は 2 回目で昇格(Rule of two)**: 先回りして汎用化しない。最初は SoC 内の `local/` に置き、2 つ目の利用者が現れたら ip へ昇格する。
- **P13 追加作業は定型化する**: `xtask new …` で雛形を生成し、配置に迷わせない。

## 5. 軸と語彙

### 5.1 属性の軸(木に載せるのは A1・A2 のみ)

| 軸 | 内容 | 表現場所 |
| --- | --- | --- |
| A1 領域 (domain) | util / exec / contract / runtime / isa / accel / hw / machine / block / describe / assemble / front | **ディレクトリ(第 1 階層)** |
| A2 機能 (function) または アーキ (arch) | デバイスは機能(char, intc, timer, …)、機械はアーキ(arm, i386, …)。語彙は QEMU `hw/` に同じ | **ディレクトリ(第 2 階層)** |
| A2' 出自 (origin) | 仕様や製品の定義者。ベンダー名、標準化団体 | **識別子の名前空間と manifest のみ**(`ip:arm/pl011`、`origins.toml`)。ディレクトリには現れない(QEMU が `imx_serial.c` を `hw/char/` に置くのと同じ) |
| A3 機能 | uart, i2c-controller, temperature-sensor, display など | manifest のタグ + 生成ビュー |
| A4 バス上の役割 | initiator / target / monitor / bridge / mux | manifest の ports |
| A5 忠実度 | functional / timing-approx / cycle / rtl-cosim | manifest |
| A6 実装言語/実行形態 | rust / c-abi / rtl / wasm / out-of-process | manifest + クレート種別 |
| A7 検証水準 | experimental / spec-tested / trace-validated / silicon-validated | manifest |
| A8 リビジョン/変種 | シリコンステッピング、IP バージョン、パッケージ | `@variant` と manifest |

### 5.2 エンティティの定義

| 語 | 定義 | 置き場所 |
| --- | --- | --- |
| Spec | 規格・ISA・プロトコルの定義。実装は含まない | `isa`, `contract` |
| IP | チップ内蔵の再利用可能な機能ブロック(レジスタを持つ)。SoC 非依存 | `catalog/ip` |
| Part | チップ外の独立した部品(センサ、DRAM、Flash、PMIC など) | `catalog/parts` |
| Cpu | 1 つのプロセッサコアのモデル(ISA + エンジン設定 + コア固有レジスタ/統合ブロック) | `catalog/cpu` |
| Subsys | 複数の IP/CPU を束ねた再利用可能な部分系(CPU クラスタ、サブシステム、チップレット) | `catalog/subsys` |
| SoC | CPU・IP・サブシステムを、アドレスマップ・IRQ・クロック・ピン多重化で結合した 1 チップ | `catalog/soc` |
| Module | 基板上で着脱される単位(SoM、シールド、PMOD、拡張カード、カートリッジ) | `catalog/module` |
| Board | SoC + Part + Module + 配線(ネット) + 環境 | `catalog/board` |
| World | 複数のボード/機械 + リンク + 物理環境 + シナリオ | `catalog/world` |
| Instance | 実行時に生成されたもの。名前は SoC/Board 記述にだけ現れる(`uart0` など) | 実行時のみ |

**同一性の表記**: すべてのエンティティは `種別:出自/名前[@変種]`(例 `ip:arm/pl011@r1p5`)という識別文字列を持ち、manifest・記述ファイル・レジストリ・ドキュメント・スナップショットヘッダで同じ表記を使う。出自の正規名は `origins.toml` に集約し、`st` / `stmicro` のような表記揺れを lint で防ぐ。

## 6. 全体構成(ディレクトリ)

ADR 0024 による QEMU 型の構成。旧案(framework/catalog の 2 分割)からの
対応は ADR 0024 §3.1 の表を参照。

```
<proj>/
├─ docs/                  # adr/ architecture/ guides/
├─ util/                  # L0 純粋部品(bits, fifo, fnv)。QEMU util/
├─ exec/                  # L1 共通語彙: Addr, VTime, Attr, RegId, BusFault, Snapshot
├─ contract/              # L2 全ポート契約を 1 クレート 12 モジュールに
│                         #    (mmio, signal, clock, cpu_engine, gpio, i2c, spi,
│                         #     uart_line, can, net, stream, model)
├─ runtime/               # L3 スケジューラ, アドレス空間(Region木→FlatView),
│                         #    イベント, clock tree, snapshot 機構, ctx 実装, Machine
├─ isa/                   # L2 ISA 語彙 + CPU モデル(QEMU target/。cargo が
│  ├─ arm/                #    target/ を予約しているため isa/ と呼ぶ)
│  ├─ aarch64/            #    arm: regs + cpu/{arm926ej-s, cortex-m3}
│  ├─ x86/                #    i386 と x86_64 を 1 クレートに
│  └─ riscv/              #    regs + cpu/e31
├─ accel/                 # L3 CPU 実行エンジン(QEMU accel/)
│  ├─ unicorn/            #    初期エンジン(GPL-2.0, 第21章)。FFI の unsafe はここだけ
│  ├─ armv7m/             #    自前インタプリタ(ADR 0017)
│  └─ conformance/        #    全エンジンに同一テストを流す枠
├─ hw/                    # L3 デバイスモデル。機能別 1 ディレクトリ(QEMU hw/<function>/)
│  └─ src/                #    char(pl011, usart-f1, pair) / intc(pl190, pic-8259,
│                         #    ioapic, gic-v2, scs) / timer(sp804, pit-8254) / rtc /
│                         #    gpio(pl061, gpio-f1, net, pinmux) / ssi(pl022, bus) /
│                         #    i2c(bus, dw-apb-i2c, tca9548a) / can(bus) /
│                         #    sensor(bme280/, lm75a) / misc(primecell-id, rcc-f1) /
│                         #    mailbox(mhu) / nvram(fw-cfg) / pci / ide / virtio
│                         #    バス実体(hw/i2c/bus.rs)は QEMU hw/i2c/core.c と同じ位置
├─ machine/               # L6 機械モデル。アーキ別(QEMU hw/<arch>/)
│  └─ src/                #    arm/stm32f103/ i386/pc/(machine.rs, linux_boot.rs)
│                         #    機械レジストリ(-M)とアーキ共通ブート経路もここ
├─ block/                 # L2 ゲストイメージ形式(iso9660、将来 raw/qcow2)。QEMU block/
├─ describe/              # L4 記述スキーマ, パーサ, MachineGraph(IR), 検証
├─ assemble/              # L5 Registry, IR→実行時インスタンス生成, engine 結合
├─ manifests/             # manifest データ木。コードと同じ形にミラー
│  ├─ origins.toml        #    出自の正規名一覧
│  ├─ isa/…  hw/…        #    CPU・IP・部品の manifest(機能ミラー)
│  └─ machine/…           #    soc.toml / board.toml(機械ミラー)
├─ firmware/              # ブートROM等の記述(ハッシュ・入手方法)。QEMU pc-bios/
├─ host/ ops/ bridge/     # L5 将来層のプレースホルダ(chardev/ui/monitor 等)
├─ mosaic/                # 実行入り口は 1 本(bin `mosaic`)。QEMU system/vl.c 相当
├─ tests/                 # 機械レベル(実イメージを起動し出力を比較)
├─ xtask/                 # lint-layers, lint-names, lint-catalog(manifest ミラー含む)
├─ ffi/                   # 自前で抱える *-sys クレート
└─ third_party/
```

**領域で分ける理由**: 変更頻度と所有者が領域ごとに違う点は旧案と同じ。
さらに (a) `hw` は `machine` を決して依存できず、逆方向のみ許される、
(b) 機能が既存ディレクトリに溶け込まない(1 モデル 1 ファイル)、
(c) モデル追加が「mod.rs かレジストリに 1 行」で終わる、の 3 点が
クレート境界で機械的に強制される。外部 pack(第16章)は `hw`/`machine`
と同じ形のディレクトリを別リポジトリに置く形に読み替える。

**深さの固定**: hw は `機能/モデル` の 2 階層(+多バスモデルのディレクトリ)、
machine は `アーキ/機械`、isa は `アーキ`(+`cpu/`)。深さが固定なので、
ツールとレビューが場所を機械的に判定できる。

## 7. 層の責務

### 7.1 framework

| 層 | 責務 | 置いてよいもの | 置いてはいけないもの |
| --- | --- | --- | --- |
| L0 prim | VM 概念を持たない純粋部品 | bit 操作, endian, crc, 固定長コンテナ(no\_std) | Addr や時間などの VM 語彙, I/O |
| L1 core/types | 全層共通の語彙 | Addr, VTime, Attr, BusFault, Snapshot trait, エラー型 | ロジック, スケジューラ |
| L2 contract | モデル間・層間の約束 | trait, メッセージ型, 能力(capability)定義, conformance テスト | 実装, 状態を持つ型 |
| L2 modelkit | モデル作者の道具 | regbank, fifo, timer, エッジ検出, 状態機械ヘルパ | 特定の IP/SoC/ベンダーの知識 |
| L2 isa | ISA の仕様データ | レジスタ ID, 例外種別, システムレジスタ表 | 実行ロジック |
| L3 core/runtime | 実行時サービス | スケジューラ, アドレス空間, イベントキュー, snapshot 機構, RNG, log, ctx 実装 | モデル固有ロジック, ホスト I/O |
| L3 interconnect | 接続の実体 | i2c-bus, axi-xbar, apb-bridge, can-bus, pci-bus | レジスタを持つ周辺 |
| L3 engine | CPU 実行の実装 | Unicorn アダプタ, 自前インタプリタ/JIT, KVM, remote | 周辺デバイス, SoC の知識 |
| L4 describe | 記述から IR | スキーマ, パーサ, overlay 解決, 検証, IR | 実行, catalog への依存 |
| L5 assemble | IR からインスタンス | Registry, 生成, FDT/ACPI 出力 | catalog 実装への直接依存 |
| L5 host | ホストとの接点 | display, audio, tty, tap, file, USB パススルー | モデル内部 |
| L5 ops | 観測と制御 | trace, gdbstub, replay, fault injection, coverage | モデルの中に埋め込む処理 |
| L5 bridge | 外部世界との橋 | Verilator, SystemC, FMI, remote-port, C-ABI plugin, WASM | catalog モデルそのもの |

### 7.2 catalog

| 種別 | 中身 | コードの割合 |
| --- | --- | --- |
| cpu | コア 1 つのモデル(ISA + エンジン設定 + コア固有レジスタ/統合ブロック) | コード |
| ip | チップ内蔵の再利用ブロック。SoC 非依存 | コード |
| parts | 外部部品。バス上のターゲット、メモリ、PMIC など | コード |
| subsys | cpu と ip の束(クラスタ、サブシステム、チップレット) | 宣言が主 + 任意の glue |
| soc | subsys と ip を結ぶ 1 チップ | 宣言が主 + 任意の glue |
| module | 着脱単位(SoM, シールド, PMOD, カートリッジ) | 宣言が主 |
| board | soc と parts と module の配線、環境 | 宣言が主 |
| world | 複数ボード、リンク、物理環境、シナリオ | 宣言が主 |
| firmware | ブートROM等の入手方法とハッシュ | データのみ |

## 8. 依存規則

```
mosaic(front) ─► machine ─► hw ─► contract ─► exec ─► util
     │             │   │       └► (accel 経由で isa)
     │             │   └► accel ─► isa
     │             └► runtime ─► contract
     ├─► block
tests ─► mosaic + block
assemble ─► runtime, accel, describe, contract, exec
describe(依存なし)   isa ─► contract(※), exec
```

- **R1** 依存は下位層へのみ(レベルは util 0 < exec 1 < contract 2 =
  isa 2 < block 2 < runtime = accel = hw 3 < describe 4 < assemble 5 <
  machine 6 < front = tests 7 < tooling 8)。
- **R2** `hw`(デバイス)は `machine`・`accel`・`runtime` を依存禁止。
  機械がデバイスを載せる方向のみ許される(ADR 0024 の 2 軸境界)。
  ※ isa → contract は CPU モデルが CpuDescriptor を読むための唯一の
  同位依存(QEMU の target/ が cpu.h を含むのと同じ)。
- **R3** `hw` 内の横依禁止は「機能ディレクトリをまたぐ依存」で判定
  (char から intc へ等)。共有ロジックは util か概念モジュール
  (misc/primecell_id)へ切り出す。
- **R5** 機械(machine)は任意の下位(hw, accel, isa, runtime, block,
  describe, assemble)を依存してよい。QEMU の `hw/<arch>` が cpu コアと
  デバイスを直接使うのと同じ到達範囲。フロントエンド(`mosaic`)は
  `machine` + `block` のみで機械コードを持たない。
- **R6** `unsafe` は accel 内の限定モジュールだけで許し、それ以外は
  `#![forbid(unsafe_code)]`(workspace lints)。
- **R7** 外部クレートは層ごとの許可リストで管理する(unicorn-engine は
  accel/unicorn のみ、toml は machine/tests/tooling のみ)。
- **R8** contract の破壊的変更は ADR 必須。contract は最も変更コストが
  高い層なので、小さく保ち、`#[non_exhaustive]` と能力交渉で後方互換を保つ。

**強制の手段**(ルールは CI に置く):

1. `xtask lint-layers` が `cargo metadata` と各クレートの `[package.metadata.layer]` を読み、R1〜R3・R5 の違反でビルドを落とす。
2. cargo-deny の `bans`(特定クレートを使えるクレートの限定 = `wrappers`)で外部クレートの持ち込みを層別に制限する【要検証: 設定の表現力を実機で確認】。
3. `clippy.toml` の `disallowed-methods` / `disallowed-types` で、catalog に `SystemTime::now`、`thread::sleep`、乱数、`std::fs` などを禁止する。catalog 配下に置く設定ファイルがクレートに効くかは要確認【要検証】。
4. `#![forbid(unsafe_code)]` をテンプレートに焼き込む。
5. CODEOWNERS で framework/contract と framework/core の変更に必須レビュアーを付ける。
6. `xtask lint-names` が禁止語(第9章)と `origins.toml` 外の出自名を検出する。

## 9. 命名規則

- **N1 実世界の名前で呼ぶ**: 型番・規格名・公式の IP 名(`pl011`, `bme280`, `dw-apb-i2c`)。用途名(`uart`, `temp-sensor`)や自作の愛称は使わない。用途は manifest のタグが担う。
- **N2 禁止語**: `common` `utils` `generic` `base` `helper` `manager` `handler` `impl` `shared` `new` `old` `stuff` をディレクトリ名・クレート名に使わない(`xtask lint-names` で検出)。受け皿が必要に見えたら、それは概念の切り出しが足りないサイン。**例外**: `misc` は QEMU の `hw/misc` と同じく `hw/src/misc`(とその manifest ミラー)に限り許可(ADR 0024)。
- **N3 表記**: kebab-case。出自は `origins.toml` の正規名のみ(例 `arm`, `st`, `nxp`, `ti`, `bosch`, `synopsys`, `riscv`)。
- **N4 クレート名**: ディレクトリ階層をそのまま写す(`<proj>-hw`, `<proj>-machine`, `<proj>-accel-unicorn`, `<proj>-isa-arm`, …)。入り口バイナリは `<proj>` のみ。
- **N5 変種は公式の呼称**: `@r1p5`(ARM の版表記)、`@v2`(ベンダーの IP 版)。SoC 名を変種名にしない。
- **N6 インスタンス名は soc/board 記述の中だけ**: `uart0`、`i2c1`、`env0`。
- **N7 共有ロジックは概念名の独立クレート**: 例 `primecell-id`。manifest は `kind = 'ip-lib'`。
- **N8 `local/`**: 昇格待ちの SoC 固有ブロック専用(第4章 P12)。名前は `<chip>-<block>`。
- **N9 ボード改訂**: `rev-a` `rev-b` のように基板の表記に合わせる。
- **N10 テスト/フィクスチャ**: 目的でなく対象で命名する(`pl011-fifo-overrun` など)。
- **N11 コード内の用語**: レジスタ名・ビット名はデータシート/TRM の名前をそのまま使う(検索性と仕様照合のため)。

| 避ける名前 | 採る名前 | 理由 |
| --- | --- | --- |
| `ip/uart/` | `ip/arm/pl011/` | 機能は分類軸でなく同一性の軸で呼ぶ |
| `parts/sensors/temp/` | `parts/bosch/bme280/` | 用途分類は検索ビューで生成 |
| `soc/generic-cortexm/` | `soc/<origin>/<family>/<chip>/` | 実在するチップ名で呼ぶ |
| `common/uart_base.rs` | `modelkit/fifo` か `ip-lib` | 受け皿を作らず概念名にする |
| `board/my_custom/` | 外部 pack の `board/<自社>/<基板名>/` | カスタムは pack(第16章) |

## 10. モデルの解剖

「i2c の下のモデル同士が混ざる」問題は、ディレクトリ分類だけでは解けない。**モデル 1 個の内部**を層に分け、外との接点を明示してはじめて混ざらなくなる。

```
catalog/ip/arm/pl011/
├─ Cargo.toml
├─ ip.toml            # manifest: 識別, ports, params, タグ, 出自と根拠
├─ regs/pl011.toml    # レジスタ定義の唯一の情報源(SVD / SystemRDL からの取込も可)
├─ src/
│  ├─ lib.rs          # 公開面は register(&mut Registry) と Params と State だけ
│  ├─ logic/          # 純粋な状態機械。contract に依存しない(no_std 可)
│  ├─ regs_gen.rs     # regs/ から xtask が生成(コミットして差分レビュー可能にする)
│  ├─ bind/           # contract への薄い適合層: mmio.rs irq.rs clk.rs stream.rs dma.rs
│  ├─ state.rs        # スナップショット対象。version 付き + migrate
│  └─ variants.rs     # 版差(r1p5 など)。SoC 名は入れない
├─ tests/             # レジスタスクリプト, golden-trace, fuzz, snapshot 往復
└─ docs/              # 仕様の章・表番号 → テスト名の対応表, 既知の未実装
```

**ルール**

1. `logic/` は contract・ctx・ホストを import しない。これで単体テスト、ファジング、RTL との突き合わせ、他フレームワークへの流用ができる。
2. `bind/` は変換だけを行う薄い層にし、分岐ロジックを入れない。
3. 状態は `state.rs` に集約する。ホストのハンドル(ファイル・ソケット)は状態に入れず、復元時に host ポートから再接続する。
4. **1 チップが複数のバスを話せる場合**(BME280 は I2C と SPI): logic は 1 つ、bind を I2C 用と SPI 用に分け、Cargo feature `bind-i2c` / `bind-spi` で選ぶ。ports には排他(`one-of`)を宣言する。ディレクトリは 1 つのまま、機能の多重所属はタグで表す。
5. **SoC 固有の癖**は、(1) パラメータ化(FIFO 深さ、有無など)、(2) 公式の版名の variant、(3) それでも足りなければ soc 側の glue で IP を包む、の順に処理する。IP クレートに SoC 名を書かない(lint)。
6. 未知オフセット・予約ビット・禁止アクセスの扱い(RAZ/WI、バスエラー、警告ログ)は Params のポリシーで選べるようにし、ゲスト障害の再現性を確保する。
7. 時間・乱数・ファイル・ネットワークは ctx 経由のみ(直接使用を lint で禁止)。
8. ゲストが何を書いても panic しない(`unwrap` と添字アクセスを lint で禁止)。
9. 公開面は register 関数・Params・State 型だけにする。

**ctx 渡しの原則(P5 の実現手段)**: モデルのコールバックは `&mut self` と `&mut Ctx` を受け取る。Ctx が提供するのは、現在の仮想時間、イベントの登録/取消、シグナル操作(IRQ の上げ下げ)、DMA(マスタ経由のアクセス)、ログ/トレース、RNG、ホストポートの 7 つだけ。モデルは他モデルへの参照を持たない。これで、Rust の所有権(二重可変借用)、コールバックの再入、決定論、観測点(トレース)の 4 つが 1 か所で解ける。DMA が別デバイスの MMIO に届く場合の再入制約は設計の細部として詰める【要検証】。

```rust
pub trait MmioTarget {
    fn read(&mut self, ctx: &mut Ctx, off: u64, size: u8, attr: &Attr) -> Result<u64, BusFault>;
    fn write(&mut self, ctx: &mut Ctx, off: u64, size: u8, val: u64, attr: &Attr) -> Result<(), BusFault>;
}
```

**ip.toml の例(スケッチ。値は例示)**

```toml
[identity]
id       = 'ip:arm/pl011'
summary  = 'ARM PrimeCell UART'
spec     = ['ARM DDI 0183']        # 出自(クリーンルームの根拠)
variants = ['r1p5']

[tags]                             # ディレクトリに現れない分類はすべてここ
function = ['uart']
fidelity = 'functional'            # functional | timing-approx | cycle | rtl-cosim
verified = 'spec-tested'           # experimental | spec-tested | trace-validated | silicon-validated

[ports]
mmio   = { role = 'target', size = 0x1000 }
irq    = { role = 'source', pins = ['combined', 'rx', 'tx', 'rt', 'ms', 'err'] }
clk    = { role = 'sink', names = ['uartclk', 'pclk'] }
reset  = { role = 'sink' }
stream = { contract = 'stream/byte', role = 'endpoint' }
dma    = { contract = 'bus/master', optional = true }

[params]
fifo_depth = { type = 'u8', default = 16 }
unmapped   = { type = 'policy', default = 'raz-wi' }
```

## 11. 具体例: i2c をどう分けるか

「i2c」で括ると、プロトコル、バス実体、コントローラ、センサ、マルチプレクサ、ホスト接続、解析ツールが 1 か所に集まって混ざる。**役割ごとに居場所を決める**と次のようになる。

| 構成要素 | 場所 | 中身 |
| --- | --- | --- |
| プロトコル契約 | `framework/contract/serial/i2c` | Initiator / Target の trait、伝送単位(Start・アドレス+R/W・Byte・Ack・Stop・Repeated Start)、クロックストレッチの表現、速度モード、7/10bit アドレス。SMBus 拡張は別 trait |
| バスの実体 | `framework/interconnect/i2c-bus` | アドレス解決、複数 Initiator の調停、衝突/NACK、mux/bridge の取り扱い、ピンレベルとトランザクションレベルの変換 |
| コントローラ | `catalog/ip/synopsys/dw-apb-i2c` | Initiator ロールのレジスタモデル。他社のコントローラも同じ作法で並ぶ |
| ターゲット部品 | `catalog/parts/bosch/bme280`、`parts/microchip/24lc256` | Target ロールのモデル。複数バス対応なら bind を feature で選ぶ |
| mux 部品 | `catalog/parts/ti/tca9548a` | 部品であり、同時にバスのブリッジ。ports に `role = bridge` を宣言し、実体は interconnect の部品を使う |
| bit-bang | 契約は `contract/signal(net)`、変換は `interconnect/i2c-bus` | GPIO IP が SDA/SCL を駆動し、バス実体がエッジを復号してトランザクションに戻す。同じ Target 部品が、コントローラ経由でも bit-bang でも動く |
| 実機パススルー | `framework/host/i2c-passthrough` | ホストの I2C デバイスや USB-I2C アダプタへ転送する Target 側アダプタ |
| 解析 | `framework/ops/trace` | タップ点から i2c をデコードして表示。モデルには埋め込まない |
| 検査 | `contract/serial/i2c` の conformance、各モデルの tests | 全 Initiator/Target 実装が同じ適合テストを通る |

ポイントは 3 つ。(1) **同じ BME280 が** `function = ['environmental-sensor']` のタグを持ち、`i2c.target` と `spi.target` の ports を持つ。ディレクトリでは分けず、検索ビューで「i2c の Target 一覧」「センサ一覧」を生成する。(2) **ピンレベルとトランザクションレベルの両方**を最初から契約に含める。ソフトウェアの bit-bang を動かすにはピンレベルが必須で、後付けは難しい。(3) **mux は部品でもバスでもある**ため、manifest の ports の役割で表す。

**同じ手法を他の領域にも適用する**

| 領域 | 契約 | 接続の実体 | チップ内蔵(ip) | 外部部品(parts) | ホスト側(host) |
| --- | --- | --- | --- | --- | --- |
| GPIO / ピン | signal/gpio-pin, net | net 解決(プル抵抗, オープンドレイン) | GPIO コントローラ, pinmux | LED, ボタン | GPIO パススルー |
| SPI | serial/spi | spi-bus(CS 管理) | SPI コントローラ | Flash, ADC, ディスプレイ | spidev |
| UART | serial/uart-line, stream/byte | 直結(ペア) | UART | モデム, GPS | tty, pty, ソケット |
| CAN | serial/can | can-bus(調停) | CAN コントローラ | トランシーバ, ECU | SocketCAN |
| PCIe / USB | bus + 専用契約 | pci-bus / usb-hub | ルートコンプレックス, ホストコントローラ | エンドポイント, 各種クラスのデバイス | USB パススルー |
| Ethernet | stream/frame, serial/mdio | スイッチモデル | MAC | PHY | tap, slirp 相当 |
| ストレージ | block, serial/sdio | — | SD/MMC コントローラ | SD カード, eMMC, NAND | ファイル, 実デバイス |
| ディスプレイ/入力 | host の display, input | — | LCDC, タッチコントローラ | パネル | SDL, ウィンドウ |
| ADC/アナログ | signal/analog | — | ADC | センサのアナログ出力 | world の環境モデル |

## 12. プロセッサ系: isa / engine / cpu / subsys

### 12.1 4 つの層を分ける

| 層 | 問い | 例 |
| --- | --- | --- |
| `isa` | この命令セットの語彙は何か(実行はしない) | レジスタ ID、例外種別、システムレジスタ名表 |
| `engine` | 命令を実行して止まる仕組みは何か | unicorn、native(ISA ごと)、kvm、remote |
| `cpu`(catalog) | コア 1 つとして何が付いているか | isa + エンジン設定(CPU モデル名・拡張・実装依存値)+ コア固有レジスタ/統合ブロック(NVIC、CP15、MPU、デバッグ)+ IRQ/FIQ/NMI 入力 + リセット |
| `subsys`(catalog) | コアの集まりとして何が共有されるか | クラスタ(コア × N + 共有 L2・割り込みコントローラ・タイマ) |

Unicorn の ISA 一覧に Xtensa・Z80・AVR などは無い【確認済】ため、将来 ISA を足す道は「自前エンジンを ISA ごとに足す」になる。`engine/native/` を ISA 別クレート + 共通部(`native-kit`: デコーダ生成、TB キャッシュ、softmmu 相当)の形にしておくと、Unicorn からの置換も ISA の追加も同じ作業になる。

### 12.2 エンジン契約のスケッチ

```rust
pub trait CpuEngine {
    fn caps(&self) -> EngineCaps;                       // 能力の宣言(命令フック, icount, 排他モニタ, 精密例外 ...)
    fn run(&mut self, budget: Budget) -> StopReason;    // 命令数 or 仮想時間で予算を与える
    fn request_stop(&self, why: StopWhy);               // フックや他スレッドから停止要求
    fn reg_read(&self, r: RegId) -> Result<u128, EngErr>;
    fn reg_write(&mut self, r: RegId, v: u128) -> Result<(), EngErr>;
    fn set_line(&mut self, line: CpuLine, level: bool); // irq / fiq / nmi / 割り込みベクタ
    fn bind_view(&mut self, view: &dyn AddressView);    // FlatView 相当(RAM はホストポインタ, MMIO はコールバック)
    fn invalidate(&mut self, r: core::ops::Range<Addr>, why: Inval); // TB / TLB の無効化
    fn save(&self) -> EngineState;
    fn load(&mut self, s: &EngineState);
    fn instrument(&mut self, spec: Instrumentation);    // 任意能力: 命令/ブロック/メモリのフック
}
// StopReason: BudgetExhausted | Wfi | Halted | Breakpoint | Exception(kind)
//           | UnmappedAccess(info) | SemihostCall | CapabilityMissing
```

これは叩き台。Unicorn の API(レジスタ読み書き、メモリマップ、`emu_start` の終了条件・命令数・時間制限、フック、コンテキスト保存、`uc_ctl`)が、そのまま自前エンジンの最小要件になる【確認済】。**エンジン固有の型(Unicorn の列挙値など)は契約へ漏らさず**、isa の RegId に写像する。能力が足りない場合は `EngineCaps` で宣言し、上位が機能を絞る(例: icount 非対応なら決定論モードを無効化)。

### 12.3 Unicorn を使う間のルール

1. **1 コア = 1 インスタンス**を前提にする。SMP を内蔵する機構ではないという理解のもと、共有 RAM は同一のホストメモリを各インスタンスにマップして実現する【要検証: 複数インスタンス間でのマップ共有、無効化の同期】。
2. MMIO はエンジンのフックから `AddressView` へ転送し、core/runtime が唯一の正本として解決する。エンジン側のメモリマップは FlatView から機械的に生成する派生物にする。
3. **フック中にそのエンジンへ再入しない**。デバイスが IRQ を上げる、他コアを起こすなどの副作用は、すべてイベントキューへ積み、フック復帰後に処理する(Unicorn は入れ子の操作に弱い面がある【要検証】)。
4. CPU モデルの選択と TB キャッシュ制御には `uc_ctl` 相当を使う【確認済】。
5. 排他アクセス(LDREX/STREX 相当)をインスタンスまたぎで厳密に扱えない場合は、`EngineCaps` に `exclusive_monitor = false` と宣言し、SMP は best-effort と明記する【要検証】。
6. `arch_*` features で ISA を絞り、ビルド時間とサイズを抑える【確認済】。
7. ライセンス(GPL-2.0)の扱いは第21章。

### 12.4 マルチコア・異種のスケジューリング

- **量子(quantum)方式**: スケジューラが各エンジンに「min(量子, 次のイベントまでの時間)」の予算を順番に与える。量子の長さは精度と速度の調整つまみで、既定値は実測で決める。
- **アクセス時同期(sync-on-access)**: 時間に影響する状態を持つデバイスへの MMIO では、アクセス前にそのコアの局所時間までグローバル時間を進める。
- **決定論**: コアの実行順は固定(ラウンドロビン + 固定順序)。同じ入力列は同じ結果を生む。
- **コア間の連携**(共有 RAM、メールボックス、IPI、排他モニタ)は core/runtime の責務。エンジンは自コアのことだけを知る。
- **並列モード(将来)**: コアごとのスレッドで実行する。契約の型は `Send` に保つが `Sync` は要求しない。最初から並列にはしない。

### 12.5 CPU 構成のバリエーション

| 構成 | 扱い方 |
| --- | --- |
| 同種 SMP(N コア) | subsys のクラスタ。コアは同じ cpu を N 個、共有部は ip |
| AMP / 異種(A + M + DSP など) | SoC のグラフに ISA が異なる cpu ノードを並べる。エンジンはノードごとに別々 |
| 異 ISA の切替(起動時にコアを選ぶタイプ) | cpu ノードを `variants` で択一にし、選択を soc のパラメータにする |
| 仮想化拡張(EL2 / ハイパーバイザ拡張) | cpu の能力と isa の例外モデルで表現。ゲストの OS ごと動かす場合も、エンジンは特権レベルを区別しない |
| セキュリティ状態(TrustZone 等) | Attr の security を全バス経路に通す。ビューを secure / non-secure で分ける |
| ハードウェアスレッド(SMT) | 論理コア = ノード 1 つとして扱う |
| FPGA 内のソフトコア | subsys に cpu ノードを置き、周辺は bridge/rtl か ip で用意 |
| オフロードコア / 管理コア(remoteproc, PMU) | SoC の cpu ノード。メイン CPU がファームを書き込んで起動する流れは、通常のメモリ書き込みとリセット線で表現できる |
| DSP(8bit 非整合、ワードアドレス) | AddressSpace に `addr_unit_bits` を持たせる【要検証】。契約は常にバイト単位に正規化できない場合を想定 |
| レトロ機(複数 CPU + カスタムチップ、サイクル精度) | 量子を最小化する精密モード、cycle 忠実度の cpu、スロット(module)でカートリッジを差し替え |
| 実チップ(HIL) | engine/remote。バス上の MMIO を実機へ中継 |
| 同一 ISA の高速実行 | engine/kvm。デバイスは同じ contract のまま |

## 13. コアサービス

### 13.1 アドレス空間

- **Region 木**: container / alias / ram / rom / mmio / iommu を木に組み、優先度で重なりを解決し、(AddressSpace, 視点)ごとに**FlatView**へ平坦化してエンジンと DMA マスタへ渡す(QEMU の MemoryRegion → FlatView に相当する発想【既知】)。
- **視点(view)**: CPU ごと、DMA マスタごと、セキュリティ状態ごと、x86 の I/O 空間など。Renode が CPU 専用メモリを記述できる点【確認済】と同等以上を、最初から持つ。
- **Attr**: セキュリティ、特権、マスタ ID、アクセス種別(データ/命令/デバッグ/DMA)、排他、バイトイネーブル、エンディアンを全トランザクションに通す。
- **DMI**: RAM は直接メモリ経路(ホストポインタ)としてエンジンに渡し、MMIO だけをコールバックにする。
- **動的変更**: バンク切替・リマップ・ブートROMのオーバレイは Region の更新として行い、FlatView 再生成と TB/TLB の無効化を連動させる。
- **副作用なしのアクセス**: デバッガ用の読み出し(デバッグ属性)はレジスタの読み出し副作用を起こさない契約にする。
- **アドレス幅**: Addr は u64 を基本とし、アドレス単位が 8bit でない機種は例外扱いにする【要検証】。

### 13.2 時間とクロック

- **VTime**: 分解能は初日に決める。候補は ps の u64(約 213 日で一周)、より長い期間が必要なら u128(遅い)。
- **モード**: 決定論モード(実時間と切り離し、同じ入力は同じ結果)、実時間同期モード(ゲストの 1 秒をホストの 1 秒に合わせる)、最速モード(待ち時間を飛ばす)。
- **clock ツリー**: 周波数・分周・ゲーティングを `contract/clock` で表し、タイマはサイクルと時間を clock を通して換算する。クロックドメイン間の細かな挙動は機能モデルでは無視する。
- **icount**: 命令数ベースの時間は、エンジンの能力として宣言する。

### 13.3 信号・リセット・電源

- **irq**: レベル/エッジ、複数ソース、wired-OR、MSI(メッセージ型)、名前付きピン。
- **net**: ピン間の電気的接続。プルアップ/プルダウン、オープンドレイン、トライステート、衝突検出。
- **analog**: 電圧などの連続値。ADC への入力や環境モデルと接続する。
- **reset / power**: リセット種別(コールド/ウォーム/ウォッチドッグ)、電源ドメイン、保持(retention)、復帰要因。

### 13.4 Ctx と再入規則

1. モデルは他モデルへの参照を持たず、Ctx 経由でのみ外へ働きかける(第10章)。
2. エンジンのフック内ではそのエンジンを呼ばない。副作用はイベントキューへ。
3. 同じ入力列は同じ状態遷移を生む(時間・乱数・ホスト入力はすべてコアのサービスを通す)。

### 13.5 スナップショット

- **対象**: エンジンのコンテキスト、メモリ(ダーティページ追跡、差分保存)、VTime、イベントキュー、RNG、各モデルの State。
- **ヘッダ**: 識別文字列、State のスキーマ版、IR のハッシュ。構成が違えば復元を拒否する。
- **移行**: State は版を持ち、旧版からの migrate 関数をモデルが提供する。
- **ホスト接続**: ファイルやソケットは保存せず、復元時に host から再接続する。
- **記録再生**: 非決定的な入力(ホストの入力イベント、ネットワークパケット)は仮想時間付きのログとして保存し、再生する。

### 13.6 観測点(tap)

バス、IRQ、クロック、net、エンジンの命令/ブロックに観測点を定義し、ops(trace、coverage、fault injection)が購読する。無効時のコストは分岐 1 つ以下にする(購読者ゼロなら即戻る)。トレースコードをモデルに書かせない。

## 14. 記述(describe)と組立(assemble)

### 14.1 流れ

```
宣言(TOML) ─┐
             ├─► describe: 読込 → include/overlay 解決 → 検証 ─► MachineGraph(IR, 直列化可)
Rust builder ┘                                                     │
                                                                   ▼
                          assemble: Registry で識別子を引く → 生成 → 配線 → リセット → 実行
```

- IR は実行時の型に依存しない(core/types のみに依存)。可視化、差分、ハッシュ(スナップショットの互換判定)、他ツールからの利用を IR に集約する。
- 識別子の種別は単数形(`part:bosch/bme280`)、ディレクトリ名は `parts` と複数形。この対応は xtask が機械的に扱う。

### 14.2 IR の要素と検証

- **要素**: ノード(識別子、インスタンス名、params)、型付きポート(contract の識別子とロール)、エッジ、視点ごとのアドレスマップ、clock ツリー、reset/power のツリー、net の一覧、環境(world)。
- **記述時に検出するもの**: ポートの型・ロールの不一致(I2C の Target を SPI のバスに接続した、など)、アドレスの重なり、必須ポートの未接続、clock の循環、存在しない識別子、パラメータの型違い。エラーはインスタンスのパス付きで出す(例: `board:raspberrypi/pico > env0 > conn.bus`)。

### 14.3 フロントエンドは 2 つ、IR は 1 つ

- **宣言(TOML)**: 大多数(目安 9 割)の SoC/ボードはこれだけで書く。
- **Rust builder**: 計算が要る少数の例(多数の同形ブロックの生成、レジスタ値から決まる配線など)に使う。
- **記述自体にはループ・条件・式を入れない**(チューリング完全にしない)。計算は builder 側に出す。これで宣言が読みやすく、解析ツールが書きやすいまま保たれる。

### 14.4 継承と overlay

- `extends` で別の記述の全体を取り込み、`overlay` で差分を当てる。操作は **add / set / remove / replace-use** の 4 つだけに限定する。
- 例: 同じファミリーのチップ差(メモリ量、パッケージ、周辺の有無)は `family` の共通記述 + チップごとの overlay。ボード改訂や部品の実装オプション(DNP)も overlay。
- `xtask describe --expand` で全展開後の IR を表示できる。overlay の結果は常にこの形で確認できるようにする。
- 優先順位は「後から読んだ overlay が勝つ」で一本化し、上書きは `replace-use` のように明示した場合のみ許す。

### 14.5 記述のスケッチ(値は例示)

```toml
# catalog/soc/raspberrypi/rp2/rp2040/soc.toml
[soc]
id = 'soc:raspberrypi/rp2/rp2040'

[[cpu]]
name = 'core0'
use  = 'cpu:arm/cortex-m0plus'
view = 'private0'                      # コア専用ビュー

[[cpu]]
name = 'core1'
use  = 'cpu:arm/cortex-m0plus'
view = 'private1'

[[block]]
name = 'uart0'
use  = 'ip:arm/pl011'
at   = { space = 'sysbus', base = 0x40034000 }
irq  = { pin = 'combined', to = ['core0.nvic:20', 'core1.nvic:20'] }
clk  = { uartclk = 'clk_peri', pclk = 'clk_sys' }

[[block]]
name = 'i2c0'
use  = 'ip:synopsys/dw-apb-i2c'
at   = { space = 'sysbus', base = 0x40044000 }
irq  = { pin = 'combined', to = ['core0.nvic:23', 'core1.nvic:23'] }
pads = { sda = 'gpio0.func3', scl = 'gpio1.func3' }   # pinmux 経由

[[block]]
name = 'pio0'
use  = 'local:rp2040-pio'              # この SoC 専用。2 つ目の利用者が出たら ip へ昇格
```

```toml
# catalog/board/raspberrypi/pico/board.toml
[board]
id  = 'board:raspberrypi/pico'
soc = 'soc:raspberrypi/rp2/rp2040'

[[part]]
name = 'env0'
use  = 'part:bosch/bme280'
bind = 'i2c'                           # SPI でも話せる部品なので bind を選ぶ
conn = { bus = 'soc.i2c0', addr = 0x76 }
```

```toml
# 異種構成の抜粋(Zynq UltraScale+ 風)
[[cpu]]
name = 'apu0'
use  = 'cpu:arm/cortex-a53'
view = 'apu'                           # セキュア/非セキュアは Attr で区別

[[cpu]]
name = 'rpu0'
use  = 'cpu:arm/cortex-r5'
view = 'rpu'

[view.rpu]
extends = 'sysbus'
add     = [ { region = 'rpu-tcm', at = 0x0, size = 0x10000 } ]   # RPU の専用メモリ
```

### 14.6 外部形式との連携

- **SVD / SystemRDL → regs**: 既存の SVD(多数の MCU の公開資産)からレジスタ定義の雛形を生成し、手で振る舞いを足す。
- **DTS → 記述の雛形**: Linux や Zephyr のデバイスツリーから `soc.toml` の雛形を生成する取込コマンドを用意する。
- **FDT / ACPI の出力**: assemble が IR からゲスト向けの記述を出力する。FDT の書き出しは rust-vmm の `vm-fdt` が候補【確認済: 存在】。
- **IP-XACT**: 需要があれば取込のみ。

### 14.7 記述形式の比較

| 候補 | 長所 | 短所 | 判断 |
| --- | --- | --- | --- |
| TOML + 独自の extends/overlay | 標準的で読みやすく、スキーマ検証(serde + schemars)と相性が良い | 深い入れ子が冗長 | **採用(初期)** |
| KDL | ノード指向で階層記述が自然 | エコシステムが小さい | 保留 |
| 独自 DSL | 表現が最短 | パーサ・エディタ支援・文書化の負担 | 繰り返しパターンが固まるまで作らない |
| Rust builder のみ | 表現力が最大 | 解析ツールが書けず、非開発者が触れない | 補助として併用 |
| CUE / Nickel など | 制約と合成が強力 | 学習コストと依存 | 保留 |

## 15. 想定ケースの網羅表(これは何処に置くか)

迷ったらこの表を引く。表に無いケースが出たら、判定に使った理由を ADR に追記して表を更新する。

| 足したいもの | 置き場所 | 補足 |
| --- | --- | --- |
| 新しいバスプロトコル(例 I3C) | `contract/src/i3c.rs` + `hw/src/i2c` 隣接の機能ディレクトリ + conformance | 契約・実体・適合テストを 1 セットで追加 |
| 新しい ISA | `isa/<name>` + `accel/<name>`(自前)または accel/unicorn の arch feature | QEMU の target/<arch> に同じ |
| 新しい CPU コア | `isa/<arch>/src/cpu/<core>.rs` + `manifests/isa/<arch>/<core>.toml` | QEMU の target/<arch> 内の CPU モデルと同じ居場所 |
| CPU クラスタ(コア × N + L2 + GIC) | `catalog/subsys/<origin>/<name>` | 宣言が主 |
| SoC 内蔵の周辺 IP | `hw/src/<function>/<ip>.rs` + `manifests/hw/<function>/<ip>.toml` | 版差は variant、SoC 差は params |
| その SoC だけの専用ブロック | `catalog/soc/.../local/<chip>-<block>` | 2 つ目の利用者が出たら ip へ昇格 |
| 外部のセンサ・メモリ・PMIC | `hw/src/sensor/<part>.rs` 等(機能軸)+ manifest | 複数バス対応はモデルをディレクトリにして bind 別モジュール(bme280 型) |
| バススイッチ・mux(PCA9548 など) | `hw/src/i2c/<part>.rs` | ports に role = bridge を宣言 |
| クロスバ・ブリッジ(AXI, APB) | プロトコルのモデルは `interconnect`、SoC 固有の設定は soc の記述 | 配置・優先度は記述側 |
| IOMMU / SMMU | `catalog/ip/arm/...` + `contract/bus` の変換ロール | DMA マスタごとの視点で表現 |
| 割り込みコントローラ | 独立 IP は `hw/src/intc/`、コア密結合(NVIC)は `isa/<arch>` 側 | 境界は「CPU と別に存在できるか」 |
| SoC 内蔵の DSP・管理コア | machine の機械が cpu ノードとして宣言 | ファーム読み込みはメモリ書き込み + reset 線 |
| FPGA の論理(PL) | subsys + `bridge/rtl-verilator` または ip | RTL 協調は第17章 |
| ボード改訂・部品の実装差 | `board/.../<board>/rev-*` の overlay | DNP は remove で表現 |
| SoM + キャリアボード | `catalog/module` + `catalog/board` | module のコネクタ契約で接続 |
| シールド・PMOD・拡張カード | `catalog/module` | コネクタは `contract/signal/connector` |
| ゲーム機のカートリッジ・拡張スロット | `catalog/module` + MAME 型のスロット選択 | スロットは module の差し替え口 |
| 画面・音声・入力・tty・ネットワーク | `framework/host/*` + `contract/host` | モデルは契約のポートにだけ接続 |
| ホストの USB / I2C / GPIO の実機パススルー | `framework/host/*-passthrough` | Target 側アダプタとして実装 |
| 複数ボードのネットワーク・CAN バス | `catalog/world` + `interconnect/*-bus` | 仮想時間は全マシンで共有 |
| 温度・電池・ロボットなど物理環境 | `catalog/world` の環境 + `bridge/fmi` | FMI などの外部モデルに接続 |
| 実チップの ROM・ブートローダ | `firmware/` に記述のみ | ハッシュと入手方法だけを持つ |
| 時刻(RTC)をホストと同期 | `core/runtime` の時間モード + `host/clock` | 決定論モードでは使わない |
| gdb・トレース・カバレッジ・故障注入 | `framework/ops/*` | タップ点を購読する |
| 電源管理(PMIC)・電源ドメイン | 内蔵は ip、外部は parts + `contract/power` | ドメインは soc の記述 |
| ADC への入力・アナログセンサ | `contract/signal/analog` + world の環境 | 値の供給元は環境モデル |
| 暗号アクセラレータなど乱数を使う IP | `catalog/ip` | 乱数は ctx の RNG のみ |
| レトロ機(16bit アドレス、バンク切替、サイクル精度) | isa / cpu(cycle 忠実度)/ Region の動的変更 | 量子を最小にする精密モード |
| 実機を繋ぐ HIL | `engine/remote` または `bridge/hil` | バス上の MMIO を実機へ中継 |
| ホスト側の高速実行(KVM 等) | `engine/kvm` | contract は同じ |
| 外部の C/C++ 資産(既存デバイスモデル) | `ffi/*-sys` + `bridge/c-abi` | `catalog` には直接入れない |
| GUI・リモート制御 | `mosaic`(bin)を拡張、monitor 系は `ops/` | 実行入り口は 1 本だけ(ADR 0021/0024) |
| 検証用のテストベンチ(CPU なし) | `machine/src/<arch>/` + tests | CPU の無い machine も同じ仕組み |
| SoC を持たないマルチチップマシン全体(PC/AT など) | `machine/src/i386/pc/`(soc 宣言なし) | 8259A/PIT/RTC は `hw/src/intc` `hw/src/timer` `hw/src/rtc` の個別チップ(ADR 0023/0024) |
| `-kernel` による直接カーネル起動の実装 | `machine/src/<arch>/<機械>/` のブート経路モジュール | QEMU の `hw/i386/x86.c::x86_load_linux` / `hw/arm/boot.c` と同じ分担。語彙はフロントエンドが機械中立に保持。2 台目の同アーキ機で `machine/src/<arch>/boot.rs` へ昇格 |
| ゲストイメージの形式解析(ISO9660, El Torito, 将来の FAT/ELF) | `block/src/<format>.rs` | QEMU の `block/` 相当。CLI・機械・テストの 3 者が共有する形式関心事 |

## 16. pack とカスタム SoC / ボード

### 16.1 pack とは

`catalog` と同じレイアウト(`cpu/ ip/ parts/ subsys/ soc/ module/ board/ world/ firmware/`)のディレクトリ + `pack.toml`(識別子、版、必要な framework と contract の版、依存する他 pack、ライセンス、提供する識別子の一覧)。in-tree の `catalog/` も `pack:<proj>/first-party` という 1 つの pack として扱い、外部 pack と同じ検査・同じ読み込み経路を通す。

### 16.2 配布と読み込み

- **基本はソース配布**(Cargo の依存として取り込む)。Rust の ABI は安定しないため、動的ロードは使わない。
- **動的に差し込みたい場合**は、`bridge/c-abi`(C ABI)、`bridge/wasm`、別プロセスのいずれかに限る。
- `<proj>-pack-path`(環境変数/設定)で pack のルートを列挙し、`xtask catalog-build` が識別子の一意性と契約の適合を検査して、`catalog-all`(登録関数)を生成する。リンク時の自動登録(linkme / inventory 系)に頼らず、生成した明示的な登録関数を使う。

### 16.3 カスタム SoC を作る手順

1. `xtask new pack <org>` で外部リポジトリに pack の雛形を作る。
2. 既存の ip / cpu から使うものを選び、`soc.toml` に配線を書く(コードは書かない)。
3. 足りないブロックは pack 内の `ip/` に、第10章の解剖に従って実装する(最初は `local/`、再利用が見えたら昇格)。
4. `board.toml` に parts と配線を書く。
5. `<proj> run board:<org>/<board>` で起動し、conformance と machine テストを通す。

既存のボードを少し変えたいだけなら、`board:raspberrypi/pico` を `extends` した overlay を自分の pack に置き、部品を足す/外す。

### 16.4 SoM + キャリア、シールド、拡張コネクタ

- コネクタのピン定義(Arduino ヘッダ、PMOD、M.2、mikroBUS など)は `contract/signal/connector` に名前付きの集合として置く。
- SoM やシールドは `module` として、コネクタ契約の一端を公開する。キャリアボードやベースボードの `board` が、もう一端を公開する。
- 接続時に、ピンの数・電圧レベル・バス種別の整合を IR の検証で確かめる。

### 16.5 機密・NDA のモデル

- 社内限定の SoC やボードは **private pack** にする。framework 側に社外秘の名前が現れない。
- 実チップのブートROMなど再配布できないものは `firmware` に**記述だけ**(sha256 と入手方法)を置き、利用者が自分で用意したバイナリをハッシュで検証して使う。

### 16.6 識別子の衝突と上書き

- 識別子の `origin` は組織の名前空間として扱い、pack をまたいで一意にする。
- 別 pack の同名識別子は読み込み時にエラー。上書きは `replace-use` を明示した場合だけ許し、警告を出す。

## 17. 忠実度・協調シミュレーション・他言語

### 17.1 忠実度(A5)を 1 つのクレートに混ぜない

- 忠実度は functional / timing-approx / cycle / rtl-cosim の 4 段階とし、manifest に宣言する。
- **同じ識別子に複数の実装**を許す。Registry のキーは(識別子, 忠実度)で、記述側が `fidelity = 'cycle'` のように選ぶ。省略時は functional。
- 実装はクレートを分ける。共有できる `logic/` は共通クレートに出し、cycle 版が functional 版の状態機械を土台にする形も許す。
- cycle 忠実度は、量子を最小にする精密モード(第12章)とセットで使う。

### 17.2 bridge の一覧

| bridge | 用途 | 時間同期 | 備考 |
| --- | --- | --- | --- |
| `rtl-verilator` | RTL をそのまま 1 つのデバイスとして実行 | clock 契約に同期(サイクルごと or 量子ごと) | C++ モデルを FFI で囲む |
| `systemc-tlm` | SystemC / TLM-2.0 のモデルを取り込む | TLM の量子に合わせる | Simics が SystemC を 1 つのモジュールとして取り込む構成が参考になる【確認済】 |
| `fmi` | 物理モデル(FMU)との接続 | 固定刻みで同期 | world の環境モデルに接続 |
| `remote-port` | 別プロセス・別ホストのデバイスや他シミュレータ | ロックステップ/量子/非同期から選択 | 分割して重いモデルを別プロセスに出す用途も |
| `hil` | 実チップ・実機との接続 | 実時間 | engine/remote と併用 |
| `c-abi` | C で書かれた既存モデルの取込 | ctx 経由 | Simics のモジュール IF が C ABI で安定している点が参考【確認済】 |
| `wasm` | サンドボックス化したプラグイン | ctx 経由 | 信頼できないモデルの隔離 |

### 17.3 FFI の規約

1. `-sys` クレート(生の宣言)と安全ラッパーを分け、`unsafe` は bridge / engine / host の中に閉じ込める。
2. C++ の例外と Rust の panic は境界で捕まえ、必ずエラー値に変換する。境界を越えて伝播させない。
3. 所有権は境界で一本化する(誰が解放するかを型で表す)。
4. 外部コードのライセンスは bridge 単位で隔離し、`catalog` へ持ち込まない(第21章)。
5. C/C++ のビルドは `cc` / `cmake` クレートで行い、framework の層構造の中には C/C++ のソースを置かない(`ffi/` と `third_party/` に隔離)。

### 17.4 複数マシン・分散

- **同一プロセス内の複数マシン**: world が複数の Machine を持ち、仮想時間を共有する。複数ターゲットを 1 プロセスで並列に走らせる点は Simics【確認済】、複数マシンの同時エミュレーションは Renode【確認済】にも見られる。
- **別プロセス/別ホスト**: remote-port で分割する。時間は world が調停する。
- **リンク**: UART、CAN、Ethernet、I2C(複数ボードで共有するバス)などは world の記述で結び、実体は interconnect を使う。

## 18. ビルドと CI

### 18.1 Cargo workspace

- `members` は **`xtask sync-workspace` が manifest から生成する**。glob に頼らないのは、クレートでないディレクトリの混入や重複・欠落を機械的に検査するため。
- `[workspace.dependencies]` で外部依存の版を一元管理する。`default-members` は framework のみとし、catalog 全体のビルドは明示的に行う。
- **粒度**は「1 IP / 1 部品 / 1 CPU = 1 クレート」。依存規則(第8章)が機械で効くことを優先する。クレート数が数千規模になり重くなったら、出自単位の集約クレート + feature + モジュール境界 lint への移行を ADR で検討する。
- クレートが数百〜数千になる前提で、`cargo-hakari` の workspace-hack(feature の統一でビルド成果物を共有)【既知】、`sccache`、`cargo-nextest`、変更されたクレートとその依存先だけを対象にするテスト選択(`xtask affected`)を最初から用意する。

### 18.2 生成物

| 生成物 | 生成元 | 扱い |
| --- | --- | --- |
| `regs_gen.rs` | `regs/*.toml`(または SVD) | コミットし、CI で再生成して差分を検査 |
| `catalog-all`(登録関数) | 全 pack の manifest | コミットしない。ビルド時に `xtask` が生成 |
| `docs/catalog-views/` | manifest のタグ | 機能別・バス別・出自別・忠実度別・検証水準別の一覧を生成 |
| 依存グラフ図 | `cargo metadata` | 層違反の可視化 |

### 18.3 xtask のコマンド

| コマンド | 内容 |
| --- | --- |
| `lint-layers` | 層の依存規則(R1〜R3, R5)の検査 |
| `lint-names` | 禁止語、出自名、クレート名の規則 |
| `lint-manifests` | ip.toml 等のスキーマ、ports と実装の整合 |
| `regs` | レジスタ定義からコードを生成、差分検査 |
| `catalog-build` | catalog-all と検索ビューの生成 |
| `new <kind> <origin>/<name>` | 雛形生成(第10章の構造、forbid(unsafe)、テスト、manifest) |
| `describe --expand` | overlay 展開後の IR を表示 |
| `import dts / svd` | 外部形式からの取込 |
| `sync-workspace` | members の生成 |
| `affected` | 変更の影響範囲のテスト選択 |
| `doctor` | 環境と構成の自己診断 |

### 18.4 CI のステージ

| ステージ | 内容 | 頻度 |
| --- | --- | --- |
| fast | fmt, clippy, lint-layers / names / manifests, 生成物の差分検査 | 毎コミット |
| unit | logic の単体テスト、proptest | 毎コミット |
| model | レジスタスクリプト、snapshot 往復、contract の conformance | 毎コミット(affected のみ) |
| machine | ファームを起動して出力を比較 | PR ごと |
| engine-conformance | Unicorn と自前エンジンの差分検証 | PR ごと(エンジン変更時は必須) |
| nightly | fuzz、長時間実行、決定論の検査 | 毎晩 |
| perf | 命令/秒、MMIO/秒、起動時間の回帰 | 毎晩 + リリース前 |
| docs | ドキュメントの生成、仕様対応表の検査 | PR ごと |

### 18.5 feature と版

- ボードごとの Cargo feature は作らない(組み合わせ爆発)。app は `catalog-all` から選び、軽くしたいときは `--pack` で対象 pack を絞って `catalog-all` を再生成する。
- contract は個別にセマンティックバージョンを持ち、`cargo-semver-checks` 相当の検査を CI に入れる【既知】。pack は対応する contract の版範囲を `pack.toml` に宣言する。

## 19. テスト戦略

| 階層 | 対象 | 内容 |
| --- | --- | --- |
| T1 logic 単体 | `logic/` | 状態機械の単体テスト、proptest |
| T2 モデル | 1 モデル | レジスタスクリプト(読み書き列 + 期待値)のハーネスで契約を通して検証 |
| T3 conformance | contract ごと | その契約を実装する全モデルが通す共通テスト |
| T4 golden-trace | モデル | データシート・実機・RTL 由来のトレースとの一致 |
| T5 machine | ボード | ファームを起動し、UART 出力などを比較。ファームは再現可能ビルドで、出自を記録 |
| T6 engine conformance | エンジン | Unicorn と自前エンジンを命令単位で突き合わせ(レジスタ・メモリ差分)。既存の命令セット適合テスト群も併用【既知】 |
| T7 fuzz | 横断 | MMIO アクセス列、記述ファイル、スナップショットファイル |
| T8 決定論 | 横断 | 同じ入力で 2 回実行して状態ハッシュを比較、snapshot 往復、機能モデルは量子を変えても結果が同じになること |
| T9 性能 | 横断 | 命令/秒、MMIO/秒、起動時間。ベンチを固定して回帰を検出 |

**追跡可能性**: 各モデルの `docs/` に「仕様の章・表番号 → テスト名」の対応表を置き、`lint-manifests` が空欄を警告する。検証水準(A7)は、この表とテスト階層の到達度で自動判定する。

## 20. 性能設計の勘所

- **MMIO ホットパス**: `dyn` 呼び出しは 1 回に抑え、FlatView の検索は二分探索かページ単位の直接表にする。
- **DMI**: RAM はホストポインタとしてエンジンに渡す。MMIO だけがコールバックを通る。
- **量子**: 精度と速度のつまみ。モードごとの既定値は実測で決め、ADR に残す。
- **TB 無効化**: Region の更新を最小の範囲に限定し、無関係な範囲の翻訳済みコードを捨てない。
- **所有構造**: デバイスは Machine 所有のアリーナに置き、インデックスで参照する。コールバック中は対象を一時的に取り出す(take-out)ことで、`Rc<RefCell>` を避けつつ二重借用を防ぐ【要検証】。
- **ホットパスで割り当てない**: トレース・ログは無効時にコストゼロ(第13章)。
- **スナップショット**: メモリはコピーオンライト + ダーティ追跡で差分保存。
- **並列化は後**: 契約を `Send` に保つだけにして、最初は単一スレッドの決定論を優先する。
- **計測基盤は最初に作る**: contract を決める段階でベンチを用意し、設計判断を数字で裏付ける。

## 21. ライセンスと出自管理

> 以下は設計上の論点整理であり、法的助言ではない。方針の確定前に必ず法務の確認を取ること。

### 21.1 事実と論点

- Rust クレート `unicorn-engine` のライセンスは **GPL-2.0**【確認済】。Unicorn 本体も GPLv2 の配布条件とされている【確認済】。
- このため、Unicorn をリンクしたバイナリを**配布する**場合の扱いが、プロジェクト全体のライセンス方針に直結する。自前エンジンへ置換する計画があっても、置換前に配布や公開をするなら論点は残る。
- `unicorn-engine` には `dynamic_linkage` feature がある【確認済】が、動的リンクにすれば GPL の扱いが変わるかどうかは法的判断が必要で、技術的な選択だけでは決まらない。
- Apache-2.0 など一部のライセンスは GPL-2.0-only との互換性に注意が必要とされる【既知】。依存クレートの選定にも影響する。

### 21.2 方針の選択肢

| 案 | 内容 | 長所 | 短所・注意 |
| --- | --- | --- | --- |
| A | プロジェクト全体を GPL 互換のライセンスで最初から公開 | 単純で、Unicorn の利用に迷いがない | 後から非 GPL へ再ライセンスするには貢献者全員の同意が必要。CLA / DCO を初日から整備しておく必要がある |
| B | Unicorn の部分を別バイナリ・別プロセスに分離して配布 | ライセンスの境界を明確にしやすい | MMIO のたびにプロセス間通信になり、性能と実装が現実的でない可能性が高い |
| C | 内部利用に限定して開発し、自前エンジンへ置換してから公開 | 公開時のライセンスを自由に選べる | 内部利用の範囲の線引きと、外部協力者の扱いを法務で確認する必要がある。置換までの期間が長いと実質的に運用が難しい |

推奨は、**まず A と C のどちらで行くかを初日に決める**こと。どちらを選んでも、次の保険は共通で入れる。

### 21.3 置換を可能に保つ設計上の保険

1. Unicorn への依存は `engine/unicorn` クレートに限定する(第8章 R2)。他の層に Unicorn の型・定数を漏らさない。
2. `engine/conformance` を最初から作り、自前エンジンへ置換する際の同値性を検証できるようにする(第19章 T6)。
3. `arch_*` features で必要な ISA だけをビルドし、配布物に含まれる範囲を明確にする【確認済】。

### 21.4 出自(provenance)管理

- すべてのクレートの `Cargo.toml` に SPDX 形式で `license` を書き、`cargo-deny` で許可リスト外のライセンスを CI で拒否する。
- manifest の `spec` に、実装の根拠にした仕様書(データシート、TRM の文書番号)を書く。
- **クリーンルーム方針**: 他の GPL プロジェクト(QEMU の C 版・Rust 版の PL011 実装などを含む)のコードを読み写ししない。仕様書から書く。外部コードから派生した場合は `derived_from` に明記する。
- 取り込む外部資産(SVD、DTS など)は、取込時にライセンスと入手元を manifest に記録する。SVD は提供元ごとに条件が異なる【既知】。
- ブートROMなど再配布できないものは同梱しない(第16章)。

### 21.5 貢献規約

DCO か CLA のどちらかを初日に決める。A を選ぶ場合、将来の再ライセンスの可能性を残したいなら CLA が必要になる。

## 22. ロードマップ

期間の見積りは体制次第のため、順序と**出口条件**だけを示す。

| フェーズ | 内容 | 出口条件 |
| --- | --- | --- |
| P0 骨格と強制 | ADR(第23章の 6 項目)、workspace、層ごとの空クレート、xtask の lint-layers / lint-names、CI の fast ステージ | 層違反と禁止語がコミット時に落ちる |
| P1 縦スライス 1 | 単一コア(最小の MCU 級)+ Unicorn、core の最小実装、contract(mmio, irq, clock)、IP 2 つ(UART、タイマ)、SoC 1、ボード 1、machine テスト | ファームが起動して期待した UART 出力を得る。決定論テストが通る。ベンチを初計測 |
| P2 バスとピン | contract(i2c, spi, gpio, net)、interconnect、GPIO + pinmux、i2c コントローラ、センサ部品 | 同じセンサ部品が「コントローラ経由」と「bit-bang」の 2 経路で動く |
| P3 マルチコア・異種 | 2 コア SMP → 異 ISA の AMP、コア別ビュー、量子スケジューラ、メールボックス | 決定論を保ったまま異種構成が起動する |
| P4 スナップショットと運用基盤 | snapshot、記録再生、gdbstub、trace、故障注入 | 保存・復元・再生で状態ハッシュが一致する |
| P5 自前エンジン | native-kit + 1 ISA、engine-conformance、切替 | Unicorn と自前エンジンが同じテスト群を通る |
| P6 pack と外部提供 | pack.toml、private pack、DTS / SVD 取込、カスタム SoC の手順書 | 外部リポジトリだけでカスタム SoC / ボードが起動する |
| P7 協調と多機械 | bridge(rtl, systemc, fmi, remote-port)、world | 複数ボードと RTL モデルが 1 つのシナリオで動く |
| P8 大型 SoC | MMU / IOMMU / 割り込み階層 / PCIe / USB / virtio など | Linux 級のファームが起動する |

**原則**: (1) 層のディレクトリは先に作り、中身は空でよい。強制は P0 で入れる。(2) **contract は 2 つ目の実装が現れるまで確定扱いにしない**(例: i2c は 2 種類のコントローラで確かめる)。(3) 各フェーズは縦に貫く最小の機能で終える。

## 23. 初日に決める事項(不可逆なもの)

| # | 項目 | 選択肢 | 推奨 | 変更コスト |
| --- | --- | --- | --- | --- |
| 1 | contract の形 | 粒度、Ctx の有無、同期/非同期、エラー型 | Ctx 渡し + 同期コールバック + イベントキュー | 極大 |
| 2 | アドレス空間モデル | 単純な表 / Region 木 + FlatView | Region 木 + FlatView + Attr + 視点 | 極大 |
| 3 | 時間モデル | 分解能、決定論を既定にするか、量子の扱い | VTime(ps / u64 を候補)、決定論を既定、量子で調整 | 大 |
| 4 | スナップショット | 版付き State + ヘッダ、後付け | 初日から全モデルに State を義務化 | 大(後付けは非常に困難) |
| 5 | manifest / IR のスキーマ | 識別子、ports、params、tags の形 | 第5章・第10章・第14章の形から開始し、1 年は互換性を気にせず進化 | 大 |
| 6 | ライセンスと貢献規約 | 第21章の A / B / C、DCO / CLA | 法務確認のうえ決定 | 極大(後からの変更は同意が必要) |

**中程度のコストで決めるもの**: クレートの粒度(第18章)、記述形式(第14章)、origins.toml の初期内容、禁止語リスト。

**後回しにしてよいもの**: 並列実行、GUI の作り込み、wasm / プラグイン ABI、独自 DSL、KVM などのハードウェア支援。ただし契約が `Send` を保つことと、観測点が最初からあることだけは守る。

## 24. リスクと未決事項

| リスク | 影響 | 対策 |
| --- | --- | --- |
| 設計が大きすぎて動くものができない | 進捗の停滞 | P1 の縦スライスを優先。層は空で作り、強制だけ先に入れる |
| contract の設計ミス | 全モデルの書き直し | 2 つ目の実装が現れるまで確定しない。`#[non_exhaustive]` と能力交渉。semver 検査 |
| Unicorn の制約(SMP、精密例外、再入、タイミング) | 精度・決定論の限界 | `EngineCaps` で宣言し、機能を絞る。自前エンジンへの置換と conformance で補う |
| GPL-2.0 の扱い | 公開・配布の自由度 | 第21章。初日に方針を決め、法務確認 |
| 時間モデルの手戻り | 全体の再設計 | 初日に決定。P1 でベンチと決定論テストを入れる |
| Rust の所有権(デバイス間参照、再入) | 実装の複雑化 | Ctx 渡し、アリーナ + take-out。早期にプロトタイプで検証 |
| manifest と Cargo.toml の二重管理 | 不整合 | xtask が生成・整合検査 |
| クレートの爆発 | ビルド・IDE の重さ | hakari、sccache、affected テスト、集約クレート案 |
| pack の ABI | 外部提供の破綻 | ソース配布を基本にし、動的ロードは C ABI / WASM / 別プロセスのみ |
| 分類をめぐる議論の再燃 | 作業の停滞 | 第15章の表と ADR で決着させる |
| 外部資産(SVD、DTS)のライセンス | 公開時の問題 | 取込時に出自とライセンスを記録 |

### 24.1 この設計書で未確認の事項(【要検証】の集約)

1. Unicorn の複数インスタンス間でのメモリ共有、無効化の同期、排他モニタの扱い(第12章)。
2. Unicorn のフック中の入れ子操作の制約(第12章)。
3. cargo-deny の `wrappers` による層別の依存制限の表現力、clippy の設定ファイルが catalog 配下のクレートに効く範囲(第8章)。
4. DMA が別デバイスの MMIO に届くときの再入制約、アリーナの take-out 方式(第10章、第20章)。
5. ワードアドレスなど 8bit 単位でないアドレス空間の扱い(第12章、第13章)。

### 24.2 今回の調査で網羅できていない項目

今回は QEMU、Renode、MAME、Simics、rust-vmm、Unicorn を中心に確認した。次の項目は未調査で、設計に影響しうるため、追加の調査候補として挙げる: gem5(マイクロアーキテクチャ寄りのシミュレータの構成)、QEMU の内部(qdev のバスモデルと Kconfig の運用の最新)、SystemC / TLM-2.0 の標準仕様の細部、IP-XACT と SystemRDL によるレジスタ記述、Zephyr / Linux のデバイスツリーのバインディング設計、FMI の仕様、Qiling・Avatar2・libafl 系のエミュレーション応用。QEMU の Rust 対応は 2025 年時点の記述に基づく。

### 24.3 設計を確定するために聞きたいこと

- 最初に動かす対象(MCU 級か、Linux が動く SoC か、レトロ機か)と、最初の ISA。
- 決定論と記録再生を、どの程度の必須要件として扱うか。
- 公開の予定とライセンス方針(第21章の A / C のどちらに近いか)。
- 想定する規模(IP の数、貢献者の数)と開発体制。
- 実機を繋ぐ用途(HIL)や、RTL 協調の予定の有無。

## 25. 代替案と不採用理由

| 案 | 長所 | 短所 | 判断 |
| --- | --- | --- | --- |
| A 機能別に分ける(QEMU / Renode 型) | 機能で探しやすい、モデル千件でも破綻しない(QEMU が実証) | 機能 1 軸に種別・出自を詰め込むと混ざる | **修正採用**(ADR 0024): 機能軸(デバイス)とアーキ軸(機械)の 2 軸を使い分け、出自は名前空間へ退避 |
| B 出自(ベンダー別)のみで分ける | 同一性が明確 | SoC・IP・部品・ボードが混在する | 種別を最上位にして解決 |
| C 巨大な 1 クレート + feature | クレート数が少ない | 依存規則が機械で効かない、並列ビルドが効きにくい、独立テストが難しい | 不採用(数千規模で再検討) |
| D IP ごとに別リポジトリ | 所有が明確 | 版の整合管理が重く、契約の変更が全体に波及しづらい | 不採用。必要な分だけ pack として外出し |
| E Rust の動的ライブラリによるプラグイン | 後から差し込める | ABI が安定しない | 不採用。C ABI / WASM / 別プロセスに限る |
| F 本設計(v0.1: 種別 → 出自 + タグ + 生成ビュー) | 多軸の属性を衝突させず、依存を機械で強制できる |出自軸は機能を答えず、1 モデル 1 クレートが lib.rs 単体を生み、機械が apps に逃げた | v0.2 で軸を QEMU 型へ置換(ADR 0024)。タグ + 生成ビュー、契約/実体の分離、IR 一本化は維持 |

## 付録 A. 参考資料

- Renode: プラットフォーム記述 https://renode.readthedocs.io/en/latest/basic/describing\_platforms.html / マシンの扱い https://renode.readthedocs.io/en/latest/basic/machines.html / CPU 専用メモリ https://antmicro.com/blog/2022/07/simulating-socs-with-isolated-address-spaces-in-renode
- QEMU の Rust 対応: https://qemu.readthedocs.io/en/master/devel/rust.html / single-binary(KVM Forum 2025): https://pretalx.com/kvm-forum-2025/talk/ZFLGZZ
- MAME: machine\_config https://git.redump.net/mame/tree/src/emu/mconfig.cpp / slot device https://git.redump.net/mame/tree/src/emu/dislot.h
- Simics: https://www.intel.com/content/www/us/en/developer/articles/technical/simics-simulator-technology.html / SystemC ライブラリ https://dvcon-proceedings.org/wp-content/uploads/integrating-different-types-of-models-into-a-complete-virtual-system-the-simics-systemc-library.pdf / DML https://github.com/isabella232/device-modeling-language
- rust-vmm: https://github.com/rust-vmm/community
- Unicorn(Rust クレート): https://docs.rs/crate/unicorn-engine/latest / リリース https://github.com/unicorn-engine/unicorn/releases

## 付録 B. 用語集

| 用語 | 意味 |
| --- | --- |
| contract | モデル間・層間の約束(trait と型)。実装を含まない |
| interconnect | バスやブリッジなど、接続の実体 |
| IR / MachineGraph | 記述から生成される中間表現。配線とアドレスマップを持つ |
| pack | catalog と同じレイアウトの配布単位 |
| ctx | モデルが外へ働きかけるための唯一のハンドル |
| FlatView | Region 木を平坦化したアドレス解決用の表 |
| 量子(quantum) | 各コアに一度に与える実行予算 |
| tap | 観測点。trace などが購読する |
| DMI | RAM を直接ホストポインタとして渡す経路 |
