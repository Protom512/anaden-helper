# AGENTS.md — anaden-helper

Codex / Gemini CLI など AGENTS.md 規約ツール用のエントリポイント。
Claude Code は `.claude/CLAUDE.md` と `.claude/rules/*.md` を自動読込するため、ここには
全ツール共通の最小限を置き、詳細は rules ファイル (plain markdown) を参照する。

## プロジェクト概要

- Rust workspace。PC (Windows) 専用の Another Eden 支援ツール。
- crates/: `anaden-cli`, `anaden-core`, `anaden-device`, `anaden-engine`,
  `anaden-strategies`, `anaden-studio`, `anaden-vision`
- `templates/` に MAA/MDA pipeline JSON・テンプレート資材。upstream MDA リポジトリの
  参照クローンが `references/MDA/` (詳細は「MDA / pipeline 作業」節)。

## ビルド・テスト

```bash
cargo check --all
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
CARGO_BUILD_JOBS=4 cargo nextest run --workspace
ast-grep test --skip-snapshot-tests   # ルール単体テスト
ast-grep scan                         # 構造 lint (error severity で非ゼロ終了)
```

- **`cargo test` ではなく `cargo nextest run` を使う** (doc test のみ `cargo test --doc`)。
- テスト実行時 `CARGO_BUILD_JOBS=4` は必須 (品質ゲートの PASS 条件)。
- `node --test <dir>` は Node v24/win32 で壊れている — テストファイルを直接指定すること。

## 品質ゲート (commit / PR の前提)

| コマンド | PASS 条件 |
|---|---|
| `node scripts/quality/complexity_gate.mjs` | exit 0 (cyc≤15 / cog≤20 違反 0) |
| `node scripts/quality/duplication_gate.mjs` | exit 0 (similarity ≥0.85 生産ペア 0) |
| `node scripts/quality/assert_tests_untouched.mjs [base]` | exit 0 (テスト不改変) |
| `CARGO_BUILD_JOBS=4 cargo nextest run --workspace` | 全 green |

## コミット

- 1 コミット = 1 論理的変更。type(scope): description 形式
  (`feat` / `fix` / `refactor` / `test` / `docs` / `chore`)。
- **Co-Authored-By**: `glm 4.7 <noreply@zhipuai.cn>`
  (詳細: `.claude/rules/git-commit-attribution.md`)
- ブランチ命名: `feature/<scope>-<desc>` / `fix/<scope>-<desc>` /
  `refactor/<scope>-<desc>` / `impl/<spec-name>`。master から分岐し PR 経由でマージ
  (詳細: `.claude/rules/git-branch-strategy.md`)。
- WIP commit は `--no-verify` でローカル限定。push しない。

## Evidence 規約 (自己申告禁止)

- テスト通過・行数・gate verdict などの主張は**生コマンド出力・実測値・
  `.omc/logs/{run-id}/` の永続ログ**で示す。自己申告の「全て green」は evidence にならない。
- gate 入力の diff が空なら fail-closed (vacuous GO 禁止)。
- close 主張するコミットは `git branch -a --contains <sha>` で trunk 到達を確認する。
- 詳細: `.claude/rules/pipeline-evidence-verification.md`

## Rust 規約

- ライブラリコードで `unwrap()` / `expect()` / `panic!()` / インデックスアクセス禁止。
  `Result` + `?` で伝播する (詳細: `.claude/rules/rust-anti-patterns.md`)。
- workspace lints: `panic` / `expect_used` / `unwrap_used` = deny。テストモジュールは
  `#[allow(clippy::unwrap_used)]` `#[allow(clippy::panic)]` `#[allow(clippy::expect_used)]`
  の 3 点セットを付ける (詳細: `.claude/rules/workspace-lint-allowances.md`)。
- テストは公開 API の振る舞いのみ検証し、実装詳細 (private メソッド・内部状態) に
  結合させない (詳細: `.claude/rules/tdd-coupling.md`)。
- 1 ファイル 500 行超は保守性警告の目安。

## MDA / pipeline 作業時

- upstream MDA 側の作成指針は `references/MDA/AGENTS.md` (中国語・Windows/PowerShell
  想定) を参照。基本認識 (テンプレート・OCR・色判定) は Pipeline JSON 宣言側に、
  複雑な業務ロジックはコード側に置く分担原則が述べられている。
- pipeline JSON の認識パラメータ (ROI・テンプレート・閾値) をコードにハードコード
  しない。
