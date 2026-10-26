# 0022: ゲストイメージによるテスト方針

状態: 採用。`tests/`（`mosaic-tests-boot`）が機械レベルテストの正本。
旧テスト（`apps/*/tests/` の OS 別個別テスト、合成イメージ fixture）は
2026-10 の再設計で**全て廃棄**した。

根拠: 設計書 §6（`tests/` は機械レベルテストの居場所）、§19（T5 machine）。
オーナー指示「OS 別のテストを個別に書くな。QEMU みたく本物の OS イメージを
選んで起動できるようにしろ。自前の合成イメージは使わない」。
参照モデルは QEMU の `tests/functional`（アセット取得とスキップ）、
`tests/qtest`（デバイスレベル）、`tests/boot-serial-test.c`、および
Microsoft の Windows Validation OS / QTest の「実イメージだけを試験対象に
する」という姿勢。

## 廃棄したもの（と理由）

| 旧テスト | 実態 | なぜ廃棄か |
| --- | --- | --- |
| `apps/vm-runner/tests/boot_install.rs` 50 件 | 合成 ISO のボリューム ID を書き換え、50 OS が「起動した」と主張 | 実イメージでも合成イメージでもなく、何も検証していない |
| `machine_pc.rs` の `run_generic_pc_vm` / `run_generic_arm64_virt_vm` | NOP + 無限ループの 5 バイトを「起動」と呼ぶ合成 VM | VM の実行ではなく関数の実行 |
| OS 別のテスト関数群 | OS 名ごとにしか意味のない個別コード | テストの構造が「OS の列挙」になると、画像の追加がコード追加になる |

## 新しい構造

```
tests/
├─ images.toml        # 台帳: 1 行 1 実イメージ（唯一のデータソース）
├─ assets/            # リポジトリに同梱する実イメージ（tinycore のみ）
├─ src/lib.rs         # 汎用ハーネス: 解決→起動→主張 の 1 ループ
├─ src/sha256.rs      # 内製 SHA-256（NIST ベクトル検証済み）
└─ tests/boot_matrix.rs   # OS ファミリごとの薄いループ
└─ tests/front_end.rs     # mosaic-system CLI 自体の end-to-end 検証
```

**テストコードは OS を知らない。** `linux`/`windows`/`android`/`rtos`/
`other` の各カテゴリは同じ `boot_case` を回すだけ。OS を増やすのは
`images.toml` に行を足すことであって、テストコードを書くことではない。

## イメージの調達（QEMU のアセット管理に倣う）

| `source` | 意味 | ハーネスの動作 |
| --- | --- | --- |
| `committed` | `tests/assets/` に同梱（再配布可能なものだけ） | 必ず検証 |
| `download` | `cargo xtask images fetch` が URL から取得し sha256 を検証。zip は `archive-member`/`archive-sha256` で中身を取り出す | 無ければ**スキップ（理由つき）**、あれば検証 |
| `manual` | ライセンス上、運用者が自分で用意（Windows 商材、ローカルビルド） | 無ければスキップ、あれば検証（sha256 未ピンなら検証のみ省略） |

`download` で sha256 が無い行は台帳のパース時点でエラー。**検証できない
ダウンロードはテスト資産ではない**ため。digest 不一致は失敗（スキップで
ない）: 違うバイト列を黙って起動するのが嘘の始まりだから。

## 状態は嘘をつかない

| `status` | 意味 |
| --- | --- |
| `asserted` | `expect` パターンは完全なブート契約。取りこぼしは失敗 |
| `bring-up` | 実イメージ・実起動。パターンは「そのマシンがそのイメージで既に到達したマイルストン」。マシンが育ったら前へ動かす。**削除は禁止** |
| `machine-pending` | 必要なマシンが未実装。理由を添えてスキップ |

bring-up のエントリは開発台帳である: どこまで行けていて、次に何が要るかが
`note` に書いてある（例: 「タイマー IRQ の注入が次の隘路」）。

## 実測（2026-10-27 時点のマイルストン）

| イメージ | 到達点 |
| --- | --- |
| Tiny Core 16 Core (CD) | ISOLINUX 4.05 バナー、CD 読み取り |
| Tiny Core 16 カーネル直接起動 | カーネル自身の `Linux version` バナーがシリアルへ、WP ビット自己診断 `Ok.`、メモリマップ受理まで |
| Tiny Core 16 CorePure64 (x86_64) | ISOLINUX バナー（64 ビットカーネルを 16 ビット BIOS から） |
| Alpine 3.21 virt x86/x86_64 | ISOLINUX バナー |
| SliTaz rolling core | ISOLINUX バナー |
| Debian 6.12 amd64 netboot | 実モード setup、EDD 探査、64 ビット入口まで（`-cpu haswell` が必要） |
| FreeDOS 1.3 ブートフロッピ | **DOS カーネル完全起動**: KERNEL.SYS 7.10 が言語選択メニューとカウントダウンを実行 |
| ReactOS 0.4.16 CD | `Starting FreeLoader...` バナー |
| Android-x86 4.4-r5 | ISOLINUX 4.04 バナー |

## CI との関係

CI はイメージを取得しない。`test-images/` が空の状態で `cargo test` を
実行するため、`committed` のエントリだけが走り、`download`/`manual` は
「画像が無い」旨のスキップログを出して通過する。 fetched 環境では
`cargo xtask images fetch && cargo test` で全実イメージ分が走る。
