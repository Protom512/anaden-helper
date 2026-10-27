# anaden-helper

Windows PC ゲーム (ANOTHER EDEN) の自動操作パイプライン。Rust ワークスペース
(7 crates) + egui GUI (anaden-studio) + CLI (anaden)。

## crates

| crate | 責務 |
|---|---|
| `anaden-core` | ドメイン型 (Goal/StopCondition/InputCommand/ScreenRegion)。純粋・I/O なし |
| `anaden-vision` | テンプレートマッチ (SSE 輝度差 / CCOEFF 相関)、TaskDef・pipeline 読み込み |
| `anaden-device` | Win32 キャプチャ (PrintWindow)・入力注入 (SendInput/PostMessage)・起動監視 |
| `anaden-engine` | PipelineDriver (tick ループ・リカバリ・ゴール評価)、routine 実行 |
| `anaden-strategies` | 戦略カタログ (CLI 引数列の単一情報源) |
| `anaden-studio` | egui 統合 GUI (オーサリング・実行・履歴) |
| `anaden-cli` | `anaden` バイナリ (run / routine / ensure-open / launch) |

依存方向: core ← vision/device ← engine ← strategies ← cli/studio。

## よく使うコマンド

```bash
cargo build -p anaden-cli --release      # CLI (jobs=4 必須 — OOM 回避、settings.json で常設済み)
cargo nextest run --workspace            # テスト (cargo test ではなく nextest)
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
node scripts/quality/complexity_gate.mjs    # 複雑度 gate (cyc≤15/cog≤20)
node scripts/quality/duplication_gate.mjs   # 重複 gate (similarity ≥0.85)
node scripts/quality/assert_tests_untouched.mjs  # テスト不改変ガード
./scripts/quality/bench.sh                  # ピーク RSS ベンチ (release 必須)
```

## 重要な契約

- **ライブラリコードで unwrap/expect/panic 禁止** (clippy deny)。テスト
  (`#[cfg(test)]`) では 3 つの `#[allow]` (unwrap/panic/expect) を追加。
- **テンプレートは 1280x720 正規化空間**。発火座標の逆変換は
  `rescale_command` (幅基準スケール) を通す。
- **fail-closed**: IO/parse/検証失敗は黙って継続しない。エラーを構造化して伝播。
- ログ文言 (warn!/info! のフォーマット) は e2e テストが pin している場合が
  ある — 変更時は対象テストを確認。

## ディレクトリ

- `templates/pipelines/` — pipeline 定義 (login / fishing / field_loop_pc / nav_to_field_pc)
- `templates/tasks/` — タスク定義 (TOML)
- `docs/quality/complexity-baseline.md` — 品質ゲート基線 (数値凍結)
- `.claude/rules/` — 詳細ルール (evidence 検証・Rust anti-patterns・TDD)
