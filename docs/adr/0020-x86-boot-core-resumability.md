# 0020: x86 ブートコアと再開可能性の限界

状態: 暫定採用。PC クラスのゲストは `UnicornX86Boot` で実行する。
例外の IDT 配送は実装済み（下記「更新: 例外配送」）。ページング下のデバイスと
コンソールは未対応（下記「残る穴」）。

根拠: ADR 0018、ADR 0019。実測は `framework/engine/unicorn/src/lib.rs` の
`x86_boot_core_resumes_across_a_real_mode_far_jump` と `apps/system`（`mosaic-system -cdrom`）
の Tiny Core 起動。Unicorn 2.1.5 のソース（`uc.c`、`qemu/target/i386/unicorn.c`）
を直接読んで確定させた。

## 背景

カーネルを起動するとは、実モードの `setup` コードから 32 ビットの
デコンプレッサ、さらに保護モードのカーネルへと、**同じゲストを量子ごとに
停止・再開しながら**渡り歩くことである。Unicorn の `UC_MODE_16` はこの
再開に耐えない。

`uc_emu_start(uc, begin, ...)` は x86 では `begin` を「実行開始アドレス」では
なく**線形アドレス**として扱う:

```c
case UC_MODE_16: {
    uint16_t ip;
    uc_reg_read(uc, UC_X86_REG_CS, &cs);
    ip = begin - cs * 16;          /* 16 ビットに切り詰め */
    uc_reg_write(uc, UC_X86_REG_IP, &ip);
}
```

- 再開位置は `ip = begin - cs * 16` で、`begin` に「セグメント内オフセット」を
  渡すと `ip` が `offset - cs * 16` に化ける（`cs = 9020h` なら 200h ずれる）。
  実測では `EIP 0` に再開したはずのゲストが `EIP FE00h` から実行を始め、
  `A0000h` で `FETCH_UNMAPPED` になった。
- `ip` は `uint16_t` なので、**64 KiB を超える位置に再開できない**。
  保護モードのデコンプレッサは `10007Ch` で `rep movsl` を回す。ここで
  量子を切ると `10007Ch` ではなく `FF7Ch` に戻り、以降の実行は実在しない
  コードになる。これが「`0xFF7C` の無限ループに見えた」正体である。

Unicorn 2.1.5 のモード語は 3 つしかなく、うち `UC_MODE_32` は初期化で
`CR0.PE` を**立てる**（`cpu_x86_update_cr0(env, CR0_PE_MASK)`）ため、そのままでは
実モードから始められない。

## 決定

- `Backend::X86Boot` / `UnicornX86Boot` を追加する。Unicorn のモード語は
  `UC_MODE_32`（したがって `uc_emu_start` は EIP をそのまま書く）、初期状態は
  実機のリセット直後:
  - `CR0` に 0 を書いて `PE` を落とす。
  - `CS`/`DS`/`ES`/`SS`/`FS`/`GS` にセレクタ 0 を書く。`PE = 0` では
    `cpu_x86_load_seg` が base = selector * 16、limit = FFFFh、16 ビットの
    コードセグメントを計算するので、これだけで実モードのセグメントキャッシュに
    なる。
- `run()` の再開アドレスは、実モード（`CR0.PE = 0`）なら EIP の下位 16 ビット、
  保護モードなら EIP そのもの。モード語が 32 ビットなので `begin` は
  どちらの場合も「セグメント内オフセット」でよい。
- `UnicornX86`（`UC_MODE_16`）は残すが、**64 KiB に閉じたゲスト専用**と
  みなす。カーネル起動経路は使わない。
- 量子の予算は `emu_start` の `count` ではなくコードフックで数える。
  `count` は `rep movs` の途中で止まり得て、その文脈は再開できないため。
  フックは「(limit + 1) 命令目に入る直前」に `emu_stop()` するので、退役する
  命令数はちょうど `limit`。`rep` 文字列命令だけは丸ごと通し、1 命令だけ
  超過する（ADR 0018 の実測を置き換える）。

## 受け入れ基準

- 同じゲストを量子 1 で回しても量子 100 万で回しても、退役命令数と最終状態が
  一致する（Tiny Core のデコンプレッサで実測: 1,382,684 命令、停止位置一致）。
- Tiny Core の `bzImage` が自前の `setup` から 32 ビットのデコンプレッサに入り、
  `Decompressing Linux... done` と `Booting the kernel` を VGA 窓に出す。

## 実測で分かったこと（PC のメモリウィンドウ）

カーネルは起動の途中で、実機なら必ず応答するアドレスを読む。落ちた順に:

| アドレス | 何 | 対応 |
| --- | --- | --- |
| `E0000h`–`FFFFFh` | ACPI RSDP の走査範囲 | ROM ウィンドウ全体（`C0000h`–`FFFFFh`）をゼロで復号する |
| `A0000h`–`BFFFFh` | VGA アパーチャ | テキスト窓だけでなく 128 KiB 全部をマップする |
| `FEE00000h` | Local APIC | 4 KiB を読み書き可能でマップする（まだタイマーではない） |

## 残る穴

- コンソール。`boot_params.screen_info` を埋めていないので `vgacon` は
  名乗り出ず、シリアルも 8250 が見つからず、カーネルの `printk` はどこにも
  出ない。次はここ。

## 実測で分かったこと（コンソールが無くてもカーネルは読める）

`printk` はリングバッファ（`prb`、カーネルの `.data`/`.bss` の中）に
**平文**でメッセージを積む。コンソールが 1 つも登録されていなくても、
そこは読める。`PcBootOptions::dumps` で `0xB50000` 付近（i386 の
`Linux version` バナーが実際に居た場所）を指定し、印字可能な連続領域を
拾うだけで、起動ログ全文が手に入る。Tiny Core 6.6.8 の実測:

- `Linux version 6.6.8-tinycore ... #666 SMP`
- `BIOS-e820: [mem 0x100000-0x3ffffff] usable` など、ファームウェアの
  `INT 15h E820` をカーネルが受理している
- `RAMDISK: [mem 0x630000-0x125bfff]` — ローダが置いた initramfs を発見
- `Found and enabled local APIC!` — `0xFEE00000` の 4 KiB で足りた
- `Kernel command line: console=tty0 ...` — **コマンドラインは届いている**
- `ACPI BIOS Error (bug): A valid RSDP was not found` — 警告であって致命ではない

その後 `Checking if this processor honours the WP bit even in supervisor mode...`
で止まる。ここは読み取り専用ページへの書き込みでページフォルトを起こし、
例外テーブルで受け止める検査であり、**例外を IDT で配送できない**上記の穴に
そのまま突き当たる。

次にやること:

1. 例外配送。`UC_HOOK_INTR` の中で、サービスが答えないベクタについて
   ゲストの IDT（保護モード。32 ビットゲート限定）または IVT で
   フレームを積んでゲートへ飛ぶ。エラーコードを持つベクタは 8/10-14/17/21/29/30。
   `#PF` の `CR2` とエラーコードは Unicorn が露出しないので、
   `UC_HOOK_MEM_*` で自前で拾う必要がある。
2. コンソール。`screen_info` は埋めた（`orig_video_isVGA = 22h`、80x25）ので
   `vgacon` が名乗り出るはずだが、VGA の CRTC/属性/シーケンサ・レジスタ
   （`3B4h`–`3DAh`）を読めるようにしてから確認する。シリアルは
   8250 が PNP/ACPI 無しでは見つからないので、`earlyprintk` は
   `CONFIG_EARLY_PRINTK` 依存だと分かった（このカーネルでは出ない）。

## 更新: 例外配送（実装済み）と、その手前にあった 2 つの穴

`UC_HOOK_INTR` の中でサービスが答えなかったベクタを、ゲスト自身のゲートへ
配送する（実モードは `IVT`、保護モードは 32 ビットゲート）。エラーコードを
持つベクタ（8/10–14/17/21/29/30）はフレームの下に積む。`#PF` のエラーコードは
Unicorn が露出しないので、`CR2` とゲストのページテーブル（`P` ビット）と
故障命令のオペコード（`W` ビット）から再構成する。

これが動くまでに、**無関係に見える 2 つの穴**を先に塞ぐ必要があった。どちらも
「`#PF` が起きているのに配送されない」という同じ症状を別の場所で作っていた。

### 穴 1: `emu_start` の `until` を毎量子で無効化すると幽霊 `#PF` が立つ

`uc_emu_start(uc, begin, until, ...)` の `until` は「終了アドレス」であり、
このアダプタはアドレスで止まらないので `u64::MAX` を渡していた。Unicorn は
量子の終わりに `tcg_cpu_exec` から `uc_invalidate_tb(uc, until - 1, 1)` を呼ぶ。
つまり**毎量子ごとに `FFFFFFFFFFFFFFFEh` の TB を捨てようとし**、
`get_page_addr_code` がそのアドレスをゲストのページテーブルで引く。ページング
が有効なカーネルではこれは必ず `#PF`（`CR2 = FFFFFFFEh`）になり、
`raise_interrupt2` の中の `check_exception` が `old_exception = 14` を記録して
から、例外そのものは無効化側の `setjmp` に飲み込まれる。`old_exception` を
戻す経路は無いので、2 量子目で `#DF`、3 量子目で triple fault になり、
以降は `check_exception` が**すべての本物の例外を `EXCP_HLT` に変換**して
フックにすら届かない。症状は「同じ EIP で 1 命令ずつ消費して予算を使い切る」。

修正は `uc_ctl_exits_enable()`。`until` が無視され、`ctl_set_exits` で登録した
アドレスだけが無効化対象になる（登録はゼロ）。

### 穴 2: Unicorn の `uc_mem_read`/`uc_mem_write` は i386 では物理アドレス

`cpu_memory_rw_debug` は `cpu_get_phys_page_attrs_debug` ではなく
`address_space_read` に落ちる。つまり**ゲストのページテーブルを一切引かない**。
アダプタが読むもの（`IDT` エントリ、`GDT` 記述子、スタック、命令バイト）は
すべて線形アドレスなので、`c0000000h` 以上のカーネルが動き出すと全部
`UNMAPPED` になる。実測では `LGDT`/`LIDT` の観測（`0F 01` のデコード）が
ずっと失敗し、`tables.idt = None` のままだった。

修正は `x86_translate()`（2 レベル/PAE のページウォーク）を挟み、
`x86_read_bytes`/`x86_write_bytes`/`x86_modrm_operand`/`rep` 判定/記述子観測を
すべて線形アドレスとして扱うこと。ページウォークは `#PF` エラーコードの
`P` ビットと共通化した。

### 実測（Tiny Core 15 / Linux 6.6.8-tinycore, 4 億命令）

- `Checking if this processor honours the WP bit even in supervisor mode...Ok.`
  — 読み取り専用ページへの書き込みで起きた `#PF` がカーネルの
  `asm_exc_page_fault` に届き、例外テーブルのフィックスアップから
  故障命令へ戻った
- 続けて `Memory: 46060K/65144K available`、`SLUB`、`rcu:`、
  `Console: colour *CGA 80x25`、`printk: console [tty0] enabled`、
  `APIC: Switch to virtual wire mode`、`Calibrating delay loop...` まで到達
- `PcBootReport::exception` に `Some((14, 0xC021D0CD, 0xFFD38000))` が残り、
  `unclaimed_interrupt` は `None`（＝ゲストが引き取った）

次は `Calibrating delay loop` を越えるためのタイマー（PIT → PIC → IRQ0）と、
`vgacon` のための VGA レジスタ（`3B4h`–`3DAh`）応答。
