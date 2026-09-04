# Git Guardrails — BLOCK/ALLOW Contract

この文書は `.claude/hooks/block-dangerous-git.sh` の BLOCK/ALLOW 契約を正準化する。
Claude Code の PreToolUse フックとして危険な git 操作を未然にブロックし、
feature ブランチへの通常 push は許容する（single-point-of-failure 解消、org-feedback #150/#151）。

- 対象フック: `.claude/hooks/block-dangerous-git.sh`
- 正準ケース一覧（BLOCK/ALLOW の真実の源）: `scripts/test_hook_harness.sh`
- 関連 Issue: #35（本文書化）, #32 / PR #31（`--force-with-lease` value 形式の strip）,
  **#108（jq-scoped matcher 化 — §2 構造解析仕様・§2.2 DISABLE_GIT_GUARD・§7 Case ID 導入・§8 non-scope 回収。本改訂で実装後の契約へロックステップ更新済み）**

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

### 2.1 コマンドの構造解析（jq segmentation — Issue #108 改訂）

フックは Bash ツールの RAW コマンドテキストを **jq ベースで構造解析** し、
「実際にシェルが実行するトークン列」のみをスキャンする。
Issue #108 により RAW 全文 grep は廃止された（heredoc・コメント・文字列リテラル由来の
false-positive BLOCK の解消。経緯は org-feedback 07-04）。

```bash
INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command')
```

- 入力は stdin への JSON（`jq` で `.tool_input.command` を抽出）。
- 抽出した COMMAND を以下の順で構造解析する（各処理の正準 pin は §7 の
  `newcase-a`〜`newcase-j`）:

| # | 処理 | 仕様 | pin |
|---|------|------|-----|
| 1 | セグメント分割 | `;` / `&&` / `\|\|` / `\|` / 改行 でコマンド列をセグメントへ分割。以降の判定はセグメント単位（複合コマンドの後段も実行される） | newcase-h |
| 2 | クォート除外 | 単引用符 `'...'` 内は文字列リテラルとしてスキャン除外。二重引用符 `"..."` 内もリテラル扱いだが、その中の `$(...)` は実行されるため **除外しない** | newcase-c / newcase-g |
| 3 | heredoc 本体除外 | `<<MARKER` 〜 `MARKER` 間はデータであり実行トークンではないためスキャン除外（§8 の残存リスク参照） | newcase-a |
| 4 | コメント除外 | トークン先頭の `#` 以降はコメントとしてスキャン除外 | newcase-b |
| 5 | `$()` は実行扱い | コマンド置換 `$(...)` の内容は（二重引用符内を含む）実際に実行されるためスキャン対象。旧式バッククォート置換は本契約の対象外（#108 仕様は `$()` のみ） | newcase-g |
| 6 | fail-closed RAW fallback | jq による解析に失敗した場合（stdin が不正 JSON 等）は **従来の RAW 全文スキャンにフォールバック** し、危険パターンがあれば BLOCK。解析失敗を理由に黙って ALLOW しない（fail-closed） | newcase-j |

- §4（ALWAYS_BLOCK）・§6（本線保護）のすべての判定は、この構造解析後の
  実行セグメント／トークン列に対して適用される。

### 2.2 DISABLE_GIT_GUARD エスケープハッチ（reviewer / verifier lane 専用）

- フック **プロセスの環境変数** `DISABLE_GIT_GUARD=1` が設定されている場合、
  全ガードを免除し `exit 0`（ALLOW）する（pin: newcase-d）。
- 目的は reviewer / verifier lane がフックの誤爆に阻害されず検証コマンドを実行するための
  例外経路（org-feedback 07-04 の誤爆 3 件、consensus-judge 推奨）。
  実装 lane での使用は想定しない。
- 読み取り元は **プロセス環境変数のみ**。コマンド本文中の
  `DISABLE_GIT_GUARD=1 git push origin master` のような前置きはフックプロセスの環境には
  伝播しないため **無効**（pin: newcase-i）。未設定時はガードが全面有効のまま
  （pin: newcase-e）。

---

## 3. 終了コード契約（Exit-code Contract）

| 終了コード | 意味 | Claude Code の動作 |
|-----------|------|-------------------|
| `0`       | **ALLOW** | コマンド実行を許可 |
| `2`       | **BLOCK** | コマンド実行を阻止。stderr の `BLOCKED: ...` メッセージをユーザーへ表示 |

- BLOCK 時は **必ず stderr** に `BLOCKED: '<command>' <reason>` 形式で理由を出力する。
- `1` 等の他のコードは契約外（フックは明示的に `0` or `2` のみ返す）。
  ハーネスは `0`/`2` 以外の終了コードを **CONTRACT-VIOLATION** として失敗扱いにする
  （#108 T1。期待値との一致だけでは通過させない）。

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

> **Issue #108 改訂**: 判定対象は RAW 全文ではなく §2.1 構造解析後の実行トークン列。
> これに伴い `push --force` / `push -f`（#8/#9）は「同一 push セグメント内の
> `--force` / `-f` トークン照合」へ強化され、flag 位置（remote の前後）に依存しない。
> 従来 RAW grep が `git push origin --force feat` を見逃していた gap（flag が remote 後で
> 部分文字列 `push --force` に不一致）は本改訂で closes する
> （決定の記録は §7 末尾。#108 estimate review condition 2）。

---

## 5. Safe-variant ポリシー（`--force-with-lease`）

`--force-with-lease` は feature ブランチ上の安全な force push として **ALLOW** 対象。
しかし ALWAYS_BLOCK リストの `push --force` / `push -f` が誘爆するのを防ぐため、
判定前に **`--force-with-lease` を strip した COPY** を生成して ALWAYS_BLOCK 判定に用いる。

- strip は §2.1 構造解析後のセグメント／トークン単位で適用する（実装詳細はフック参照）。
- 元の COMMAND は refspec / bare-push 判定（§6）で再利用するため破壊しない。
- strip 対象は以下の **3 形式すべて**（Issue #32 / PR #31 で追加）:
  1. bare 形式: `--force-with-lease`
  2. value 形式: `--force-with-lease=<ref>`
  3. value 形式: `--force-with-lease=<expect>:<update>`
- **ガードレイル維持**: 無条件 `--force` / `-f` は strip 後の COPY 上でも一致して BLOCK される。

### mix lease+force ケース（ハーネス canonical-B11 / newcase-f）

`git push --force-with-lease --force origin feat` のように同一コマンド内で
lease と無条件 force が混在した場合、`--force-with-lease` は strip されるが
**残った `--force` が ALWAYS_BLOCK に一致して BLOCK** される（UC-4）。

```
入力: git push --force-with-lease --force origin feat
strip 後 COPY: git push  --force origin feat
                                   ^^^^^^^ BLOCK (push --force)
```

---

## 6. 本線保護ルール（Trunk Protection）

`git push` 系コマンドは feature ブランチへの push を許可しつつ、
**master/main（本線）への直接 push のみブロック** する。

> **Issue #108 改訂**: 以下の判定も §2.1 構造解析後の push セグメントに対して適用する。
> heredoc 本体・コメント・文字列リテラル内の `git push` トークンは判定対象外。
> （下記の正規表現・抽出ロジックはセグメントテキストへの適用として読むこと）

### 6.1 refspec に master/main がスタンドアロントークンとして含まれる

境界アンカー付き正規表現で検出:

```bash
echo "$PUSH_SEGMENT" | grep -qE "(^|[^[:alnum:]./_-])(master|main)([^[:alnum:]./_-]|$)"
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

refspec 抽出ロジック（push セグメントから `git push`・global flags・remote・スタンドアロン
HEAD を除去し、残ったトークンが空なら「refspec 無し＝現在ブランチ push」と判定）:

```bash
refspec=$(echo "$PUSH_SEGMENT" | sed -E \
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
> ハーネスは canonical-A03 / canonical-A04 の期待値をフックと同じ手順で
> 現在ブランチから解決する（master/main 上では BLOCK、それ以外では ALLOW を期待）。

---

## 7. 正準ケース一覧（`scripts/test_hook_harness.sh`）

以下はハーネスのケース一覧。本文書の BLOCK/ALLOW 表はこれと **完全に一致** しなければならない
（ドリフト検出のため、ケース追加時は本文書とハーネスの両方を機械的に更新すること）。

- Issue #108 より各ケースに **Case ID**（`canonical-B01`…/`canonical-A01`…/`newcase-a`…）を
  付番し、ハーネス出力の ID と §7 表の ID を機械的に突合可能にした（drift 検出要件、#108 AC-5）。
- ハーネスは各ケースの期待値（BLOCK/ALLOW）を **assertion** し、全ケース一致で
  `exit 0`、不一致で `exit 1`（report-only は不可 — #108 AC-6）。
- 現行ケース数: **BLOCK 17 / ALLOW 13 / 計 30**。
  このうち canonical 20 ケース（BLOCK 11 / ALLOW 9）は #108 以前からの **契約不変**
  （コマンド・期待値とも不変。#108 AC-2）。`newcase-a`〜`newcase-j` の 10 ケースが
  #108 追加（T1。うち a/b/c/d/j は RAW スキャン時代は RED だった false-positive 廃止・
  fail-closed 要求の pin）。

### SHOULD BLOCK — canonical 11 ケース（契約不変）

| ID | ケース | コマンド | ブロック理由 |
|----|--------|---------|-------------|
| canonical-B01 | push origin master | `git push origin master` | refspec が master（§6.1） |
| canonical-B02 | push origin main | `git push origin main` | refspec が main（§6.1） |
| canonical-B03 | push HEAD:master | `git push origin HEAD:master` | refspec が master（§6.1） |
| canonical-B04 | delete master refspec | `git push origin :master` | refspec が master（§6.1） |
| canonical-B05 | force push feat | `git push --force origin feat` | `push --force`（§4 #8） |
| canonical-B06 | -f push feat | `git push -f origin feat` | `push -f`（§4 #9） |
| canonical-B07 | push --all | `git push --all` | `push --all`（§4 #10） |
| canonical-B08 | push --mirror | `git push --mirror` | `push --mirror`（§4 #11） |
| canonical-B09 | clean -fd | `git clean -fd` | `git clean -fd`（§4 #3） |
| canonical-B10 | branch -D | `git branch -D feat` | `git branch -D`（§4 #5） |
| canonical-B11 | mix lease+force (AC6) | `git push --force-with-lease --force origin feat` | lease strip 後に `--force` 残存（§5） |

### SHOULD ALLOW — canonical 9 ケース（契約不変）

> feature ブランチ上、または refspec に本線を含まない通常 push。
> canonical-A03 / canonical-A04 は §6.2 branch-dependent（現在ブランチから期待値を解決）。

| ID | ケース | コマンド | 許可理由 |
|----|--------|---------|---------|
| canonical-A01 | push origin feat/x | `git push origin feat/x` | feature refspec（§6.1 非一致） |
| canonical-A02 | push -u origin feat/x | `git push -u origin feat/x` | feature refspec + upstream flag |
| canonical-A03 | push origin HEAD | `git push origin HEAD` | 現在ブランチ解決。feature 上なら ALLOW（§6.2） |
| canonical-A04 | bare push | `git push` | 同上（§6.2） |
| canonical-A05 | push feat/master-fix | `git push origin feat/master-fix` | 境界アンカー非一致（§6.1） |
| canonical-A06 | push release/masterson | `git push origin release/masterson` | 境界アンカー非一致（§6.1） |
| canonical-A07 | push --force-with-lease feat | `git push --force-with-lease origin feat` | lease strip → ALLOW（§5） |
| canonical-A08 | lease=\<ref\> | `git push --force-with-lease=mainfeat origin feat` | lease value 形式 strip → ALLOW（§5, Issue #32） |
| canonical-A09 | lease=\<expect\>:\<update\> | `git push --force-with-lease=abc123:def456 origin feat` | lease value 形式 strip → ALLOW（§5, Issue #32） |

### Issue #108 追加ケース — 10 ケース（`newcase-a`〜`newcase-j`、T1）

| ID | 期待値 | ケース | コマンド | 根拠 |
|----|--------|--------|---------|------|
| newcase-a | ALLOW | heredoc 本体（UC-1） | `cat <<'EOF'` ⏎ `docs example: git push --force origin feat (reference text, not executed)` ⏎ `EOF` | heredoc 本体はスキャン除外（§2.1 #3。残存リスクは §8） |
| newcase-b | ALLOW | コメント（UC-1） | `echo deployed # never run git reset --hard here` | コメント除外（§2.1 #4） |
| newcase-c | ALLOW | 文字列リテラル（UC-1） | `echo "push --force"` | クォート除外（§2.1 #2） |
| newcase-d | ALLOW | DISABLE_GIT_GUARD 設定（UC-3） | フック **プロセス環境変数** `DISABLE_GIT_GUARD=1` 設定下で `git push origin master` | エスケープハッチ（§2.2。reviewer/verifier lane 専用） |
| newcase-e | BLOCK | DISABLE_GIT_GUARD 未設定 | `git push origin master`（環境変数なし） | 未設定時はガード全面有効（§2.2）。refspec が master（§6.1） |
| newcase-f | BLOCK | lease+force 混在（UC-4） | `git push --force-with-lease --force origin feat` | lease strip 後に `--force` 残存（§5）。canonical-B11 と同一コマンドの UC-4 pin |
| newcase-g | BLOCK | $() 置換 | `echo "danger: $(git push --force origin feat)"` | `$(...)` は実行扱い。二重引用符内も除外しない（§2.1 #2/#5） |
| newcase-h | BLOCK | 複合セグメント | `echo start && git reset --hard HEAD~1` | セグメント分割で後段を実行トークン列として検出（§2.1 #1） |
| newcase-i | BLOCK | 敵対的 env prefix | `DISABLE_GIT_GUARD=1 git push origin master`（フック環境変数は **未設定**） | 本文 prefix はプロセス環境へ伝播しない（§2.2）。refspec が master（§6.1） |
| newcase-j | BLOCK | fail-closed RAW fallback | 不正 JSON stdin: `not-json-prefix {"tool_input":{"command":"git push --force origin feat"}}` | 解析失敗時の RAW fallback が危険 payload を BLOCK（§2.1 #6） |

### 機械的突合（drift 検定 — #108 AC-5）

本文書とハーネスのケース一致は以下で機械検証する（ハーネスは各ケースのラベルに
`[ID]` を出力する）:

```bash
# §7 表の ID セット
grep -oE '^\| (canonical-(B|A)[0-9]{2}|newcase-[a-j]) ' .claude/rules/git-guardrails.md \
  | tr -d '| ' | sort -u > /tmp/doc-ids.txt
# ハーネス出力の ID セット（FAILURES サマリが ID を重複出力するため -u で一意化）
bash scripts/test_hook_harness.sh 2>/dev/null \
  | grep -oE '\[(canonical-(B|A)[0-9]{2}|newcase-[a-j])\]' | tr -d '[]' | sort -u > /tmp/harness-ids.txt
# 両者が完全一致すること（差分ゼロ・30 ID）
diff /tmp/doc-ids.txt /tmp/harness-ids.txt && echo "IDs in sync (30)"
```

> **決定記録（#108 estimate review condition 2）**: 従来 RAW grep は
> `git push origin --force feat`（flag が remote 後）を部分文字列 `push --force`
> 不一致で ALLOW していた gap を、#108 の token 照合化（§4 改訂）で **closes する**
> 方式を採用した。本 gap はハーネスケースとして未 pin であるため、follow-up で
> canonical ケースへの追加を推奨する（pin されるまで本決定の回帰検証は
> §4 の token 照合仕様と既存 30 ケースが代理で担保する）。

---

## 8. スコープ外（Non-scope）と残存リスク

以下は本契約／Issue #108 の対象外:

- **`BENIGN_FLAGS` リファクタ**: refspec 抽出の sed 群をデータ駆動に再構成する改修
  （Issue #35 当時から継続 deferred）。
- **bash heredoc 経由のインタプリタ注入 — 残存リスク（チケット受諾済み）**:
  UC-1（newcase-a）のとおり heredoc 本体はスキャン除外のため、
  `bash <<'EOF' … git reset --hard … EOF` のように **インタプリタ (bash) に危険コマンドを
  流し込む** 経路は本契約では検出しない。これは Issue #108 が false-positive BLOCK の廃止と
  引き換えに **明示的に受諾した残存リスク** である（agent が heredoc・一時スクリプト経由で
  フックを素通りする運用実態 = org-feedback 07-04 を踏まえ、構造解析化が正味のガードレイル
  強化であるとの判断）。この経路の検出は本契約のスコープ外とし、human review で担保する。
- （回収記録）旧 non-scope 項目「matcher の jq-scoping 厳密化」は **Issue #108 で回収済み**。
  §2.1 として実装・文書化されたため non-scope リストから削除した。

---

## 9. 改訂時のチェックリスト

フックの動作を変更した場合:

- [ ] `scripts/test_hook_harness.sh` のケースを追加/更新した
- [ ] 本文書の §4 ALWAYS_BLOCK 表 / §7 正準ケース一覧を同一内容で更新した
- [ ] 新規ケースに §7 の Case ID（`canonical-Bxx` / `canonical-Axx` / `newcase-x`）を付番し、ハーネス側も同じ ID をラベルに出力するようにした
- [ ] 本文書とハーネスでケース数（BLOCK / ALLOW）と ID セットが一致することを §7 の機械的突合コマンドで検証した
- [ ] `bash scripts/test_hook_harness.sh` が期待どおり BLOCK/ALLOW を assertion し `exit 0` で通過することを確認した
- [ ] ALLOW ケース数と BLOCK ケース数を本文書とハーネスで突き合わせた（現行: BLOCK 17 / ALLOW 13 / 計 30）
