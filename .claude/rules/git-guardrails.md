# Git Guardrails — BLOCK/ALLOW Contract

この文書は `.claude/hooks/block-dangerous-git.sh` の BLOCK/ALLOW 契約を正準化する。
Claude Code の PreToolUse フックとして危険な git 操作を未然にブロックし、
feature ブランチへの通常 push は許容する（single-point-of-failure 解消、org-feedback #150/#151）。

- 対象フック: `.claude/hooks/block-dangerous-git.sh`
- 正準ケース一覧（BLOCK/ALLOW の真実の源）: `scripts/test_hook_harness.sh`
- 関連 Issue: #35（本文書化）, #32 / PR #31（`--force-with-lease` value 形式の strip）,
  **#108（jq-scoped matcher 化 — §2 構造解析仕様・§2.2 DISABLE_GIT_GUARD・§7 Case ID 導入・§8 non-scope 回収。本改訂で実装後の契約へロックステップ更新済み）**,
  **#108 remediation（cycle-44 majors — AC-2 quoted-flag §2.1 #8・AC-3 監査証跡 §2.2・
  AC-4 インタプリタ再帰スキャン §2.1 #9・§4 quoted-token 注意・§8 closure。
  本改訂で newcase-k/l/m の遡及反映を含む 45 ケースへ lockstep 更新・drift 解消）**

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
  `newcase-a`〜`newcase-w`・数字サフィックス ID (`newcase-d2` 等) 含む）:

| # | 処理 | 仕様 | pin |
|---|------|------|-----|
| 1 | セグメント分割 | `;` / `&&` / `\|\|` / `\|` / 改行 でコマンド列をセグメントへ分割。以降の判定はセグメント単位（複合コマンドの後段も実行される） | newcase-h |
| 2 | クォート除外 | 単引用符 `'...'` 内は文字列リテラルとしてスキャン除外。二重引用符 `"..."` 内もリテラル扱いだが、その中の `$(...)` は実行されるため **除外しない**。git セグメントでの完全クォート「単語」トークンのフラグ合流は #8（AC-2）参照 | newcase-c / newcase-g |
| 3 | heredoc 本体除外 | `<<MARKER` 〜 `MARKER` 間はデータであり実行トークンではないためスキャン除外。**ただし受取コマンド語がインタプリタの場合は #9 の再帰スキャン対象**（列挙は §8） | newcase-a |
| 4 | コメント除外 | トークン先頭の `#` 以降はコメントとしてスキャン除外 | newcase-b |
| 5 | `$()` は実行扱い | コマンド置換 `$(...)` の内容は（二重引用符内を含む）実際に実行されるためスキャン対象。旧式バッククォート置換は本契約の対象外（#108 仕様は `$()` のみ） | newcase-g |
| 6 | fail-closed RAW fallback | jq による解析に失敗した場合（stdin が不正 JSON 等）は **従来の RAW 全文スキャンにフォールバック** し、危険パターンがあれば BLOCK。解析失敗を理由に黙って ALLOW しない（fail-closed） | newcase-j |
| 7 | CRLF 正規化 | `\r` はコマンド抽出・構造解析の前に strip（Windows checkout・CRLF 混入対策）。CRLF 改行の混入で後段の危険セグメントが隠れない | newcase-l |
| 8 | quoted-flag 合流（AC-2） | **git コマンドセグメント**（トークン列に standalone `git` を含む）では、**完全クォートかつ空白を含まない「単語」トークン**（`"--hard"` / `'-fd'` 等）をクォート剥がし後にフラグとして ALWAYS_BLOCK/§5 照合へ合流する（シェル展開後は同一トークンになるため）。**多語クォート文字列**（`"git reset --hard"` 全体・`commit -m "prose"` 等）はデータ除外を維持し誤爆ゼロ。非 git セグメントのクォート文字列は #2 どおり除外のまま | newcase-q/r/s/t/u/v（BLOCK pin）/ newcase-o/p（push 系 born-green pin）/ newcase-w（多語 ALLOW 境界 pin） |
| 9 | インタプリタ heredoc 再帰スキャン（AC-4） | heredoc の受取コマンド語（basename 正規化: `/bin/bash` → `bash`）が bash/sh/ash/dash/zsh/ksh/csh/tcsh の場合、本文はインタプリタへ渡る **実行コード** のため再帰的にトークナイズしてスキャン対象セグメントへ合流する。cat 等への heredoc は #3 どおり本文除外のまま。再帰は同一 jq プロセス内で完結する（プロセス fan-out 不変）。未終端インタプリタ heredoc も蓄積済み本文を合流（fail-closed）。列挙外インタプリタは §8 の明示受諾 | newcase-n（BLOCK pin）/ newcase-a（cat 系 ALLOW 境界 pin） |

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
- **監査証跡仕様（AC-3, cycle-44 major-3）**: ハッチ使用は無言にしない。`exit 0` の前に
  stdin を消費し（producer 側 SIGPIPE 書き込みエラーの防止も兼ねる）、**stderr** へ
  `GIT_GUARD_DISABLED: escape hatch used (DISABLE_GIT_GUARD=1) — command NOT scanned: <対象コマンド>`
  形式の監査行を出力する。**stdout には書かない**（exit 0 時の stdout は hook 出力として
  解釈されうるため）。
- 監査行は **genuine なハッチ使用時のみ** 出力する。コマンド本文中の env prefix
  （newcase-i）ではガードが有効のまま BLOCK し、監査行も出さない
  （pins: newcase-d2 = 監査行あり / newcase-i2 = 監査行なし）。

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

> **quoted-token 適用注意（AC-2, cycle-44 major-2）**: ALWAYS_BLOCK 判定は、git コマンド
> セグメントでは **クォート剥がし済み「単語」トークンを合流させたトークン列**
> （§2.1 #8）に対して適用する。`git reset "--hard"` / `git clean '-fd'` /
> `git branch "-D" feat` 等の完全クォート フラグトークンは、シェル展開後（= クォート剥がし後）
> に同一トークンになるため **BLOCK 対象**（pins: newcase-q/r/s/t/u/v）。一方、
> **多語クォート文字列**（`git commit -m "never run git reset --hard here"` 等）は
> データ除外のまま **ALLOW**（pin: newcase-w — quoted-flag 検出が raw-substring 照合へ
> 回帰しない境界 pin）。push セグメントの `--force` / `-f` / `--all` / `--mirror`
> トークン検出もクォート付きトークンを（剥がしたうえで）含む **全トークン列** で行う
> （pins: newcase-o/p）。

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
> 判定には push セグメントの **全トークン列**（クォート付きトークンもクォート剥がしの
> うえ含む）を用いるため、`git push origin "master"` のような refspec のクォートは
> 回避にならない（pin: newcase-m）。
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

- Issue #108 より各ケースに **Case ID**（`canonical-B01`…/`canonical-A01`…/`newcase-a`…/
  `newcase-d2`…（数字サフィックス含む））を付番し、ハーネス出力の ID と §7 表の ID を
  機械的に突合可能にした（drift 検出要件、#108 AC-5）。
- ハーネスは各ケースの期待値（BLOCK/ALLOW/stderr 監査）を **assertion** し、全ケース一致で
  `exit 0`、不一致で `exit 1`（report-only は不可 — #108 AC-6）。
- 現行ケース数: **計 45 assertion — BLOCK 29 / ALLOW 14 / stderr 監査 2**。
  数値は §7 末尾の機械的導出コマンドの **実測値** から記載する（手編集禁止 —
  かつて doc 30 / 実測 33 の pin drift を生んだ運用の再発防止）。
  このうち canonical 20 ケース（BLOCK 11 / ALLOW 9）は #108 以前からの **契約不変**
  （コマンド・期待値とも不変。#108 AC-2）。`newcase-a`〜`newcase-m` の 13 exit-code
  ケースが #108 本体追加（a〜j は T1。うち a/b/c/d/j は RAW スキャン時代は RED だった
  false-positive 廃止・fail-closed 要求の pin。**k/l/m は T2 追加で一時 §7 未反映だった
  pin drift を cycle-44 で遡及解消**）。`newcase-n`〜`newcase-w` の 10 exit-code ケースと
  `newcase-d2`/`newcase-i2` の stderr 監査 2 ケースが cycle-44 remediation 追加
  （AC-2 quoted-flag・AC-3 監査証跡・AC-4 インタプリタ bypass）。
  なお canonical-A03/A04 の 2 ケースは §6.2 branch-dependent であり、master/main 上では
  BLOCK に解決する（その場合の機械的導出値は BLOCK 31 / ALLOW 12 / stderr 監査 2）。

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

### Issue #108 追加ケース — 13 ケース（`newcase-a`〜`newcase-m`、T1/T2）

| ID | 期待値 | ケース | コマンド | 根拠 |
|----|--------|--------|---------|------|
| newcase-a | ALLOW | heredoc 本体（UC-1） | `cat <<'EOF'` ⏎ `docs example: git push --force origin feat (reference text, not executed)` ⏎ `EOF` | heredoc 本体はスキャン除外（§2.1 #3。受取がインタプリタの場合は §2.1 #9） |
| newcase-b | ALLOW | コメント（UC-1） | `echo deployed # never run git reset --hard here` | コメント除外（§2.1 #4） |
| newcase-c | ALLOW | 文字列リテラル（UC-1） | `echo "push --force"` | クォート除外（§2.1 #2。非 git セグメントの多語/単語クォートは #8 の合流対象外） |
| newcase-d | ALLOW | DISABLE_GIT_GUARD 設定（UC-3） | フック **プロセス環境変数** `DISABLE_GIT_GUARD=1` 設定下で `git push origin master` | エスケープハッチ（§2.2。reviewer/verifier lane 専用）。stderr 監査証跡は newcase-d2 |
| newcase-e | BLOCK | DISABLE_GIT_GUARD 未設定 | `git push origin master`（環境変数なし） | 未設定時はガード全面有効（§2.2）。refspec が master（§6.1） |
| newcase-f | BLOCK | lease+force 混在（UC-4） | `git push --force-with-lease --force origin feat` | lease strip 後に `--force` 残存（§5）。canonical-B11 と同一コマンドの UC-4 pin |
| newcase-g | BLOCK | $() 置換 | `echo "danger: $(git push --force origin feat)"` | `$(...)` は実行扱い。二重引用符内も除外しない（§2.1 #2/#5） |
| newcase-h | BLOCK | 複合セグメント | `echo start && git reset --hard HEAD~1` | セグメント分割で後段を実行トークン列として検出（§2.1 #1） |
| newcase-i | BLOCK | 敵対的 env prefix | `DISABLE_GIT_GUARD=1 git push origin master`（フック環境変数は **未設定**） | 本文 prefix はプロセス環境へ伝播しない（§2.2）。refspec が master（§6.1）。stderr 監査なしは newcase-i2 |
| newcase-j | BLOCK | fail-closed RAW fallback | 不正 JSON stdin: `not-json-prefix {"tool_input":{"command":"git push --force origin feat"}}` | 解析失敗時の RAW fallback が危険 payload を BLOCK（§2.1 #6） |
| newcase-k | BLOCK | force flag が remote 後 | `git push origin --force feat` | push セグメント内トークン照合が flag 位置 (remote 後) を検出（§4 改訂。従来 RAW grep の gap closure — T2 追加、cycle-44 で遡及 pin 解消） |
| newcase-l | BLOCK | CRLF 混入セグメント | `echo ok\r\ngit clean -fd`（`\r\n` 区切り 2 行） | `\r` は構造解析前に strip（§2.1 #7）。Windows checkout でも危険セグメントが隠れない — T2 追加 |
| newcase-m | BLOCK | クォート済み trunk refspec | `git push origin "master"` | §6 判定は push セグメントの全トークン列（クォート付きトークンを含む）に適用。refspec クォートは回避にならない — T2 追加 |

### Issue #108 remediation 追加ケース — 12 ケース（cycle-44 majors、T1/T3）

| ID | 期待値 | ケース | コマンド | 根拠 |
|----|--------|--------|---------|------|
| newcase-n | BLOCK | heredoc→インタプリタ bypass（AC-4） | `bash <<'EOF'` ⏎ `git reset --hard HEAD~1` ⏎ `EOF` | 受取コマンド語がインタプリタの場合は本文を再帰スキャン（§2.1 #9）。§8 の当該経路 closure pin |
| newcase-o | BLOCK | push クォート `--force`（remote 前） | `git push "--force" origin feat` | push セグメントの全トークン列照合はクォート付きトークンを含む（§4 注意）。born-green 回帰 pin |
| newcase-p | BLOCK | push クォート `--force`（remote 後） | `git push origin "--force" feat` | 同上（§4 注意）。born-green 回帰 pin |
| newcase-q | BLOCK | quoted-flag `--hard`（二重クォート） | `git reset "--hard"` | 完全クォート「単語」トークンはクォート剥がし後にフラグとして照合（§2.1 #8） |
| newcase-r | BLOCK | quoted-flag `-fd`（二重クォート） | `git clean "-fd"` | 同上（§2.1 #8） |
| newcase-s | BLOCK | quoted-flag `-D`（二重クォート） | `git branch "-D" feat` | 同上（§2.1 #8） |
| newcase-t | BLOCK | quoted-flag `--hard`（単一クォート） | `git reset '--hard'` | 同上（§2.1 #8） |
| newcase-u | BLOCK | quoted-flag `-f`（単一クォート） | `git clean '-f'` | 同上（§2.1 #8） |
| newcase-v | BLOCK | quoted-flag `-D`（単一クォート） | `git branch '-D' feat` | 同上（§2.1 #8） |
| newcase-w | ALLOW | 多語クォートはデータ（誤爆ゼロ pin） | `git commit -m "never run git reset --hard here"` | 多語クォート文字列はデータ除外を維持（§2.1 #8 の境界）。quoted-flag 検出の raw-substring 照合への回帰防止 |
| newcase-d2 | stderr=present | ハッチ監査証跡（AC-3） | フック **プロセス環境変数** `DISABLE_GIT_GUARD=1` 設定下で `git push origin master` — **stderr に `GIT_GUARD_DISABLED` 監査行が出ること** | ハッチ使用は無言にしない（§2.2 監査証跡仕様）。newcase-d と同一呼び出しの stderr 検証 |
| newcase-i2 | stderr=absent | 敵対的 env prefix に監査行なし（AC-3） | `DISABLE_GIT_GUARD=1 git push origin master`（本文 prefix・環境変数は **未設定**）→ BLOCK + **stderr に監査行が出ないこと** | 監査証跡は genuine なハッチ使用のみ（§2.2）。newcase-i と同一呼び出しの stderr 検証 |

### 機械的突合（drift 検定 — #108 AC-5）

本文書とハーネスのケース一致は以下で機械検証する（ハーネスは各ケースのラベルに
`[ID]` を出力する）。newcase の ID は **英小字 + 数字サフィックス**（`newcase-d2` 等）
なので正規表現は `newcase-[a-z0-9]+` を使うこと — `newcase-[a-j]` 等の狭い範囲だと
`k` 以降・数字付き ID が突合対象外になり vacuous pass になる（cycle-44 で実際に
k/l/m が doc 未反映のまま透過していた drift の直接原因）:

```bash
# ハーネス実行（exit 0 = 全ケース PASS を確認してから ID セットを採る）
bash scripts/test_hook_harness.sh 2>/dev/null > /tmp/harness-out.txt
# §7 表の ID セット
grep -oE '^\| (canonical-(B|A)[0-9]{2}|newcase-[a-z0-9]+) ' .claude/rules/git-guardrails.md \
  | tr -d '| ' | sort -u > /tmp/doc-ids.txt
# ハーネス出力の ID セット（FAILURES サマリが ID を重複出力するため -u で一意化）
grep -oE '\[(canonical-(B|A)[0-9]{2}|newcase-[a-z0-9]+)\]' /tmp/harness-out.txt \
  | tr -d '[]' | sort -u > /tmp/harness-ids.txt
# 両者が完全一致すること（差分ゼロ・45 ID）
diff /tmp/doc-ids.txt /tmp/harness-ids.txt && echo "IDs in sync ($(wc -l < /tmp/doc-ids.txt))"
# ケース数の機械的導出 — §7 見出しの数値は必ずこの実測値から書く（手編集禁止）
grep -c '^PASS expect=BLOCK' /tmp/harness-out.txt   # → 29（master/main 上は 31）
grep -c '^PASS expect=ALLOW' /tmp/harness-out.txt   # → 14（master/main 上は 12）
grep -c '^PASS stderr\['     /tmp/harness-out.txt   # → 2
```

> **決定記録（#108 estimate review condition 2）**: 従来 RAW grep は
> `git push origin --force feat`（flag が remote 後）を部分文字列 `push --force`
> 不一致で ALLOW していた gap を、#108 の token 照合化（§4 改訂）で **closes する**
> 方式を採用した。**本 gap は `newcase-k` としてハーネス pin 済み**（§7 表参照）であり、
> 回帰検証は常にハーネス実行で担保される。かつて本決定に付記されていた
> 「未 pin につき follow-up で canonical 追加を推奨」は pin 完了により解消した
> （T2 で newcase-k を追加した際に §7 へ反映せず doc↔harness drift を生んでいた点も、
> cycle-44 remediation で k/l/m を遡及反映して解消済み）。

---

## 8. スコープ外（Non-scope）と残存リスク

以下は本契約／Issue #108 の対象外:

- **`BENIGN_FLAGS` リファクタ**: refspec 抽出の sed 群をデータ駆動に再構成する改修
  （Issue #35 当時から継続 deferred）。
- **heredoc→インタプリタ注入 — cycle-44 remediation で closure（AC-4）**:
  `bash <<'EOF' … git reset --hard … EOF` 形式の **インタプリタへの heredoc 流し込み** は、
  受取コマンド語（basename 正規化）が bash/sh/ash/dash/zsh/ksh/csh/tcsh の場合に本文を
  再帰スキャンして **BLOCK する**（§2.1 #9、pin: newcase-n）。Issue #108 当時
  「明示的に受諾した残存リスク」だった本経路は、cycle-44 majors の remediation で
  閉塞（回収）した。
- **列挙外インタプリタへの heredoc — 明示受諾（継続）**: 上記列挙外のインタプリタ
  （`python <<'EOF'` / `node <<'EOF'` 等）への heredoc 本文は、各言語の文法で git を
  実行するコードを静的に判定することが本契約のスコープ外であるため、**引き続き
  データ除外（ALLOW）を受諾する**。human review で担保する。
- （回収記録）旧 non-scope 項目「matcher の jq-scoping 厳密化」は **Issue #108 で回収済み**。
  §2.1 として実装・文書化されたため non-scope リストから削除した。
- （回収記録 2）旧残存リスク「bash heredoc 経由のインタプリタ注入」は
  **cycle-44 remediation で回収済み**（§2.1 #9 の再帰スキャン + newcase-n pin。
  列挙外インタプリタは上記の明示受諾として分離）。

---

## 9. 改訂時のチェックリスト

フックの動作を変更した場合:

- [ ] `scripts/test_hook_harness.sh` のケースを追加/更新した
- [ ] 本文書の §4 ALWAYS_BLOCK 表 / §7 正準ケース一覧を同一内容で更新した
- [ ] 新規ケースに §7 の Case ID（`canonical-Bxx` / `canonical-Axx` / `newcase-<英小字+数字>`。実在しない ID 形（例: `newcase-x` のようなリテラル）をプレースホルダに使わない）を付番し、ハーネス側も同じ ID をラベルに出力するようにした
- [ ] 本文書とハーネスでケース数（BLOCK / ALLOW / stderr 監査）と ID セットが一致することを §7 の機械的突合コマンド（正規表現 `newcase-[a-z0-9]+`）で検証した
- [ ] §7 見出しのケース数は機械的導出コマンドの **実測値** から記載した（手編集による 30 vs 33 drift の再発防止）
- [ ] `bash scripts/test_hook_harness.sh` が期待どおり BLOCK/ALLOW/stderr 監査を assertion し `exit 0` で通過することを確認した
- [ ] DISABLE_GIT_GUARD ハッチ使用時の stderr 監査証跡（`GIT_GUARD_DISABLED` 行）の出力（newcase-d2）と、敵対的 env prefix での非出力（newcase-i2）を確認した
- [ ] 現行ケース数を突き合わせた（現行: **計 45 assertion — BLOCK 29 / ALLOW 14［うち §6.2 branch-dependent 2］+ stderr 監査 2**）
