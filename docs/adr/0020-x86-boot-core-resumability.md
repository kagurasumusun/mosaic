# 0020: x86 ブートコアと再開可能性の限界

状態: 暫定採用。PC クラスのゲストは `UnicornX86Boot` で実行する。
例外の IDT 配送、ページング下のデバイス、コンソールは未対応（下記「残る穴」）。

根拠: ADR 0018、ADR 0019。実測は `framework/engine/unicorn/src/lib.rs` の
`x86_boot_core_resumes_across_a_real_mode_far_jump` と `apps/vm-runner`
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

- **Unicorn は例外をゲストの IDT で配送しない。** `UC_HOOK_INTR` が無い場合は
  halt、あっても Unicorn 自身は `IDT` を引かない。今はサービスが答えない
  ベクタで量子を止め、`PcBootReport::unclaimed_interrupt` に番号を残す。
  保護モードのゲストが自分の IDT でページフォルトを処理するには、フレームを
  積んでゲートへ飛ぶ配送をエンジン側で実装する必要がある。
- コンソール。`boot_params.screen_info` を埋めていないので `vgacon` は
  名乗り出ず、シリアルも 8250 が見つからず、カーネルの `printk` はどこにも
  出ない。次はここ。
