# 0021: システムフロントエンド（QEMU 型コマンドライン）

状態: 採用。`mosaic-system`（`apps/system`）が仮想マシンの実行入り口であり、
それ以外の入り口を増やさない。

> **2026-10-27 修正(ADR 0024)**: バイナリは `apps/system` から
> `mosaic/`（bin 名 `mosaic`）へ移り、`apps/cli` と縦スライス用デモ
> アプリ群は削除された。入り口は引き続き 1 本だけ。`-M stm32f103`
> が機械レジストリに登録されている（実イメージは `-kernel` で
> ユーザが提供する）。

根拠: 設計書 §6（`apps/`）、§14（記述と組立）、オーナー指示
「テストと入り口だけ再設計。プロジェクト全体の設計は維持」。
参照実装は QEMU のコマンドライン（`qemu-system-*.1`）。

## 背景

これまで実行の入り口はアプリごとにばらばらだった（`apps/vm-runner` は
main で固定の合成イメージを回すだけ、テストは `run_generic_pc_vm` のような
合成 NOP ループを呼ぶだけ）。オーナーの指摘通り、これは「システムエミュ
レータの実行」ではなく「関数呼び出しの演示」であり、OS イメージを選んで
起動するという VM アプリ本来の操作に反していた。

## 決定

1. **入り口は 1 つ**: `mosaic-system <options>`。QEMU が `-M` でマシンを、
   `-m` で RAM を、`-cdrom`/`-hda`/`-fda` で実イメージを、`-kernel`/
   `-initrd`/`-append` で直接カーネル起動を、`-serial`/`-display` で出力を
   選ぶのと同じ語彙を採用する。`--instruction-limit` と `--quantum` は
   Mosaic の決定論実行の予算に相当する（QEMU の `-icount` の役割）。

2. **マシンレジストリ**: `MachineKind`（現在は `pc`）が `-M` の選択肢。
   マシンの追加はここへの追加であり、新しいアプリを作らないこと。
   `virt-64`（AArch64）や `versatile-pb`（ARM32）、`goldfish-arm` は
   実イメージが起動できるようになってから登録する（ADR 0022 の台帳参照）。
   （2026-10 修正・ADR 0023: 「ここへの追加」はディスパッチ 1 arm のことで
   あって機械コードを `apps/system` に置くことではない。機械モデルは
   `catalog/board/<origin>/<board>/glue` に置き、`pc` は
   `catalog/board/ibm/pc`（`mosaic-board-ibm-pc`）に移設済み。
   `PcBootOptions`/`PcBootReport` は同クレートからの再エクスポート。）

3. **合成イメージの禁止**: フロントエンドはイメージのバイト列しか受け取らず、
   何も合成しない。イメージがなければ操作エラーとして拒否する。旧
   `machine_pc.rs`（NOP ループを回すだけの「汎用 VM」）と旧
   `tests/boot_install.rs`（ボリューム ID を書き換えただけの合成 ISO で
   50 OS の起動を主張していた）はこの方針で削除した。

4. **報告書が第一級**: `--report` は JSON で停止理由・命令数・画面・シリアル・
   未処理割込み・例外・エンジンエラーを出す。テストハーネス（ADR 0022）と
   人間の両方が同じ報告書を読む。

5. **ドライブ番号は PC 流**: フロッピは `0x00` から、固定ディスクは `0x80`
   から、CD は El Torito 規約の `0x9F`。`-boot dca` の各文字はメディア種を
   指し、最初にブートレコードを持つイメージが選ばれる。

## CPU モデル（`-cpu`）

配布カーネル（Debian 6.12 等）は `x86-64-v2` 向けにビルドされており、
バックエンド既定の `qemu64` 相当コアは BMI2 を持たないため、最初の
`SHLX` で命令例外になる（実測: `INSN_INVALID` at `SHLX`）。
`UnicornX86Boot::new_as(X86CpuModel::Haswell)` で解決する。
実測上、Unicorn の `UC_CTL_CPU_MODEL` は**フックを 1 つでも登録した後は
失敗する**ため、モデルはコンストラクタ引数であってセッターではない。
既定は `haswell`。

## 結果

- `mosaic-system -cdrom tinycore.iso` が ISOLINUX バナーまで実行し、
  `mosaic-system -kernel os.iso\!boot/vmlinuz -initrd os.iso\!boot/core.gz`
  が Linux カーネルをシリアルコンソールまで起動する。
- 実行の入り口が 1 本に整理され、ADR 0022 のテストはこの CLI と同じ
  ライブラリ API を通じて実イメージを起動する。
