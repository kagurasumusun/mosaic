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

## 初期構築時の検証

この開発環境は Rust が初期状態では未導入で、ネットワークの許可先も限定されています。
リポジトリ外に一時的に用意した Rust 1.85.0 を使い、以下を確認しました。
ツールチェーン本体・キャッシュ・検証用生成物は Git に含めていません。

- `cargo fmt --all -- --check` 成功。
- 共通型 2 件、純粋な層・命名 policy 7 件を `rustc --test` で実行し、計 9 件成功。
- xtask を除く 14 クレートを `rustc --deny warnings` で依存順にコンパイルし成功。
- `cargo metadata --no-deps --locked` で 15 クレートを確認し、実際のグラフを Rust の層 policy に通して成功。
- 仮 CLI が「P0 skeleton / no machine execution」と表示することを確認。
- Cargo.lock は公式 crates.io-index の GitHub ミラーから取得した index と checksum で生成。
  一時的なローカル registry 設定はリポジトリに含めず、lock の source は通常の crates.io のまま。

crates.io の取得先がネットワーク制限で利用できず、**workspace 全体の cargo test、
Clippy、xtask CLI 本体の実行は未検証**です。metadata 読み込みと名前 lint の追加テスト 3 件も
この未実行部分に含まれます。通常のネットワーク環境または GitHub Actions で、README の
一式を必ず実行してください。CI の定義追加と CI 通過は別です。

## P1 に進む前の確認

1. 最初の実機・ISA を選ぶ（設計書は MCU 級を推奨するが、未決定）。
2. ライセンス、公開・配布時期、貢献規約を決める。
3. 決定論を既定とする暫定 ADR の要件を確認する。
4. 最小 machine テストの UART 期待出力と、ファームウェアの出自・ビルド手順を定める。
