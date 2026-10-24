# tests

機械レベル・決定論・互換性・性能の横断テストを配置します。
単体テストは各クレート内にあります。ファームウェア起動の golden test は `apps/bringup`、
`apps/sensorbus`、`apps/multicore`、`apps/vic-thermal` にあり、CI で実行します。

`runtime-events.stdout` は `mosaic-core-runtime` の `events` 実行例に対する期待出力です。
GitHub Actions がビルドした例を実行し、このファイルとの一致を比較します。
実行例の内部では、キューの保存状態を別インスタンスへ復元した場合も出力と最終 State が
一致することを検査します。これは CPU を使わない runtime smoke test であり、
実機やファームウェアの仕様準拠を検証する machine test ではありません。

`machine-snapshot.stdout` は `mosaic-core-runtime` の `snapshot` 実行例に対する期待出力です。
RAM、snapshot 対応デバイス State、pending timer の復元と再配送を比較します。CPU engine、SignalSink、
永続 codec を含む VM 全体の snapshot を検査するものではありません。

`unicorn-arm.stdout` は CLI の opt-in `unicorn-smoke` の期待出力です。
FlatView 由来の RX コード / RW データ領域で A32 の MOV / ADD / STR / LDR を実行し、
4 回の命令フック、レジスタ、PC、メモリを検査します。これも実機の machine test ではありません。
