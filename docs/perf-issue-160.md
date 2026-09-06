# Issue #160 UC-5 (Shard 6) — GUI 毎フレーム経路のパフォーマンス実測とホットスポット解消

Issue #160 受け入れ基準 5: 「perf — before/after 計測 evidence (例: 毎フレーム
log snapshot 全行 clone の解消、計測方法明記)」に対する実測記録。
本書の数値はすべて生コマンド出力のコピーであり、計測条件を明記して再現可能にする。

## 1. 計測対象 (ホットスポット)

egui は毎フレーム (概ね 60fps) アプリの `render_*` を呼び、その中で:

- `StudioApp::render_task_list` → `drain_task_logs()` → `refresh_task_log_snapshot()`
  (app.rs, Tasks ホーム)
- `PipelineRunnerApp::render_run_body` → `drain_logs()` → `refresh_snapshot()`
  (runner.rs, 実行ビュー)

最適化前はこの毎フレーム経路で:

1. `drain_channel_into` が **行ごとに** `with_buf` で Mutex lock/unlock を繰り返す
2. `refresh_*_snapshot` がログバッファ **全行** (上限 `DEFAULT_MAX_LINES` = 5000 行、
   各行 ~90 文字の `String`) を毎フレーム clone

していた。ログは 1 秒に数行〜数十行しか到着しないのに対し UI は 60fps で回るため、
大半のフレームは「新着 0 行でも 5000 行 clone」を払っていた (これが本 Issue の
名指しホットスポット)。

## 2. 計測方法

- **ベンチマークフレームワーク**: criterion 0.8.2 (`anaden-studio` の
  dev-dependencies のみ。実依存への影響なし)
- **ベンチファイル**: `crates/anaden-studio/benches/log_snapshot_bench.rs`
  (`harness = false`, `test = false` — `cargo test` では実行されず
  `cargo bench` のみ)
- **手法**: GUI フレーム全体 (レイアウト・ラスタライズ) はベンチに載らないため、
  毎フレーム実行される純ロジック (drain + スナップショット更新) だけを抽出。
  ベンチの `before_full_clone` 系は最適化前の production コードと同一の操作列
  (旧 `drain_channel_into` の行ごとロック + 旧 `refresh_*_snapshot` の全行 clone)
  をベンチ内に再現したものであり、**after 実行バイナリにも残して同一バイナリ・
  同一セッションでの A/B 比較を可能にしている** (セッション間のマシンゆらぎを
  排除するため)。
- **シナリオ**:
  - `frame_idle`: 新着行 0 のフレーム 1 回分 (バッファは 5000 行充填済み)
  - `frame_burst10`: 1 フレームに 10 行到着したフレーム 1 回分
  - 行内容は実運用の anaden ログに近い ~90 文字行
- **イテレーション**: criterion 既定 (各ベンチ warm-up 3 秒 + 100 サンプル、
  推定 5〜8 秒)。idle/before は 10,000 イテレーション、idle/after は
  85,000,000 イテレーションを計測に使用。
- **ハードウェア**: 12th Gen Intel Core i5-12400F / RAM 32 GB /
  Windows 11 Home (10.0.26200) / リリースビルド (`cargo bench` = bench profile)

### 実行コマンド (全文)

```bash
# before 計測 (最適化前の working tree — production は旧パターン):
cargo bench -p anaden-studio --bench log_snapshot_bench -- --save-baseline before

# after 計測 (最適化後 — legacy ベンチも同一バイナリで再測定して A/B):
cargo bench -p anaden-studio --bench log_snapshot_bench
```

## 3. 計測結果 (生値)

### before (最適化前の木で実行)

```
frame_idle/before_full_clone
                        time:   [468.77 µs 494.03 µs 519.96 µs]
Found 3 outliers among 100 measurements (3.00%)

frame_burst10/before_full_clone
                        time:   [443.35 µs 469.00 µs 494.56 µs]
Found 1 outliers among 100 measurements (1.00%)
```

### after (最適化後。同一バイナリ内で legacy パターン (A/B 基準) と新パターンを同時測定)

```
frame_idle/before_full_clone      (旧パターン再現・比較基準)
                        time:   [400.40 µs 427.73 µs 455.15 µs]

frame_idle/after_gated            (新パターン: revision-gated)
                        time:   [51.548 ns 55.043 ns 58.574 ns]

frame_burst10/before_full_clone   (旧パターン再現・比較基準)
                        time:   [415.63 µs 438.03 µs 461.67 µs]

frame_burst10/after_gated         (新パターン: 変更ありフレーム)
                        time:   [437.53 µs 459.73 µs 483.16 µs]
```

### 解釈 (正直に)

- **アイドルフレーム (新着 0 行 — 実運用の大多数)**: 427.73 µs → 55.043 ns
  (中央値、同一バイナリ比較) で **~7,800 倍**。毎フレーム 5000 行の `String`
  clone + 5000 要素 `Vec` 割当が、Mutex 1 回 + u64 比較に置換された。
  60fps フレームバジェット (16.67 ms) に対し旧パターンは毎フレーム ~2.6%
  を消費していたが、新パターンは無視できる水準になった。
- **新着ありフレーム (10 行/フレーム)**: 438.03 µs → 459.73 µs (中央値) で
  **~+5% (ほぼ同等)**。変更のあったフレームは依然としてスナップショット全行
  再構築を 1 回払う設計のため、これは想定どおり (悪化幅は legacy ベンチ自体の
  セッション間ゆらぎ (本記録でも 423〜494 µs の幅) と同程度であり、有意な
  悪化とは判断しない)。
- セッション間のマシンゆらぎは存在する (before セッションの legacy は
  494.03 µs、after セッションの同一 legacy は 427.73 µs ≈ -14%)。そのため
  改善率の根拠は **同一セッション内の A/B** (427.73 µs vs 55.043 ns) を用いる。

## 4. 解消内容 (コード箇所)

| 変更 | 箇所 |
|------|------|
| `LogBuffer::revision` 改訂番号カウンタ (push/clear で増分) | `crates/anaden-studio/src/log_view.rs:214` (`push_entry` での増分 :253、`clear` での増分 :288、アクセサ :263) |
| `SharedLogBuffer::changed_entries` — 変化時のみ全行複製を返す差分 API | `crates/anaden-studio/src/log_view.rs:369` |
| `drain_channel_into` のロック 1 回化 (旧: 行ごとに lock) | `crates/anaden-studio/src/log_view.rs:394` |
| `StudioApp::refresh_task_log_snapshot` 差分化 (`task_log_revision` キャッシュ) | `crates/anaden-studio/src/app.rs:762` (フィールド :401) |
| `PipelineRunnerApp::refresh_snapshot` 差分化 (`log_revision` キャッシュ) | `crates/anaden-studio/src/runner.rs:531` (フィールド :237)、`clear_logs` :543 |

**振る舞い互換性**: `task_log_lines() -> &[LogEntry]` / `log_snapshot() -> &[LogEntry]`
の公開署名・スナップショット内容 (更新後は全行相当・昇順) は不変。差分化に伴い
「クリア後の stale スナップショット残留」が構造的に起きないよう `clear` も改訂番号を
進める。回帰テスト (後述) が行欠けなし・冪等性・クリア挙動を機械検証する。

## 5. 検証テスト (振る舞い不変の保証)

- `log_view.rs` 単体テスト (Issue #160 UC-5):
  - `changed_entries_returns_none_when_buffer_unchanged` — 変化なしフレームで
    clone が発生しない契約
  - `changed_entries_returns_full_entries_after_push` — push 後は全行返却
  - `changed_entries_detects_clear_as_change_to_empty` — クリア検知
  - `changed_entries_reflects_eviction_at_capacity` — 上限破棄込みの内容
  - `drain_channel_into_batches_lines_and_first_exit_wins` — ロック 1 回化後も
    行数カウント・Exit 行記録・最初の Exit 観測の契約が旧実装と同一
- `runner.rs`: `test_log_snapshot_idempotent_across_idle_drains` — 60 フレーム
  相当のアイドル drain でスナップショットが冪等に行を保持
- `app.rs`: `task_log_snapshot_idempotent_across_idle_drains_and_keeps_lines` —
  起動失敗停止後の実キューで同様の冪等性を実子プロセスなしで検証
- 既存テスト一式 (タスクキューヘッドレス E2E 含む 797 件) が全通過
  (`cargo nextest run --workspace`: 797 passed, 8 skipped)

## 6. 品質ゲート (コミット前実行・生コマンド結果)

```text
cargo fmt --all --check                 → 通過 (出力なし)
cargo clippy --all-targets -- -D warnings → Finished ... 14.04s (警告なし)
cargo nextest run --workspace           → Summary: 797 tests run: 797 passed, 8 skipped
ast-grep test --skip-snapshot-tests     → test result: ok. 1 passed; 0 failed
ast-grep scan                           → exit 0 (error severity なし)
```

## 7. 残余課題 (本シャードでは解消せず記録のみ)

1. **変更ありフレームの全行再構築**: 上限 5000 行到達後は新着 1 行ごとに最古行が
   破棄されるため差分追記が効かず、変更フレームは O(n) 再構築のまま。行数上限の
   縮小 (テール固定長) か `Arc<Vec<LogEntry>>` 差し替えで更に削減できる見込み。
2. **`render_task_list` 内の毎フレーム `clone()`** (app.rs:781 `self.task_defs.clone()`
   / app.rs:838 `self.task_queue.clone()`, 借用制約のため): ログ 5000 行 clone
   に比べれば小さいが、件数増加時の同一パターンのホットスポット候補。
3. **`run_status_summary()` の毎フレーム String 生成** (runner.rs:675 実行ビュー):
   フォーマット 1 回/フレーム。微小だがキャッシュ可能。
4. **`append_history_record` の log_tail 構築** (runner.rs): 終了時 1 回のみの
   全行 clone。頻度が低いため影響小。
5. GUI フレーム全体 (egui レイアウト・テクスチャ更新) の p50/p95/p99 実測は
   本ベンチのスコープ外 (純ロジック抽出方式を採用。headless フレームループ
   ハーネルは未作成)。
