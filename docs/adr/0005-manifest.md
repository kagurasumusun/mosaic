# 0005: 識別子、TOML と MachineGraph

状態: 暫定採用（実装は P1）。根拠: 構成設計書 §5, §14, §16。

識別子は `kind:origin/name[@variant]`。SoC は family を含められる。
kind は単数（`part`）、ディレクトリは `parts`。出自は catalog/origins.toml の正規名のみ。
プロジェクト接頭辞はリポジトリ名に合わせ `mosaic` を使用する。

manifest は identity / tags / ports / params を持ち、機能・役割・忠実度は木の配置に混ぜない。
TOML と Rust builder は同じ MachineGraph へ落とす。IR は実行時と catalog 実装に依存しない。
extends / overlay の操作は add / set / remove / replace-use に限定する。
宣言はループや条件式を持たない。

P0 ではクレートの `[package.metadata.layer]` と origins だけを導入する。
モデル manifest、IR、パーサ、include / overlay、pack スキーマ検証は未実装。
最初の 1 年はスキーマの安定性より実例による改善を優先する。
