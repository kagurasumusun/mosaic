# 0010: ヘテロジニアス・マルチコア実行（per-master ビュー、共有ホスト RAM、固定順ラウンドロビン）

状態: 暫定採用（P3）。根拠: 構成設計書 §2, §12.1–§12.5, §13.1–§13.2, §19 T8, §22 T3。
ADR 0002（アドレス空間）、0003（仮想時刻）、0007（Unicorn と実行契約）、
0009（IRQ ライン）を継承します。本書は `CpuEngine` への既定メソッド追加と
runtime/assemble の公開 API 追加という破壊的になりうる変更を伴うため、
R8 に基づき判断を記録します。第 2 の構成（本格 SMP や他の ISA 組み合わせ）が
検証するまで暫定扱いです。

## 背景

P3 の終了条件は「決定論を保ったまま異種構成が起動する」ことです（§22 T3）。
設計書 §12.3 はルールを定めます: (1) コアごとに独立したエンジン実例、
(2) 共有 RAM は全コアから同一のホストメモリ、(3) MMIO の副作用はイベントを
生むだけで実行中のエンジンに再入しない、(6) backend の `arch_*` feature が
ISA 集合を絞る。また §2/§12.1 の通り CPU は多種多様（ARM/AArch64/RISC-V/
MIPS/PPC/SPARC/x86/M68K/S390X/TriCore）でなければなりません。

Unicorn 2.1.x には実例間の共有メモリ・プリミティブがありません。そこで
`uc_mem_map_ptr`（呼び出し側がバッファを管理する C API）を検討し、ctypes
プロトタイプ（venv の unicorn 2.1.4）で次の**2 点**を実験的に確認しました。

1. 2 つの `Uc` 実例が同一のホストバッファを 0x10000 に写像すると、片方の
   **読み**が他方から見える。
2. しかし `uc_mem_map_ptr` の写像は**ゲストからの書き込みに耐えない**。
   A32 の `STRB` と RV32I の `SW` は `UC_ERR_WRITE_UNMAPPED` で fault する
   （`uc_mem_write` によるホストからの書き込みは可能）。

つまり素朴な「同一ホストメモリを各インスタンスへ写像」は、Unicorn 2.1.x では
**書き込み可能な共有 RAM にならない**（§24.1 の【要検証】項目の答え）。
Rust バインディングには CPU モデル選択の ctl も公開されていないため、
`sifive-e31` などのモデル名は宣言的にのみ記録します。

## 決定

### 1. contract の語彙（`mosaic-contract-cpu-engine`）

- `EngineArch`（`ArmA32Le`, `RiscV32Le`。将来の AArch64/x86/MIPS/… は
  variant 追加で拡張）と `CpuDescriptor { arch, model: Option<&'static str> }`
  を追加します。catalog の cpu クレートは `pub const CORE: CpuDescriptor` で
  実行構成を宣言します（§12.1: cpu = ISA + エンジン設定 + コア固有部分）。
- `take_bus_fault()` と `line_edges()` は**既定メソッド**として追加します。
  contract は no_std・alloc なしの境界を保つため required メソッドを増やさず、
  既存後端の破壊的変更を避けます（bus fault の記録とライン履歴の閲覧は
  ADR 0009 の経路を再利用します）。

### 2. 共有 RAM は正本 + エンジンごとのミラー（量子境界で同期）

- `SharedRam` は**正本**のバイト列を `Arc` で所有します。エンジンは
  書き込み可能な**ミラー**（Unicorn の managed memory）として同じページを
  持ち、量子境界で正本と往復します（`CpuEngine::sync_shared_in` /
  `sync_shared_out`、既定メソッド）。
- 固定順ラウンドロビン（§5）では同期位置が固定なので観測系列は決定論的
  のままです。書き込みは次のコアのスライスで反映されます。ドアベル
  ハンドシェイク（ Clemens 応答を読むのは doorbell 到着後）には十分です。
- 書き込み可能な共有メモリを持つバックエンド（自前エンジン、future
  Unicorn 改善）は同じ contract を実メモリで実装でき、束ね側は変わりません。
- FlatView の共有領域は通常の `MemoryKind::Ram` の葉として宣言し、束ね役が
  `SharedRegion { node, ram }` で「このノードはこのバッファ」と結び付けます。
  アドレス空間の真実の源は FlatView のままです（ADR 0002）。

### 3. per-master FlatView（`mosaic-core-runtime::Machine`）

- `Machine::add_view(master, view)` でコアごとのビューを登録します。
  `bus_read`/`bus_write` は `attr.master` でビューを引き、未登録マスターは
  既定ビューに落ちます。`verify()` は既定ビューと全登録ビューを検査します。
  同じアドレスがマスターごとに別のノード（別イメージ・別デバイス集合）へ
  解決でき、AMP の私有 ROM/RAM と共有窓を同時に表現できます（§12.4）。

### 4. 写像 API は assemble の `EngineMap` trait（contract に入れない）

- `attach_bus` / `map_mmio` / `map_shared_memory` は束ね役だけが使う操作であり、
  `CpuEngine` contract は実行と観測の語彙に保ちます。Unicorn の両アダプタ
  （`UnicornArm`, `UnicornRiscV32`）が `EngineMap` を実装します。
- 例外は共有メモリの**同期**で、これは実行の一部（各スライスの前後に
  呼ばれる）なので contract の既定メソッド
  （`CpuEngine::sync_shared_in` / `sync_shared_out`）として置きます。
  既定は no-op で、共有メモリを持たないエンジンは変更不要です。

### 5. スケジューラは固定順ラウンドロビン（`mosaic-assemble::MulticoreDriver`）

- 1 ラウンド = 全コアを**追加順（= 固定順）**に 1 量子ずつ実行し、各スライスの
  後に期限到来イベントを配送し、そのコアの IRQ メールボックスを
  flush します（ADR 0009 の受信経路をコアごとに持つ）。
- `step_clocked` はコアごとの解決済みクロック周期（ps/instruction）で
  「消費命令数 × 周期」だけ共有仮想時刻を進めます（§13.2 の暫定形。
  クロックツリー解決は後続フェーズ）。
- 決定論の根拠: 同一のコア順・量子・イベントキューなら観測可能系列
  （コンソール、共有 RAM、MHU 状態、コアごとの IRQ エッジ履歴）は
  量子の形に依存しません。`apps/multicore` が 2 形のスケジュールで
  同一黄金出力を検証します（§19 T8, §22 T3）。
- スレッド並列は採用しません。並列実行は配送順が実行時タイミングに依存し、
  T8 を壊します（§12.5: 決定論は並列度より優先）。

### 6. 多 ISA への拡張経路

- ISA 語彙 crate を ISA ごとに置きます（`framework/isa/arm` に続き
  `framework/isa/riscv`: `Register X0..X31 + Pc` → `RegId`、ABI 別名）。
- エンジン crate は backend を内部 enum で持ち、`arch_*` feature で ISA 集合を
  絞ります（`arch-riscv` → `UnicornRiscV32`）。ラッパ型はマクロで生成し、
  アダプタごとの重複を排除します。
- 新アーキテクチャの追加 = isa 語彙 crate + backend variant + cpu 宣言 crate
  の 3 点で、contract・runtime・スケジューラは無変更です。

### 7. メールボックス IP（`catalog/ip/arm/mhu`）

- Arm CoreLink MHU（Juno 級、3 ドアベル・バンク）を STAT/SET/CLEAR の
  レベル意味論で実装します。バンク番号 = `SignalId`、受信コアの
  `EngineIrqSink` へ runtime 配線で届けます。コア間連携は
  「共有 RAM + ドアベル + 固定順スケジューラ」の合成であり、専用機構を
  追加しません（§12.4）。

## 帰結

- 得たもの: 異種 3 コア（ARM926EJ-S ×2 + RV32I ×1）が 1 つの共有ページと
  MHU ドアベルで協調し、量子形によらず同一の観測系列に収束する
  実証（`apps/multicore`、CI golden test）。観測値:
  `console="smp=61\namp=bf\n"`, `shared=[21 61 40 BF]`,
  `mhu_stat=[0 0 0]`, `edges c0=TFTF c1=TF c2=TF`。
- 制限: 固定順 = 決定論的優先度であり真の SMP タイミングではありません。
  共有 RAM は量子境界で同期される共有ウィンドウであり、キャッシュ・バス
  競合はモデル化しません。Unicorn 2.1.x では書き込み可能なホスト共有
  メモリが存在しないため、ミラー方式が唯一の選択肢でした。
  RV32I は基本整数命令の部分集合で検証し、M/A/C 拡張は backend 能力に
  従って段階的に広げます。`CpuDescriptor.model` は宣言のみで、
  バインディングが CPU モデル ctl を公開した時点で実行に接続します。
- 却下した代替: QEMU 複数ソケット（ADR 0007 で Unicorn を採用済み）、
  コアごとのスレッド並列（非決定論）、TCG 自前実装（設計書通り最後、P5+）。
