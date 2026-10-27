# 0023: 機械モデルの居場所（catalog board とブート経路の分担）

状態: 採用。機械モデル（マシンの実装）は `catalog/board/<origin>/<board>/`
のコードパッケージに置く。フロントエンド（`apps/system`）は QEMU の
`softmmu/vl.c` 相当（オプション解析と `-M` のディスパッチ）だけを担い、
機械のコードを 1 行も持たない。

根拠: 設計書 §6（framework/catalog の 2 分割、apps は cli/gui/server）、
§7.2（board は宣言が主）、§15（迷ったら表に引く）。オーナー指摘
「ディレクトリ構成・名前が、将来モデルを大量に追加した場合の問題
（設計書 §3 の『混ざる』）に対応できていない」「なぜ linuxboot のような
専用のものをフロントエンドに足しているのか。QEMU はそこをどうして
いるのか参考にせよ」「構成がまだ一貫しておらずバラバラ」。
参照モデルは QEMU そのもの（下表）。

## 背景

ADR 0021 でフロントエンドを 1 本に整理したとき、PC マシンの実装
（BIOS INT サービス、ポート空間、El Torito、ブートループ）と i386
ブートプロトコル（bzImage、zero page）を `apps/system/src/` 内に
置いた。この配置は設計書 §3 の M1（種別の混在）そのものであり、
機械を追加するたびにフロントエンドが肥大する。実際、
`catalog/board/` には 1 つもコードが無く、機械実装はすべて
アプリ側（`apps/system`、`apps/stm32f103`、`apps/bringup`）に逃げて
いた。逃げた理由は層ポリシー側にもある: 従来のルールでは board の
glue が `core/types` と `contract` しか依存できず、IP クレートも
エンジンも使えないため、機械を catalog に置くことが**そもそも不可能**
だった（だから STM32F103 はアプリになった）。

## QEMU の分担（参照）

| 役割 | QEMU | Mosaic |
| --- | --- | --- |
| CLI・`-M` 解決・機械リスト | `softmmu/vl.c`, `system/main.c` | `apps/system`（`mosaic-system`） |
| 機械モデル（マシン型） | `hw/i386/pc_piix.c`, `hw/arm/virt.c` … | `catalog/board/<origin>/<board>/glue` |
| アーキ共通の `-kernel` 起動 | `hw/i386/x86.c` `x86_load_linux()`, `hw/arm/boot.c` `arm_load_kernel()` | board 内 `linux_boot`（2 台目の x86 機で modelkit へ昇格、Rule of two） |
| イメージ形式の汎用解析 | `hw/core/loader.c`, `block/` | `framework/modelkit/iso9660` |
| 機械の登録 | `MODULE_INFO("machine")`（vl.c は触らない） | board クレートの `MACHINE_NAME` + フロントエンドの 1 arm |

QEMU に「linuxboot マシン」は存在しない。`-kernel`/`-initrd`/`-append`
は `MachineState` が持つ機械中立のオプションであり、その**意味**を
各マシンのブートプロトコルが実装する。`pc-bios/linuxboot_dma.bin` は
BIOS 互換ディスク起動のためのオプション ROM という実装詳細に過ぎない。
したがって `linux_boot` をフロントエンドのモジュールとして置くのは
誤りで、「`pc` マシンの `-kernel` 経路」として board に属する。

## 決定

1. **機械モデル = catalog board パッケージ**。`catalog/board/ibm/pc/`
   （`board:ibm/pc`）が最初の実装。宣言（`board.toml`）と実装
   （`glue/` クレート `mosaic-board-ibm-pc-glue`）が隣接する。機械の追加は
   board パッケージの追加であり、フロントエンドは 1 arm 増やすだけ
   （QEMU の `hw/` 追加が `vl.c` を触らないのと同じ）。

2. **SoC を持たないマシンのために `[board] soc` を任意にする**
   （`describe` スキーマ変更）。PC/AT は PIC/PIT/RTC が個別チップの
   マルチチップマシンであり、SoC を 1 つも持たない。SoC 無しの board
   は parts/conn を宣言できず、組立は glue クレートが担う。

3. **層ルール R2/R5 の修正**（`xtask/src/policy.rs`）: composite
   （board/soc/subsys/module/world）の glue は、フレームワークの
   `types`/`contract`/`modelkit`/`isa` に加え、**`engine`** と、
   catalog の葉（`ip`/`part`/`cpu`/`ip-lib`）に依存してよい。
   QEMU の `hw/<arch>` 機械が cpu コアを直接使うのと同じ到達範囲。
   ただし `assemble`/`host`/`ops`/`bridge`/`describe`/`runtime`
   （合成のサービス層）と他の composite への依存は引き続き禁止。
   これにより「機械を置ける場所が apps にしかない」という構造欠陥が
   なくなり、機械は種別の軸（catalog/board/）に乗る。

4. **`-kernel` 系は機械のブート経路**。語彙（`-kernel`/`-initrd`/
   `-append`、`DirectKernel`）はフロントエンドが機械中立に保持し、
   i386 ブートプロトコルの実装は pc board の `linux_boot` が持つ。
   2 台目の x86 機（`q35` 等）が現れた時点で、`x86_load_linux()` が
   全 x86 機械型から呼ばれるのと同じ形で modelkit へ昇格する
   （設計書 §4 P12 Rule of two）。AArch64 `virt` の `-kernel` は
   別プロトコル（Image + DTB）なので共有しない。

5. **イメージ形式の解析は modelkit**。ISO9660 のファイル取り出しと
   El Torito の抽出は `framework/modelkit/iso9660` に置く。もともと
   CLI（`iso!<path>` オペランド）、機械、テストの 3 者が使う形式
   関心事であり、QEMU の `hw/core/loader.c` + `block/` 相当。
   （当初は linux_boot モジュール内にあった。）

6. **フロントエンドの語彙は再エクスポートで安定させる**。
   `mosaic_apps_system::{PcBootOptions, PcBootReport, run_pc_machine}`
   は board クレートからの再エクスポートであり、ADR 0022 のテスト
   台帳と CI の CLI 経路は無変更で動く（実測: front_end 5/5、
   boot_matrix の tinycore 2 経路とも再現）。

## 結果

- `apps/system/src/` は `lib.rs`（語彙 + ディスパッチ）、`main.rs`
  （CLI）、`report_json.rs`（JSON 報告書）の 3 ファイルになり、機械
  実装は消えた。次の機械（`virt-64`、`goldfish`、`versatile-pb`）は
  `catalog/board/<origin>/<name>/glue` として並ぶ。
- `mosaic-modelkit-iso9660`（形式）、`mosaic-board-ibm-pc-glue`（機械）が
  追加され、層 lint（`xtask check`）が新しい依存方向を機械的に強制する。
- この ADR を設計書 §15 の表に反映し、以後の「どこに置くか」判定は
  表 + 本 ADR に従う。

## 未対応・次の課題

- `apps/bringup`、`apps/stm32f103` は縦スライス用途のアプリとして
  残っている。stm32f103 の構成（board.rs）は R2/R5 修正後に
  `catalog/board/st/...` へ引っ越せる候補だが、本 ADR では機械の
  居場所の原則を確定するだけで、移設は個別の作業とする。
- `catalog/board/ibm/pc/board.toml` は宣言のみ（部品グラフは空）。
  describe/assemble が x86 の粒度をカバーした時点で、glue の命令的
  組立を宣言側へ少しずつ移す（PCI バス、CMOS/RAM 記述から）。
