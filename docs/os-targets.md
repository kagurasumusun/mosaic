# ゲスト OS 目標マトリクス

オーナー目標: **Linux / Android / Windows / RTOS がエラーなく
boot・install・起動・操作できること。優先アーキテクチャは無く、
すべてのアーキテクチャを対象とする。** CPU エミュレーションは
自前実装せず Unicorn Engine を使う。

この表は目標の網羅管理と現状の台帳である。現状の詳細は
`tests/images.toml`（実イメージの台帳）と ADR 0022（実測マイルストン）
を参照のこと。各行は「実イメージを指定して起動する」という同じ操作の
対象であり、OS 別の個別テストコードは存在しない。

凡例: ◎ 到達済（その行のマイルストンを CI で主張） / ○ 実行中
（実イメージが起動し、開発継続中） / △ 入手・方針確定・着手待ち /
× 未着手

## Linux（オーナー指定 8 系統 × 全 CPU アーキテクチャ）

| 系統 | x86 | x86_64 | aarch64 | armv7 | riscv64 など |
| --- | --- | --- | --- | --- | --- |
| Tiny Core | ◎ CD/カーネル直接起動 | ◎ CorePure64 | △ | △ | △ |
| Alpine | ○ ISOLINUX 到達 | ○ ISOLINUX 到達 | △ 画像 pinned・virt-64 待ち | △ 画像 pinned・versatile-pb 待ち | △ |
| SliTaz | ○ ISOLINUX 到達 | △ | △ | △ | △ |
| Damn Small Linux | △ manual | △ | △ | △ | △ |
| Puppy | △ manual | △ manual | △ | △ | △ |
| Arch Linux | △ manual | △ manual | △ | △ | △ |
| Debian | △ | ○ netboot カーネルが 64 ビット入口まで | △ arm64 netboot 画像で追従 | △ | △ |
| Buildroot | △ qemu_x86_defconfig で自作・手動調達 | △ 同左 | △ | △ | △ |

次の隘路（Linux 全般）: PIT 由来のタイマー IRQ 注入（Unicorn 2.1.x は
外部割込み注入を持たないため、ADR 0009 の IRQ line 契約を x86 でも通す）、
ACPI テーブルの提供、VGA コンソールをカーネルが見つけた後の入力。

## Android（オーナー指定: 1.6〜4.4）

| 対象 | x86 | ARM (goldfish) |
| --- | --- | --- |
| Android-x86 4.4-r5 | ○ ISOLINUX 4.04 到達 | — |
| Android 4.4 AOSP | — | × goldfish マシン（ARMv7 + goldfish デバイスモデル）を vm に実装 |
| Android 1.6〜4.0 実機系 | — | × 同上（goldfish が最短経路） |

Android 1.x〜3.x は x86 正規ポートが存在しないため、ARM goldfish 系
デバイスモデル（goldfish tty, events, battery, framebuffer）の実装が
本命。PCI/USB が揃えば Android-x86 側もカーネルハンドオフから進む。

## Windows 系（オーナー指定: 95/98/2000/2008/XP/WinPE/ReactOS/WinCE）

| 対象 | 現状 | 調達 |
| --- | --- | --- |
| ReactOS 0.4.16 CD | ○ `Starting FreeLoader...` 到達 | download（台帳 pinned） |
| Windows 95/98 ブートフロッピ | △ フロッピ機構は FreeDOS で実証済み | manual（商材のため運用者が用意） |
| Windows 2000 / XP / Server 2008 | △ | manual |
| WinPE（ADK 製） | △ | manual（ADK の MakeWinPEMedia で作成可） |
| Windows Validation OS | △ | manual（Microsoft 配布） |
| Windows CE (NK.bin) | × loadlinux 風ローダが | manual（Platform Builder ビルド） |

ReactOS が現在唯一の「自由に配布できる Windows 系実イメージ」であり、
FreeLoader が要求する BIOS サーフェス（INT 13h 拡張の追加関数、VESA、
PCI) を実装する主戦場になる。

## RTOS

| 対象 | 現状 |
| --- | --- |
| FreeRTOS x86 QEMU デモ | × ELF 直接起動（`-kernel` の ELF 対応）を vm に足す |
| Zephyr qemu_x86 | × 同上。`west build -b qemu_x86` で自作 |

## マシン別の実装状況

| マシン (`-M`) | ISA | 状態 |
| --- | --- | --- | 
| `pc` | i386 / x86_64 | 実装済み（BIOS INT 暫定、ポート I/O、VGA テキスト、El Torito、i386 ブートプロトコル、`-cpu` モデル選択） |
| `virt-64` | aarch64 | 未実装（pl011 + GICv2 + FDT 生成、Alpine aarch64 画像は pinned 済み） |
| `versatile-pb` | arm (ARM926EJ-S) | 未実装（カタログの arm926ej-s / pl011 / pl190 を組むだけの縦スライス待ち） |
| `goldfish-arm` | armv7 | 未実装（Android 1.6〜4.4 ARM の受け皿） |

## 開発手順（オーナー指定の反復）

1. 実 ISO/フロッピイメージを入手し `tests/images.toml` に pin する
   （`cargo xtask images fetch`）。
2. `mosaic-system -M pc -cdrom <image>` で起動し、停止点の報告書を読む。
3. 停止の原因（未対応の BIOS コール、未実装デバイス、未注入の IRQ）を
   カタログモデルまたはマシン側に実装する。
4. マイルストンを `images.toml` の `expect` に前へ動かす（戻さない）。
5. 1 へ戻る。
