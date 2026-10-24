# 0012: T32 実行のための M-class エンジン構成

状態: 暫定採用（P3 vic-thermal で A32 側の限界を実証済み、M-class 側は
STM32F103 スライスが契約を検証するまで暫定）。根拠: 設計書 §12.3、ADR 0007、
ADR 0009、ADR 0010。

## 背景

次段階のボードスライスとして STM32F103（Cortex-M3）を計画している。
Cortex-M3 は T32 (Thumb-2) 専用であり、A32 を一切実行しない。このため
Unicorn アダプタが T32 を実行できることが前提になる。

P3 の VIC/熱センサ・スライスの CI で、A32 構成（`UC_MODE_ARM`）の
Unicorn 2.1.5 では T32 への切替が**どちらの経路でも機能しない**ことを
実証した。

1. `uc_reg_write(UC_ARM_REG_CPSR, .. | (1 << 5))` は成功を返すが
   実行状態に反映されない（T ビットを立てても A32 のまま実行される）。
2. `uc_emu_start(begin, ..)` の begin アドレス bit 0 も無視される
   （奇数 begin から A32 として実行が始まり、無関係な命令列を辿って
   fault する。FAQ の記述はこのバージョンでは当てにならない）。

両者とも stub ではなく実バイナリに対する CI での観測結果であり、
オフラインの stub ビルドでは再現できない種類の差異である
（ADR 0007 の「stub は網羅しない」注意の実例）。

## 決定

### 1. T32 は M-class 構成でのみ提供する

Unicorn は `UC_MODE_MCLASS` で Cortex-M3 CPU モデルを選択できる。
M-profile はリセットから常に T32 であり、状態切替という問題自体が
存在しない。よって:

- アダプタに `Backend::ArmMClass` を追加し、
  `Mode::MCLASS | Mode::LITTLE_ENDIAN` で構成する。
- 公開ラッパーは `UnicornCortexM3`（`unicorn_wrapper!` で生成）。
  ISA 語彙は A32 と同じ `mosaic-isa-arm` を使う。
- `run()` の begin は PC をそのまま使う（bit 0 合成は A32 専用の
  慣用句）。CPSR 読み出しを行わないため、M-profile の XPSR 互換性に
  依存しない。

### 2. PC 整列は構成ごとに規定する

- A32 (`UnicornArm`): 従来どおり 4 バイト整列を必須とし、奇数値
  （BX 形式の T32 移行要求）は `InvalidValue` で拒否する。以前試みた
  「PC bit 0 で T32 へ切替」は本バックエンドでは動作しないため、
  黙って A32 として実行するより失敗が好ましい。
- M-class (`UnicornCortexM3`): 半語整列（偶数値）を許可する。奇数は
  拒否（M-profile では実行状態の意味を持たない）。

### 3. 割り込みライン語彙

ARMv7-M は FIQ 入力を持たず、外部割り込みは NVIC 経由である。
`set_line` の受理ラインは `IrqLine(0)`（maskable exception 入力）のみと
し、`IrqLine(1)` は構成エラーとして扱う。注入自体は ADR 0009 の
能力モデル（記録のみ）を維承する。

## 他案

- **A32 構成のまま CPSR.T を書く / begin を奇数にする**: 上記のとおり
  Unicorn 2.1.5 で機能しない。将来のバージョンで修正されても、
  構成時に実行 ISA が確定している方が契約（整列規則、caps）を単純にできる。
- **`UC_MODE_THUMB`（ARM926 のまま Thumb 開始）**: Cortex-M ボードの
  実モデル（SysTick・NVIC・ベクタテーブル前提）と乖離するため不採用。
  ボードスライスの目的は実チップの再現であり、CPU モデルも本物を選ぶ。
- **uc_ctl での CPU 選択（`uc_cpu_arm` 系 API）**: 本バインディング
  （unicorn-engine 2.1.5）が公開していないため利用できない。

## 検証

- `mclass_backend_executes_t32_from_reset`: M-class 構成でリセット直後
  から T32 命令列が実行できること、半語整列 PC 書込みが受理されること、
  A32 側が引き続き 2 mod 4 を拒否すること。実 Unicorn が必要なため
  CI の `--all-features` test ステップで検証する。
- STM32F103 スライス（次段階）が end-to-end の検証となる。

## 退出条件 / 次の作業

- STM32F103 スライス: NVIC（ARMv7-M 割り込みコントローラ）、RCC、GPIO、
  flash エイリアスを含む実モデルと、`bind_flat_view_paged` による
  ビットバンド領域の表現。
- T32 ファームウェアの isa 語彙（必要になれば A32 と分離した encoding
  ヘルパ）の整備。
- 現行 vic-thermal の A32 系は本変更の影響を受けないことを CI の
  ゴールデンテストが保証する。
