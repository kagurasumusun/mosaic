# 0033: QEMU と同等のカバー範囲への拡張方式 — virtio コア・ブロック基盤・機械軸

* 状態: 採用
* 日付: 2026-10-28
* 関連: ADR 0024 (QEMU 型ディレクトリ軸), ADR 0025 (再利用基盤・カタログ),
  ADR 0030 (Unicorn 一本化), ADR 0031 (hw 責務別分類), ADR 0032 (ポート面),
  設計書 §1・§5・§6・§14

## 背景

オーナー指示 (2026-10-28): 「カバー範囲が qemu と同等になるまで突貫で
進めて。構造もよく考えて。ちょくちょく push もして。」

現状のカバーは非対称である。QEMU の `hw/` 相当の機能分類 (23 ディレクトリ)
と部品カタログ (1369 モデル) は存在するが、レジスタ実装を持つ「実モデル」は
約 64 件で、残る約 1300 件は 16 バイトのスクラッチ RAM を返すスケルトン
(ADR 0026〜0029 の大量配置の足場) である。機械は 2 台 (pc・stm32f103) で、
QEMU の主要機械クラス (virt 系・ボード系) は未実装。virtio はトランスポート
(virtio-mmio・virtio-pci) もデバイス側コア (vring 処理) もスケルトンで、
ブロック・ネット・コンソールのバックエンド基盤が存在しない。

「突貫」をただの量産にしないために、何をどの順で実装に置換えるかと、
量産が構造を壊さないための境界を決める必要がある。

## 決定

### 1. カバー範囲の定義と順序

「QEMU と同等」を数えられる形で定義する。カバー = QEMU の `hw/<fn>/`
各機能分類のうち**実機が実際に使う主要デバイスがレジスタレベルで実装
されていること** + **その機械クラスが `-M` で起動できること**。順序は
ブート上重要度 (QEMU の `-M virt` / `-M pc` が載せているものから):

1. **intc**: GICv3 (GICD+GICR), PLIC, CLINT — virt 系機械の必須三部
2. **virtio**: vring コア + virtio-mmio v2 トランスポート + blk/net/
   console/rng/input — QEMU virt の標準周辺
3. **機械**: `machine/arm/virt64`, `machine/riscv/virt` (アーキ軸, ADR 0024)
4. **x86 追加**: e1000, PCI ECAM ホスト, virtio-pci legacy, AHCI
5. **表示・入力**: stdvga+VBE, virtio-gpu, virtio-input
6. **組込み量産**: STM32 F4/F7/H7 などファミリ単位のスケルトン置換

各波はコミット + push を分けて、壊れた状態を溜めない。

### 2. virtio は 3 層に分ける — トランスポート / コア / デバイス

QEMU の `hw/virtio/` と同じ分割を `hw/ip/virtio/` 内に置く
(1 つの機能分類内のサブ構造であり、ディレクトリ軸は増やさない):

* **`vqueue.rs`** — 分割 vring の共通機構。ディスクリプタ連鎖・avail/used
  リング・書き込み戻し。トランスポートもデバイスもここを共有する
  (ADR 0031 modelkit は「2 ドメイン以上から使う」場合の置き場所なので、
  同一 `ip/virtio` 内の共有は機能ディレクトリ内モジュールで足りる)。
* **`port.rs`** — `VirtioDevice` ポート。トランスポートがデバイスに
  「設定の読み書き」「キューの設定」「キューへの通知」「機能交渉」を
  呼ぶ語彙。逆方向 (デバイス→トランスポート) は割り込み要求と
  設定変更通知の 2 つだけ。**契約は使う側が 2 種類 (mmio・pci
  トランスポート) に確定した時点で `contract` クレートへ昇格する**
  (Rule of two, 設計書 P12)。現時点では hw 内モジュールにとどめる。
* **`virtio_mmio.rs` / `virtio_pci_*.rs`** — トランスポート。レジスタ群
  (virtio-mmio v2 レイアウト, PCI BAR/能力構造) を解釈して
  `VirtioDevice` を駆動する。ゲストが見るのはこのレジスタ面だけ。
* **`virtio_blk.rs` 等** — デバイス実装。`VirtioDevice` を実装し、
  機能はすべてホスト側のバックエンド (ブロックイメージ・ネットリンク・
  コンソールストリーム・エントロピーソース) に委譲する。

デバイスモデルはトランスポートを知らない。トランスポートはデバイスの
機能を知らない (デバイス ID・機能ビットでしか参照しない)。この 2 つの
独立性が「1 モデル 1 箇所」(ADR 0031) を virtio 内でも保証する。

### 3. ブロックイメージ基盤は contract に置く

virtio-blk・AHCI・IDE・SD ホスト・NVMe はすべて「ランダムアクセス可能な
ブロックイメージ」を消費する。これは SD カードだけの話ではなく
QEMU の `block-backend` 相当の横断契約なので、`contract::block` に置く:

* `BlockImage` — セクタ単位の読み書き + フラッシュ + サイズ。
  実装は機械グルーが持つ (テストはインメモリ、フロントエンドは
  ファイルバックエンド)。モデルはイメージの由来を知らない。
* `HostAttachments::attach_block` — SD の `SdCardHandle` と同じ経路で
  spawn に届ける (ADR 0025 のホスト接続点)。

### 4. 機械はアーキ軸で増え、宣言 + グルーの二層を守る

`machine/arm/virt64`・`machine/riscv/virt` は ADR 0024 の機械アーキ軸に
置く。各機械は:

* **manifest** (`manifests/machine/<arch>/<name>/`): メモリマップ・ブロック・
  配線の宣言 (QEMU の `virt_memmap` 相当の事実)。
* **glue** (`machine/<arch>/<name>/`): 宣言で書けないホスト固有物だけ —
  FDT 生成 (boot クレートの `proto/dtb`)、イメージロード、CPU 起動、
  割り込み配送 (Unicorn 制約、ADR 0009/0018 の台帳)。

機械の追加は `MachineKind` (フロントエンド) とブート台帳
(`crates/tests/images.toml` の `machine` 列) に 1 行ずつ反映する。

### 5. 実イメージ検証の規律は維持する

ADR 0022 の方針は拡張中も不変。スケルトン置換の単体テストはレジスタ
仕様に基づく通常のモデルテスト (合成イメージを OS 起動と主張しない) で
検証し、OS 起動の主張は実イメージ台帳の行だけで行う。Unicorn 2.1.x に
外部割り込み注入 API が無い (ADR 0009) ため、virt 系機械での実 OS 起動は
「割り込み配送が機械側に入るまで machine-pending」として台帳に理由を
明記する。スケルトンの成功で代替しない。

## 受け入れ基準

* GICv3・PLIC・CLINT がレジスタ単体テストで仕様値を返すこと。
* virtio-mmio v2 でゲストが機能交渉→キュー設定→処理→割り込みまで
  レジスタ操作で完結すること (ドライバ無しのレジスタテスト)。
* `machine/arm/virt64`・`machine/riscv/virt` が manifest から組み上がり、
  ベアメタルの起動経路 (ベクタ→C エントリ) が実行できること。
* 既存 705 テストがすべて緑のまま、各波が独立したコミットで push されること。

## 実装の結果（後で追記）

- **Wave 1 (commit 520add6)**: GICv3 (GICD+GICR)・PLIC・CLINT を仕様ベース
  の実装へ置換。GICv3 は IHI 0069、PLIC は SiFive 仕様、CLINT は RISC-V
  特権仕様の machine timer 章からクリーンルーム。各モデルはレジスタ単体
  テストで仕様値を返す。
- **Wave 2 (commit d0e43e5)**: virtio 3 層化 — `vqueue.rs` (共通 vring)、
  `port.rs` (`VirtioDevice` ポート)、`virtio_mmio.rs` (MMIO v2 トランス
  ポート)、および blk/net/console/rng/input/gpu/balloon/scsi/9p の各デバイス。
  `contract::block` と `contract::guest_mem` でブロックイメージ基盤とゲスト
  メモリポートを横断契約化。
- **Wave 3 step 1 (commit 5a647a0)**: AArch64 + RV64 エンジン語彙。
  `EngineArch` に `AArch64Le` と `RiscV64Le` を追加、unicorn アダプタに
  `Backend::RiscV64` + `UnicornRiscV64` ラッパーを追加。Cortex-A53 と
  SiFive U54 の CPU モデル + manifest を追加。
- **Wave 3 step 2 (commit cc75000)**: `machine/arm/virt64` (-M virt64)。
  QEMU `hw/arm/virt.c` の VIRT_MEMMAP (RAM 0x40000000, GICv3 0x08000000,
  PL011 0x09000000, virtio-mmio 0x0a000000+)。ボードはカタログからのみ
  組成、UnicornArm64 で arm64 Image ヘッダを解釈して PC/X0 を設定。
  GICv3 + PLIC に `inputs:<intid>` / `inputs:<source_id>` 配線面を追加、
  モデルの `on_signal` が (source, signal) → INTID/source-id を検索。
  `machine/manifest::parse_soc` を全機械共有化、stm32f103 のパーサを
  include_str! + parse_soc に簡素化。
- **Wave 3 step 3 (commit a8084dc)**: `machine/riscv/virt` (-M riscv-virt)。
  QEMU `hw/riscv/virt.c` の VIRT_MEMMAP (RAM 0x80000000, CLINT 0x02000000,
  PLIC 0x0c000000, PL011 0x10000000, virtio-mmio 0x10001000+)。ボードは
  カタログからのみ組成、UnicornRiscV64 で a0/a1/PC を設定。`arch-riscv`
  feature で unicorn の RV64 バックエンドを有効化、`assemble::EngineMap`
  に UnicornRiscV64 実装を追加。

### 検証

- 各 step で `cargo build --locked --workspace --all-features` が緑。
- 各 step で `cargo test --locked --workspace --all-features` が全パッ
  ケージ緑 (705→712 テスト)。
- 各 step で `cargo run -p mosaic-xtask -- check` が OK (19 クレート、
  manifest ミラー・語彙・ID 一意性検査込み)。
- AArch64 / RISC-V の実 OS 起動は Unicorn 2.1.x に外部割込み注入 API
  が無いため machine-pending (ADR 0009)。WFI 停止は「waiting-for-
  interrupt」として停止理由に明記、合成イメージで代替しない。
