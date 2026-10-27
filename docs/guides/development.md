# P0 開発ガイド

## workspace の規則

クレートを追加するときは root `Cargo.toml` の members と、新しいクレートの
`[package.metadata.layer]` を更新します。workspace の version / edition /
rust-version / lints を継承し、ライセンス決定までは `publish = false` にします。
ライブラリの先頭には `#![forbid(unsafe_code)]` を置きます。
唯一の例外は engine/unicorn の所有権ラッパーで、deny + 局所 allow による unsafe Send を 1 箇所持ちます。
変更前に ADR 0007 の不変条件を確認してください。

```toml
[package.metadata.layer]
domain = "hw"        # 領域名 = ディレクトリ名
kind = "hw"          # domain と同じ値
level = 3            # 下の表のレベル
```

| domain (= kind) | level | 例 |
| --- | --- | --- |
| util | 0 | bits / fifo / fnv |
| exec | 1 | Addr, VTime, RegId, Snapshot |
| contract | 2 | 12 契約モジュール |
| isa | 2 | isa/arm, isa/x86（+ contract への唯一の同位依存） |
| block | 2 | ISO9660 形式 |
| runtime / accel / hw | 3 | Machine / unicorn, armv7m / デバイスモデル |
| describe | 4 | manifest → MachineGraph |
| assemble | 5 | IR → 実体、engine 結合 |
| machine | 6 | arm/stm32f103, i386/pc |
| front / tests | 7 | bin `mosaic` / 実イメージ起動テスト |
| tooling | 8 | xtask |

依存は下位レベルへのみ（例外: isa → contract の CPU モデル、tests → front）。
`hw` は `machine` を依存禁止（2 軸境界、ADR 0024）。外部クレートは
unicorn-engine が accel/unicorn のみ、toml が machine / tests / tooling のみ。

## 構成検査

```sh
cargo xtask lint-layers
cargo xtask lint-names
cargo xtask lint-catalog # CPU/IP/part/SoC/board manifest と正規化グラフを検証
cargo xtask check       # 層・命名・catalog の全検査
cargo xtask doctor      # 検査と現在の未決事項を表示
```

失敗時は終了コードが非ゼロになり、CI を落とします。
`lint-layers` は `cargo metadata --no-deps` の **依存宣言すべて**を読みます。
通常依存だけでなく dev / build / optional / 非アクティブ target の依存も検査対象です。
別名の依存も名前でなく正規化した path で照合します。
workspace 外の未分類 path 依存を拒否します。

- framework は catalog に依存しません。
- framework は原則、数値の低い層だけに依存します。
  同層は contract → contract と modelkit → contract だけを暫定許可します。
- catalog は prim / types / contract / modelkit / isa のみを利用できます。
- catalog の横依存は `ip-lib` 宛てのみ許可します。
- composite の glue は types / contract だけを利用できます。
- VM 層の外部依存は専用 engine の unicorn-engine だけを暫定許可します。
  vendor crate の直接参照を app / tooling / catalog / 他 framework に広げることは禁止します。
  その他の外部依存は xtask / apps のみが利用できます。

`lint-names` はソースディレクトリ名とクレート名を検査します。
禁止語は kebab-case の単語単位で検出します（`uart-utils` は違反、`newton` は違反ではない）。
`catalog/origins.toml` が出自の正本です。`part` は識別子・kind では単数、配置は `parts/` です。
`.git` など隠しディレクトリ、`target`、`node_modules` は走査しません。

## 検査の限界と P0 の残作業

現時点で CI に入っていない規則を、実装済みとみなさないでください。

- CPU/IP/part の manifest スキーマとプロパティ型、SoC/board の基本的不変条件、catalog ID／参照、I²C アドレス／SPI CS、`MachineGraph` の基本整合性は `lint-catalog` で検証します。
  Graph edge の port/pin 名、index 範囲、part binding、広義の source/sink role は検査します。ただし、両端の contract 互換性と manifest の port 定義と Rust 実装の自動照合、catalog registry 解決、`MachineGraph -> assemble -> Machine` の自動構築は未実装です。仕様 → テストの追跡表も今後の作業です。
- catalog のホスト API 直接利用禁止、SoC 名混入、ゲスト入力からの panic の検査。
- glue の行数警告、`local/` モデルや外部 pack の分類規則。
- workspace 自動同期、雛形生成、catalog-all と機能別ビューの生成。
- CODEOWNERS のレビュー責任者設定（オーナー判断が必要）。
- cargo-deny、依存ライセンスの許可リストと DCO / CLA（ADR 0006 が先）。

P0 の技術的骨格を追加した段階であり、上記と初日の未決判断を含む P0 全体の完了は宣言しません。
contract も 2 つ目の実装で検証するまで安定 API ではありません。

## GitHub Actions によるビルド・テスト

ローカル環境のネットワーク制限を回避するため、ビルド・テストは GitHub Actions を正本にします。
このセッションの作業ブランチへコミット・push し、**その commit SHA の run** を確認します。
古い run の成功を、新しい変更の成功として扱わないでください。

```sh
git push origin arena/d8b11562-mosaic
gh run list --branch arena/d8b11562-mosaic \
  --json databaseId,headSha,status,conclusion,url \
  --jq ".[] | select(.headSha == \"$(git rev-parse HEAD)\")"
gh run view <run-id> --json headSha,status,conclusion,jobs,url
gh run watch <run-id> --interval 15 --exit-status
# 失敗した場合
gh run view <run-id> --log-failed
```

CI は fmt、xtask、Clippy、明示的な build、workspace テスト、イベント・マシン実行例の
期待出力比較、doctor を実行します。テストは `--locked` を用い、Cargo.lock と一致させます。
workflow_dispatch でも起動可能です。新しい push では同一 ref の古い run をキャンセルします。

P0 の初回全体検証は [Actions run 39885634799](https://github.com/kagurasumusun/mosaic/actions/runs/39885634799)
（commit `9dc2cde`）で成功しています。前回ローカルで未実行だった xtask CLI、
Clippy、metadata 読み込み・命名のテストも、この run で確認できました。
イベント基盤の追加は [Actions run 39886095476](https://github.com/kagurasumusun/mosaic/actions/runs/39886095476)
（commit `990755a`）で、fmt / xtask / Clippy / build / 全テスト / 期待出力比較 / doctor が成功しました。
テスト定義は既存 12 件 + ランタイム 21 件で計 33 件です。
Unicorn / FlatView の追加は [Actions run 39887319724](https://github.com/kagurasumusun/mosaic/actions/runs/39887319724)
（commit `80e54ac`）で全ステージ成功。54 件のテスト、通常ビルド、Clippy、runtime / Unicorn の
両期待出力比較を含みます。ARM native build の前提パッケージは CI でインストールします。

後続変更の検証状況は、その commit に紐づく最新の run を参照してください。
CI は OS 移行の影響を避けて ubuntu-24.04 に固定し、checkout は Node.js 24 対応版を SHA 固定しています。
ローカルにはツールチェーン本体・依存キャッシュ・検証生成物をコミットしません。

## イベント・マシン基盤

詳細と未実装範囲は [runtime ガイド](runtime.md) を参照してください。
最初の実機・ISA を選ぶ前でも、CPU に依存しないイベント基盤と、アリーナ・MMIO・
signal を統合した `Machine` を検証できます。
これは P1 のファームウェア起動という出口条件を満たすものではありません。

```sh
cargo run --locked --package mosaic-runtime --example events
cargo run --locked --package mosaic-runtime --example machine
```

## P1 に進む前の確認

1. 最初の実機・MCU を選ぶ（開発用の初期 engine 検証は ARM A32。ボードは未決定）。
2. ライセンス、公開・配布時期、貢献規約を決める。
3. 決定論を既定とする暫定 ADR の要件を確認する。
4. 最小 machine テストの UART 期待出力と、ファームウェアの出自・ビルド手順を定める。
