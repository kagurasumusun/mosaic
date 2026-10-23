# P0 開発ガイド

## workspace の規則

クレートを追加するときは root `Cargo.toml` の members と、新しいクレートの
`[package.metadata.layer]` を更新します。workspace の version / edition /
rust-version / lints を継承し、ライセンス決定までは `publish = false` にします。
ライブラリの先頭には `#![forbid(unsafe_code)]` を置きます。

```toml
[package.metadata.layer]
domain = "framework"
kind = "contract"
level = 2
```

| domain | kind | level |
| --- | --- | --- |
| framework | prim | 0 |
| framework | types | 1 |
| framework | contract / modelkit / isa | 2 |
| framework | runtime / interconnect / engine | 3 |
| framework | describe | 4 |
| framework | assemble / host / ops / bridge | 5 |
| catalog | cpu / ip / ip-lib / part / subsys / soc / module / board / world | 3（依存は別ルール） |
| apps | app | 6 |
| tooling | tooling | 7 |

メタデータは配置と照合するため、catalog のモデルを app と偽装して検査を回避できません。
SoC 等の Rust コードは `glue` ディレクトリに限ります。

## 構成検査

```sh
cargo xtask lint-layers
cargo xtask lint-names
cargo xtask check       # 上記を両方実行
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
- VM 層の外部依存許可リストは現在空です。必要な依存は policy のレビューとともに追加します。
  xtask と apps にはこの空の許可リストを適用しません。

`lint-names` はソースディレクトリ名とクレート名を検査します。
禁止語は kebab-case の単語単位で検出します（`uart-utils` は違反、`newton` は違反ではない）。
`catalog/origins.toml` が出自の正本です。`part` は識別子・kind では単数、配置は `parts/` です。
`.git` など隠しディレクトリ、`target`、`node_modules` は走査しません。

## 検査の限界と P0 の残作業

現時点で CI に入っていない規則を、実装済みとみなさないでください。

- モデルの manifest スキーマ、ports と実装の整合、仕様 → テストの追跡表。
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

CI は fmt、xtask、Clippy、明示的な build、workspace テスト、イベント実行例の
期待出力比較、doctor を実行します。テストは `--locked` を用い、Cargo.lock と一致させます。
workflow_dispatch でも起動可能です。新しい push では同一 ref の古い run をキャンセルします。

P0 の初回全体検証は [Actions run 39885634799](https://github.com/kagurasumusun/mosaic/actions/runs/39885634799)
（commit `9dc2cde`）で成功しています。前回ローカルで未実行だった xtask CLI、
Clippy、metadata 読み込み・命名のテストも、この run で確認できました。
イベント基盤の追加は [Actions run 39886095476](https://github.com/kagurasumusun/mosaic/actions/runs/39886095476)
（commit `990755a`）で、fmt / xtask / Clippy / build / 全テスト / 期待出力比較 / doctor が成功しました。
テスト定義は既存 12 件 + ランタイム 21 件で計 33 件です。
後続変更の検証状況は、その commit に紐づく最新の run を参照してください。
CI は OS 移行の影響を避けて ubuntu-24.04 に固定し、checkout は Node.js 24 対応版を SHA 固定しています。
ローカルにはツールチェーン本体・依存キャッシュ・検証生成物をコミットしません。

## イベント基盤

詳細と未実装範囲は [runtime ガイド](runtime.md) を参照してください。
最初の実機・ISA を選ぶ前でも、CPU に依存しないイベント基盤を検証できます。
これは P1 のファームウェア起動という出口条件を満たすものではありません。

## P1 に進む前の確認

1. 最初の実機・ISA を選ぶ（設計書は MCU 級を推奨するが、未決定）。
2. ライセンス、公開・配布時期、貢献規約を決める。
3. 決定論を既定とする暫定 ADR の要件を確認する。
4. 最小 machine テストの UART 期待出力と、ファームウェアの出自・ビルド手順を定める。
