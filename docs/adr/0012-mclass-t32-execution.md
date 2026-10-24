# 0012: T32 実行のための M-class エンジン構成

状態: 暫定採用（P3 vic-thermal の CI で A32 側の限界を実証、M-class 側は
STM32F103 スライスが契約を検証するまで暫定）。根拠: 設計書 §12.3、ADR 0007、
ADR 0009、ADR 0010。

## 背景

次段階のボードスライスとして STM32F103 相当（Cortex-M3 クラス、T32 専用）を
計画しているため、Unicorn アダプタが T32 を実行できることが前提になる。

Unicorn 2.1.5（タグ 2.1.4 のソースで確認）の ARM 実装において、実行状態の
管理は次のとおり正確に把握した。

- `uc_reg_write(UC_ARM_REG_PC, v)` は `env->thumb = v & 1` を直接書く
  （`qemu/target/arm/unicorn_arm.c` の R15 ケース）。bit 0 が公式の
  Thumb 選択経路であり、`uc_emu_start` の begin も同じ書込み路径で
  bit 0 を再適用する。
- `UC_MODE_THUMB` を `uc_open` に渡すと `uc->thumb = 1` がセットされるが、
  このフィールドは **どこからも読まれない**（デッドコード）。FAQ の
  「mode で Thumb」という説明はこのバージョンでは機能しない。
- `UC_MODE_MCLASS` は CPU モデルとして **Cortex-M33** を選択する
  （`qemu/target/arm/cpu.c` `cpu_arm_init`）。ただし M-profile の
  リセットはベクタテーブル 0x0 から初期 PC と T ビットを読む。構築時点で
  0x0 は未マップのため T=0 で起動し、T32 命令はすべて未定義例外に
  なる（CI で fault として観測）。
- CPSR 書込みは `cpsr_write(env, v, ~0, CPSRWriteByUnicorn)` に届き、
  T ビットも適用されて翻訳フラグも再構築される。ただし**その後の
  偶数 PC 書込みは `env->thumb` を 0 に戻す**。最初のアダプタ実装は
  この順序（CPSR.T を書いてから整列済み PC を書く）で T ビットを自分で
  消していた。これが「CPSR.T を書いても A32 のまま」に見えた正体である。

## 決定

### 1. T32 は M-class 構成で提供する

- アダプタに `Backend::ArmMClass` を追加し、`Mode::MCLASS |
  Mode::LITTLE_ENDIAN` で Cortex-M33 を選択する。
- 構築直後に CPSR の T ビットを書く（読み出した CPSR に 0x20 を OR）。
  M-profile リセットの T=0 を確実に上書きする唯一確実な公式経路。
- `run()` の begin は `pc | 1` を渡す。`emu_start` が begin を R15 経路で
  書き戻すたびに `env->thumb` が再 asserting され、整列済み PC 書込みが
  一時的に T を消しても構築時の状態が保たれる。
- 公開ラッパーは `UnicornCortexM3`（`unicorn_wrapper!` で生成）。ISA 語彙は
  A32 と同じ `mosaic-isa-arm` を使う。

### 2. PC 整列は構成ごとに規定する

- A32 (`UnicornArm`): 従来どおり 4 バイト整列を必須とし、奇数値は
  `InvalidValue` で拒否する。A32/T32 混在（BLX 遷移）を必要とする
  スライスは現時点で存在せず、奇数 PC の受理は誤実行の温床になるため。
  必要になった場合は PC 書込み（整列）→ CPSR.T 書込みの順序で拡張する。
- M-class (`UnicornCortexM3`): 半語整列（偶数値）を許可する。奇数は拒否
  （run() が常に bit 0 を補うため、ゲストが指定する意味がない）。

### 3. 割り込みライン語彙

ARMv7-M は FIQ 入力を持たず、外部割り込みは NVIC 経由である。
`set_line` の受理ラインは `IrqLine(0)`（maskable exception 入力）のみと
し、`IrqLine(1)` は構成エラーとして扱う。注入自体は ADR 0009 の能力モデル
（記録のみ）を継承する。

## 他案

- **A32 構成のまま PC bit 0 / CPSR.T で切替**: 機構上は可能だが、偶数 PC
  書込みが暗黙に T を落とすため、ドライバの再開経路と競合する。
  構成時に実行 ISA が確定している方が契約（整列規則、caps）も単純。
- **`UC_MODE_THUMB`**: 上記のとおりデッドコードであり信頼できない。
- **uc_ctl での CPU 選択（`uc_cpu_arm` 系 API）**: 本バインディング
  （unicorn-engine 2.1.5）が公開していないため利用できない。
- **M33 でなく M3 を選びたい**: unicorn 2.1.x は MCLASS から M33 しか
  選べない。M3 と M33 の T32 ベースライン（Thumb-1/Thumb-2）は
  本プロジェクトの用途（実レジスタモデルのボードスライス）で同等に
  扱え、差異（TrustZone、v8M 追加命令）はゲストコード側で使わない
  限り観測されない。不足が生じたら後端差し替えで対応する。

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
