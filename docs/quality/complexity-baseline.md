# 複雑度ゲート ベースライン (Issue #214 / Phase 0)

リファクタリング前の機械検証済みベースライン。**単一コミットとして 1 回だけ
記録する evidence** であり、以後この数値は更新しない (ゲート実行結果は
`target/quality/*.json` を都度参照すること)。

- 採取日: 2026-10-17
- ブランチ: `refactor/complexity-gate` (基底 `expl/ai-coding-loop-1` @ 7d8321d)
- ツール (実測バージョン): cccc-rs 0.4.0 / similarity-rs 0.5.0 / cargo-mutants 27.0.0 /
  cargo-nextest 0.9.122 / hyperfine 1.20.0 / Node v24.22.0 / rustc 1.95.0
- ゲートスクリプト: `scripts/quality/complexity_gate.mjs` / `duplication_gate.mjs` /
  `assert_tests_untouched.mjs` / `mutants_gate.mjs` / `bench.sh` + `rss_bench.ps1`
- 閾値 (ユーザー決定): cyclomatic ≤ 15 / cognitive ≤ 20 / similarity ≥ 0.85 は違反
  (allowlist 除く) / mutants はワークスペース全体で「ベースライン比 新規生存ゼロ」

## 1. 複雑度 (cccc) — 違反 17 件 (生産 = src + examples)

`node scripts/quality/complexity_gate.mjs` → exit 1 (Phase 0 として期待値)。
report: `target/quality/cccc-report.json`。全 violate 関数 (cyc/cog 降順):

| # | 対象 | cyc | cog |
|---|---|---|---|
| 1 | engine `pipeline_driver.rs:1561` `run_loop_with_goal` | 38 | 74 |
| 2 | engine `pipeline_driver.rs:1305` `run_loop_with_recovery` | 31 | 54 |
| 3 | studio `app_ui_tasks.rs:20` `render_task_list` | 22 | 42 |
| 4 | device `probe_windows_launch.rs:138` `run` (example) | 22 | 37 |
| 5 | studio `app_ui_authoring.rs:54` closure in `render_authoring` | 25 | 35 |
| 6 | vision `bench_match.rs:39` `main` (example) | 23 | 35 |
| 7 | device `win32_launch.rs:238` `run_blocking` | 21 | 35 |
| 8 | device `probe_windows_input.rs:326` `parse_args` (example) | 20 | 29 |
| 9 | core `goal.rs:271` `evaluate` | 22 | 28 |
| 10 | studio `scenario_validate.rs:116` `validation_issues` | 20 | 28 |
| 11 | vision `collector.rs:253` `verify_templates` | 11 | 26 |
| 12 | studio `batch.rs:81` `evaluate` | 11 | 24 |
| 13 | studio `app_ui_authoring.rs:319` closure in `render_authoring` | 9 | 23 |
| 14 | studio `runner_exec.rs:145` `resolve_pipeline_arg` | 15 | 22 |
| 15 | vision `find_logo_corner_subfeature.rs:8` `main` (example) | 9 | 21 |
| 16 | studio `authoring_demo.rs:90` `main` (example) | 16 | 17 |
| 17 | device `probe_windows_capture.rs:130` `main` (example) | 16 | 11 |

分類 counts (機械検証): 総関数 2994 = 生産 1358 + テスト除外 1615 + bench 21
(ファイル 128)。テスト除外は `#[cfg(test)]` region 74 件 + `tests/` 16 ファイル
全体。テスト領域内の超過は 1 件のみ (`pipeline.rs:2574`
`pc_nav_to_field_cold_start_chain_is_walkable_to_field` cyc=20/cog=36 — 除外扱い)。

## 2. 重複 (similarity) — 違反 8 件 + allowlist 2 件

`node scripts/quality/duplication_gate.mjs` → exit 1 (期待値)。
report: `target/quality/duplication-report.json`。

違反 8 ペア (解消対象 — 承認プランの重複解消リストと完全一致):

| # | ペア | similarity |
|---|---|---|
| 1 | device `win32_capture.rs` `capture` ↔ `capture_blocking` | 94.24% |
| 2 | studio `scenario_panel.rs` `register_as_new_task` ↔ `bind_stub_task` | 92.81% |
| 3 | vision `letterbox.rs` `row_mean_luminance` ↔ `col_mean_luminance` | 92.76% |
| 4 | engine `pipeline_driver.rs` `run_loop_with_recovery` ↔ `run_loop_with_goal` | 92.64% |
| 5 | studio `app.rs` `save_current_template` ↔ `save_current_pipeline_task` | 91.14% |
| 6 | studio `log_process.rs` `spawn_stdout_reader` ↔ `spawn_output_readers` | 89.08% |
| 7 | studio `app_ui_live.rs` `live_authoring_side` ↔ `live_authoring_canvas` | 85.30% |

allowlist 2 件 (`duplication-allowlist.json` 登録の偽陽性 — 違反外):

| ペア | similarity | 理由 |
|---|---|---|
| cli `run_report.rs` `resolve_run_artifacts` ↔ `resolve_routine_artifacts` | 94.04% | 既に resolve_artifacts_at に共通化済み (wrapper 対称性) |
| cli `main.rs` `run_pipeline_live` ↔ `run_with_windows` | 92.76% | 704-726 は cfg(not(windows)) の bail stub。シグネチャ一致のみ |

counts: 56 ペア = 生産 40 + テスト除外 15 + bench 1。advisory (0.80-0.84) 30 件
(matcher trio 80-83% / collector trio 80-83% / win32_input 系偽陽性を含む —
情報提供のみ)。

## 3. Mutants — 総数 3180 / ベースライン取得は夜間実行

`cargo mutants --list --workspace` 実測: **3180 mutants**
(studio 1456 / vision 694 / device 366 / cli 336 / engine 206 / core 94 / strategies 28)。

較正実測 (2026-10-17, `cargo mutants -p anaden-strategies --in-place
--jobserver-tasks 4`): 28 mutants を **1m53s** (baseline build 26s 含む、
≈4s/mutant)。結果: 3 missed / 18 caught / 7 unviable。

- ワークスペース全体は夜間規模 (重い crate は 1 mutant あたり数十秒 →
  想定 8-30 時間)。**ベースライン survivor set の採取は未実施** —
  Phase 1 開始前に以下を無人実行すること:
  1. `cargo mutants --workspace --in-place --jobserver-tasks 4`
  2. `node scripts/quality/mutants_gate.mjs snapshot`
     → `target/quality/mutants-baseline.json`
- フェーズ完了時のゲート: 同コマンド + `--iterate` →
  `node scripts/quality/mutants_gate.mjs check` (新規生存ゼロで exit 0)。
- survivor = MissedMutant + TimeoutMutant (テストが殺せなかった両方)。
- 中断時は `--in-place` の変異ソースが残る恐れ → `git status` で確認後に復帰。

## 4. 決定的ベンチ (ピーク RSS 中心)

`./scripts/quality/bench.sh`。fuel も perf も本環境で使えないため
(rustc 1.95 stable に fuel 無し / perf は Linux 専用)、ピーク RSS
(`GetProcessMemoryInfo` 直接 P/Invoke — PS 5.1 の
`Process.PeakWorkingSet64` は終了後 null を返すため) を主指標とする。

ワークロード: `anaden routine <routine-bench.toml> --dry-run`
(実パイプラインテンプレート templates/pipelines/{login,fishing} を参照する
検証のみ・デバイス非接触の決定的ワークロード) ×5、release build。
実測 (2026-10-17, 5 runs):

| workload | median peak RSS | median wall |
|---|---|---|
| routine-dry-run | 8,728,576 bytes (min 8,724,480 / max 8,753,152) | 54 ms |
| cli-help (起動のみ) | 7,630,848 bytes | 44 ms |

- hyperfine 参考値 (informational, `-N` 直接実行, 5 runs): routine-dry-run
  mean 30.2 ms ± 10.4 (min 19.6 / max 46.4) — Windows ではノイジーなため
  参考値扱い (承認プランどおり)。
- criterion `anaden-studio` `log_snapshot_bench` — baseline 名
  `complexity-phase0` で保存済み (`--save-baseline`)。ベンチライン:
  - `frame_idle/before_full_clone`, `frame_idle/after_gated` ~59 ns
  - `frame_burst10/before_full_clone` [397 µs 423 µs 450 µs]
  - `frame_burst10/after_gated` [383 µs 412 µs 444 µs]
  フェーズ完了時は `--baseline complexity-phase0` で回帰比較。

## 5. Rust テストスイート (リファクタリングの安全網)

`cargo nextest run --workspace` (worktree, 2026-10-17):
**1020 tests run: 1020 passed, 10 skipped** (exit 0)。
`cargo clippy --all-targets -- -D warnings` exit 0。`cargo fmt --all --check` OK。
テストコード不改変の構造保証: `node scripts/quality/assert_tests_untouched.mjs [base]`。

## 6. ゲート運用チートシート

| 目的 | コマンド | PASS 条件 |
|---|---|---|
| 複雑度 | `node scripts/quality/complexity_gate.mjs` | exit 0 (違反 0 件) |
| 重複 | `node scripts/quality/duplication_gate.mjs` | exit 0 (≥0.85 生産ペア 0・allowlist 除く) |
| テスト不改変 | `node scripts/quality/assert_tests_untouched.mjs [base]` | exit 0 |
| mutants | `cargo mutants ... && node scripts/quality/mutants_gate.mjs check` | exit 0 (新規生存 0) |
| RSS/時間 | `./scripts/quality/bench.sh` | baseline 比で RSS 顕著な悪化なし (時間は参考値) |

注意: `node --test <dir>` は Node v24.22.0/win32 で壊れている
(ディレクトリを展開しない) — `node --test scripts/quality/complexity_gate.test.mjs`
等ファイル直指定で実行すること。
