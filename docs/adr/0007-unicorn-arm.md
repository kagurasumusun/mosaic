# 0007: Unicorn の初期 ARM アダプタと実行契約

状態: 暫定採用。ユーザーの「unicornも導入」という指示に基づく開発・検証への導入。
ADR 0006 のプロジェクトライセンス・貢献規約の判断を代行するものではありません。

## スコープと依存

`unicorn-engine = "=2.1.5"`、`default-features = false`、`arch_arm` のみを使用します。
Cargo の推移依存として unicorn-engine-sys と C/C++ ビルドツールを使用します。
直接依存は framework/engine/unicorn だけに許可します。app / catalog / その他の
framework が vendor crate を直接参照することは xtask で拒否します。

検証は little-endian ARM A32 の MOV / ADD / STR / LDR、ループ、命令予算、
コード再書き込み、独立した二つの engine instance を対象とします。実在 MCU の
正確な CPU モデル、NVIC、Thumb 例外、SMP、排他モニタの保証はまだありません。

## contract の変更（R8）

`CpuEngine::run` を StopReason 単体から RunOutcome に変更し、停止理由と実行フック数を返します。
フックは命令実行前なので、Fault 時の数は retire 数ではありません。途中停止の詳細を
判定できない場合は WFI / halt を推測せず CapabilityMissing を返します。
一回の同期実行予算は 1..=1,000,000 命令に制限し、ホストの時間制限は使いません。

ISA 側が RegId を定義し、ARM の register ID を adapter 内で Unicorn の ID へ変換します。
PC へのホスト書き込みは A32 用に 4 byte 境界を要求します。実行再開時には CPSR の T bit を
保持しますが、Thumb の適合性を確認したとはみなしません。

map_memory / read_memory / write_memory を導入します。初期ページは engine 所有の private memory。
loader/debug の write は ROM を初期化できますが、ゲストの permission を緩和しません。
loader によるコード変更後には translated block cache を無効化します。
Snapshot / AddressView / DMI / MMIO はまだこの契約に含めません。

## 所有権と Send

Rust binding は Rc<UnsafeCell<...>> を使用し、そのままでは CpuEngine: Send を満たしません。
公開 trait の Send を削除したり、raw handle を外へ渡したりせず、adapter 専用の
OwnedUnicorn に **1 箇所だけ** unsafe impl Send を定義します。それ以外の unsafe は deny します。

成立条件:

- native instance の escaping strong reference は adapter 内に 1 つだけ。clone / handle / Context は非公開。
- user hook や外部所有の callback を受け付けない。唯一のフックは capture-free で整数 Counter のみを更新する。
- native 呼び出し中にインスタンスを移動しない。callback 由来の一時参照は呼び出し終了までに破棄される。
- native engine を異なるスレッドで使う場合も逐次利用のみ。Sync は実装しない。
- 依存版を固定し、版更新・hook 追加・memory pointer / context 公開時にはこの境界を再監査する。

生成・実行済みの instance を別スレッドへ移動し、実行・回収するテストを行います。
これは所有権不変条件の検証を補助するもので、任意の callback のスレッド安全性を保証するものではありません。

## 初期アドレス連携

runtime の Region / FlatView は symbolic Mapping を生成します。CLI の smoke harness が
FlatView から Unicorn の private page map を生成します。runtime と engine は相互依存しません。
この段階では runtime に RAM の別コピーを持たせません。

これは完全な共有 AddressView ではなく、動的 view 更新、alias、DMI、MMIO callback、
複数 CPU の RAM 共有、snapshot、機械のモデル登録・配線は次の実装です。
