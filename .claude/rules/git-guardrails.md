# Git Guardrails — BLOCK/ALLOW Contract

この文書は `.claude/hooks/block-dangerous-git.sh` の BLOCK/ALLOW 契約を正準化する。
Claude Code の PreToolUse フックとして危険な git 操作を未然にブロックし、
feature ブランチへの通常 push は許容する（single-point-of-failure 解消、org-feedback #150/#151）。

- 対象フック: `.claude/hooks/block-dangerous-git.sh`
- 正準ケース一覧（BLOCK/ALLOW の真実の源）: `scripts/test_hook_harness.sh`
- 関連 Issue: #35（本文書化）, #32 / PR #31（`--force-with-lease` value 形式の strip）

---

## 1. 配線（Wiring）

`.claude/settings.json` の PreToolUse フックが Bash ツール呼出し毎に本フックを起動する。

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/block-dangerous-git.sh"
          }
        ]
      }
    ]
  }
}
```

- **matcher**: `Bash` のみ。他ツール（Edit/Write/Read 等）はスキャン対象外。
- **起動コマンド**: `"$CLAUDE_PROJECT_DIR"/.claude/hooks/block-dangerous-git.sh`
  （プロジェクトルート相対・マシン固有パスリテラルなし）

---

## 2. PreToolUse マッチャ仕様

フックは Bash ツールの RAW コマンドテキストをスキャンする。

```bash
INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command')
```

- 入力は stdin への JSON（`jq` で `.tool_input.command` を抽出）。
- スキャン対象は **RAW コマンド全文**。よって以下も scan 対象となる:
  - heredoc 本体（`<<EOF ... git reset --hard ... EOF`）
  - コメント（`# git push --force ...`）
  - 文字列リテラル・echo 文
- これは既知の false-positive 表面だが、**本件のスコープ外**とする（Issue #35 の non-scope）。
  matcher を jq-scoping で厳密化する改修は本契約の対象ではない。

---

## 3. 終了コード契約（Exit-code Contract）

| 終了コード | 意味 | Claude Code の動作 |
|-----------|------|-------------------|
| `0`       | **ALLOW** | コマンド実行を許可 |
| `2`       | **BLOCK** | コマンド実行を阻止。stderr の `BLOCKED: ...` メッセージをユーザーへ表示 |

- BLOCK 時は **必ず stderr** に `BLOCKED: '<command>' <reason>` 形式で理由を出力する。
- `1` 等の他のコードは契約外（フックは明示的に `0` or `2` のみ返す）。

---

## 4. ALWAYS_BLOCK（無条件ブロック）

以下のパターンは **現在ブランチ・Wiki の有無に関わらず常にブロック** される。
feature ブランチ上でも master/main 上でも拒否される（本線保護の最終防衛線）。

| # | パターン（正規表現） | ブロック対象 |
|---|---------------------|-------------|
| 1 | `git reset --hard`  | ハードリセット（作業ツリー破壊） |
| 2 | `reset --hard`      | 同上（エイリアス/部分形式） |
| 3 | `git clean -fd`     | 未追跡ファイル+ディレクトリ削除 |
| 4 | `git clean -f`      | 未追跡ファイル削除 |
| 5 | `git branch -D`     | ブランチ強制削除 |
| 6 | `git checkout \.`   | 作業ツリー変更の一括破棄 |
| 7 | `git restore \.`    | 同上（restore 形式） |
| 8 | `push --force`      | 無条件 force push |
| 9 | `push -f`           | 同上（短縮形） |
| 10 | `push --all`       | 全ブランチ一括 push（master/main を含むため本線保護上ブロック） |
| 11 | `push --mirror`    | ミラー push（同上） |

判定は ALWAYS_BLOCK 用の **`--force-with-lease` strip 済み COPY** に対して行う（§5 参照）。

---

## 5. Safe-variant ポリシー（`--force-with-lease`）

`--force-with-lease` は feature ブランチ上の安全な force push として **ALLOW** 対象。
しかし ALWAYS_BLOCK リストの `push --force` / `push -f` が誘爆するのを防ぐため、
判定前に **`--force-with-lease` を strip した COPY** を生成して ALWAYS_BLOCK 判定に用いる。

```bash
COMMAND_FOR_ALWAYS_BLOCK=$(echo "$COMMAND" \
  | sed -E 's/[[:space:]]+--force-with-lease(=[^[:space:]]*)?([[:space:]]|$)/ /g')
```

- 元の `COMMAND` は refspec / bare-push 判定（§6）で再利用するため破壊しない。
- strip 対象は以下の **3 形式すべて**（Issue #32 / PR #31 で追加）:
  1. bare 形式: `--force-with-lease`
  2. value 形式: `--force-with-lease=<ref>`
  3. value 形式: `--force-with-lease=<expect>:<update>`
- **ガードレイル維持**: 無条件 `--force` / `-f` は strip 後の COPY 上でも一致して BLOCK される。

### mix lease+force ケース（ハーネンス AC6）

`git push --force-with-lease --force origin feat` のように同一コマンド内で
lease と無条件 force が混在した場合、`--force-with-lease` は strip されるが
**残った `--force` が ALWAYS_BLOCK に一致して BLOCK** される。

```
入力: git push --force-with-lease --force origin feat
strip 後 COPY: git push  --force origin feat
                                   ^^^^^^^ BLOCK (push --force)
```

---

## 6. 本線保護ルール（Trunk Protection）

`git push` 系コマンドは feature ブランチへの push を許可しつつ、
**master/main（本線）への直接 push のみブロック** する。

### 6.1 refspec に master/main がスタンドアロントークンとして含まれる

境界アンカー付き正規表現で検出:

```bash
echo "$COMMAND" | grep -qE "(^|[^[:alnum:]./_-])(master|main)([^[:alnum:]./_-]|$)"
```

- 境界文字クラス `[^[:alnum:]./_-]` で挟むことで **branch 名内の master/main は誘爆しない**。
- 一致例（すべて BLOCK）:
  - `git push origin master`
  - `git push origin main`
  - `git push origin HEAD:master`
  - `git push origin :master`（master ref の削除）
- 非一致例（ALLOW）:
  - `git push origin feat/master-fix`（`master` の前に `-`）
  - `git push origin release/masterson`（`master` の後に `son`）

### 6.2 裸 push の現在ブランチ解決

refspec を明示しない裸 push は現在ブランチをリモートへ push するため、
現在ブランチが master/main の場合のみ BLOCK する。

裸 push とみなす形式:
- `git push`
- `git push origin`
- `git push origin HEAD`

refspec 抽出ロジック（`git push`・global flags・remote・スタンドアロン HEAD を除去し、
残ったトークンが空なら「refspec 無し＝現在ブランチ push」と判定）:

```bash
refspec=$(echo "$COMMAND" | sed -E \
  -e 's/^git push//' \
  -e 's/[[:space:]]+(-u|--set-upstream|--force-with-lease|--tags|--no-tags|--dry-run|-n|--quiet|-q|--verbose|-v|--follow-tags)//g' \
  -e 's/[[:space:]]+origin([[:space:]]|$)/ /g' \
  -e 's/[[:space:]]+https?:\/\/[^[:space:]]+//g' \
  -e 's/[[:space:]]+git@[^[:space:]]+//g' \
  -e 's/(^|[[:space:]])HEAD([[:space:]]|$)/ /g')
```

現在ブランチ解決:

```bash
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")
if [ "$CURRENT_BRANCH" = "master" ] || [ "$CURRENT_BRANCH" = "main" ]; then
  BLOCK=yes   # 本線上の裸 push
fi
```

> **注意**: この判定は実行環境の現在ブランチに依存する。同じコマンドでも
> feature ブランチ上なら ALLOW、master/main 上なら BLOCK となる。
> `scripts/test_hook_harness.sh` を master 上で実行すると
> `push origin HEAD` / `bare push` が BLOCK になるのはこのため（仕様通り）。

---

## 7. 正準ケース一覧（`scripts/test_hook_harness.sh`）

以下はハーネスのケース一覧。本文書の BLOCK/ALLOW 表はこれと **完全に一致** しなければならない
（ドリフト検出のため、ケース追加時は本文書とハーネスの両方を機械的に更新すること）。

### SHOULD BLOCK（11 ケース）

| # | ケース | コマンド | ブロック理由 |
|---|--------|---------|-------------|
| 1 | push origin master | `git push origin master` | refspec が master（§6.1） |
| 2 | push origin main | `git push origin main` | refspec が main（§6.1） |
| 3 | push HEAD:master | `git push origin HEAD:master` | refspec が master（§6.1） |
| 4 | delete master refspec | `git push origin :master` | refspec が master（§6.1） |
| 5 | force push feat | `git push --force origin feat` | `push --force`（§4 #8） |
| 6 | -f push feat | `git push -f origin feat` | `push -f`（§4 #9） |
| 7 | push --all | `git push --all` | `push --all`（§4 #10） |
| 8 | push --mirror | `git push --mirror` | `push --mirror`（§4 #11） |
| 9 | clean -fd | `git clean -fd` | `git clean -fd`（§4 #3） |
| 10 | branch -D | `git branch -D feat` | `git branch -D`（§4 #5） |
| 11 | mix lease+force (AC6) | `git push --force-with-lease --force origin feat` | lease strip 後に `--force` 残存（§5） |

### SHOULD ALLOW（9 ケース）

> feature ブランチ上、または refspec に本線を含まない通常 push。

| # | ケース | コマンド | 許可理由 |
|---|--------|---------|---------|
| 1 | push origin feat/x | `git push origin feat/x` | feature refspec（§6.1 非一致） |
| 2 | push -u origin feat/x | `git push -u origin feat/x` | feature refspec + upstream flag |
| 3 | push origin HEAD | `git push origin HEAD` | 現在ブランチ解決。feature 上なら ALLOW（§6.2） |
| 4 | bare push | `git push` | 同上（§6.2） |
| 5 | push feat/master-fix | `git push origin feat/master-fix` | 境界アンカー非一致（§6.1） |
| 6 | push release/masterson | `git push origin release/masterson` | 境界アンカー非一致（§6.1） |
| 7 | push --force-with-lease feat | `git push --force-with-lease origin feat` | lease strip → ALLOW（§5） |
| 8 | push --force-with-lease=\<ref\> | `git push --force-with-lease=mainfeat origin feat` | lease value 形式 strip → ALLOW（§5, Issue #32） |
| 9 | push --force-with-lease=\<expect\>:\<update\> | `git push --force-with-lease=abc123:def456 origin feat` | lease value 形式 strip → ALLOW（§5, Issue #32） |

> ケース #3/#4 は実行環境の現在ブランチに依存し、master/main 上では BLOCK される（§6.2）。
> ハーネンスは master 上で実行するとこの 2 ケースが BLOCK になるが、これは仕様通り。

---

## 8. スコープ外（Non-scope）

以下は本契約／Issue #35 の対象外:

- **matcher の jq-scoping 厳密化**: heredoc 本体・コメント由来の false-positive を
  減らすためのコマンド構造解析（AST 的アプローチ）。現在は RAW 全文スキャンで妥協。
- **`BENIGN_FLAGS` リファクタ**: refspec 抽出の sed 群をデータ駆動に再構成する改修。
- 上記のいずれも現行 BLOCK/ALLOW 契約を変えない限り追跡しない。

---

## 9. 改訂時のチェックリスト

フックの動作を変更した場合:

- [ ] `scripts/test_hook_harness.sh` のケースを追加/更新した
- [ ] 本文書の §4 ALWAYS_BLOCK 表 / §7 正準ケース一覧を同一内容で更新した
- [ ] `bash scripts/test_hook_harness.sh` が期待どおり BLOCK/ALLOW を返すことを確認した
- [ ] ALLOW ケース数と BLOCK ケース数を本文書とハーネスで突き合わせた
