# 0014: マルチコア時間モデルの保証範囲

状態: 暫定採用。現行モデルは決定論的な serial-slice simulation であり、cycle-accurate
lockstep simulation ではない。

## 背景

`MulticoreDriver` は固定順で各 CPU engine に instruction quantum を渡し、共有 RAM mirror
を量子境界で同期し、イベントと IRQ を配送する。これはテストの再現性を支える一方、
各コアが同時に実行する物理時間や、量子の途中で発生するイベントを厳密に表現しない。
「決定論的である」ことを「cycle-accurate / coherent SMP である」ことと混同してはならない。

## 決定

- 現行 `step` / `step_clocked` の契約は、固定順・量子境界での serial-slice simulation とする。
  `step_clocked` は各 slice の instruction-hook 数と設定済み period から仮想時間差を計算する。
- `VTime` は絶対時刻、`VDelta` は時間差として型を分け、時刻進行は checked arithmetic を使う。
- 同じ初期状態、engine backend、core order、quantum、外部入力なら、同じ配送順の観測結果を
  再現することを目標にする。これは一般の SMP memory consistency や命令単位の同期保証ではない。
- 現在は engine quantum の実行が終わってからイベントを配送する。quantum 内で予定時刻を
  横切るイベントや、実行中に MMIO が新規予約したイベントについて、イベント時刻で CPU を
  中断できるとは主張しない。これは既知の精度制約である。
- 本格的な lockstep / event-horizon scheduler は別の実行ポリシーとして導入し、backend が
  提供できる instruction count / cycle count / precise stop capability を検証してから採用する。
  現行 scheduler を無言で cycle-accurate と再解釈しない。

## 次の受け入れ条件

1. event horizon を越えない execution budget を計算できる API を engine contract に追加する。
2. 実行中 MMIO が予約するイベントの時刻基準を明示し、必要なら仮想時刻を engine callback へ渡す。
3. 同時刻のイベント、共有 RAM 書き込み、IRQ delivery の順序規則を ADR と golden trace で固定する。
4. lockstep を名乗る構成では、イベント時刻直前・同時刻・直後の回帰試験を必須にする。
