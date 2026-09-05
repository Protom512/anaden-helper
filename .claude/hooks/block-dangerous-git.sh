#!/bin/bash
# 危険な git 操作を PreToolUse でブロック（git-guardrails-claude-code スキール）。
#
# 2026-09-04 改訂 (Issue #108): RAW 全文 grep スキャンを jq ベースの argv ライク
#   構造解析 (jq-scoped matcher) へ置換し、heredoc 本体・# コメント・クォート内
#   リテラル由来の false-positive BLOCK を廃止 (org-feedback 2026-07-04)。
#   - シェル演算子 (&&, ||, ;, |, 改行) でコマンドセグメント分割
#   - クォート剥がし: 完全にクォートされたトークンはパターン照合対象から除外
#   - $(...) / バッククォート内はシェルが実行するため非クォート扱いで照合
#   - heredoc 本体 (<<DELIM ... DELIM) と # コメント行は解析対象から除外
#   - --force-with-lease (bare / =ref / =expect:update) はトークン除去で strip
#   - push セグメント (git ... push) 内の --force/-f/--all/--mirror トークン検出を
#     追加 — 旧 RAW scan が `git push origin --force feat` (flag が remote 後) を
#     見逃していた gap の closure (estimate-approval condition 2)
#   - fail-closed: jq 解析エラー・空出力・タイムアウト時は旧 RAW 全文スキャンへ
#     フォールバック (ガード弱化を構造的に排除)
#   - DISABLE_GIT_GUARD=1 (フックプロセスの環境変数のみ有効) で即 exit 0 —
#     reviewer/verifier lane 用エスケープハッチ (UC-3)。コマンド本文中の
#     `DISABLE_GIT_GUARD=1 git ...` 前置きは本プロセスへ伝播しないため無効
#     (ハーネス newcase-i 敵対ケースで検証)。
# 2026-09-05 改訂 (Issue #108 remediation, cycle-44 majors / T3):
#   - AC-2 quoted-flag: git コマンドセグメント (qflag 列に standalone `git` トークン
#     を含むセグメント) では、完全クォートかつ空白を含まない「単語」トークン
#     (`git reset "--hard"` 等) をクォート剥がし後に ALWAYS_BLOCK/§5 パターン照合
#     へ合流。多語クォート文字列 ("git reset --hard" 全体クォート / `commit -m
#     "prose"`) はデータ除外を維持し誤爆ゼロ。tokenizer 出力は 3 フィールド
#     (plain \x1f qflag \x1f full) に拡張。
#   - AC-3 audit trail: DISABLE_GIT_GUARD=1 ハッチは即 exit 0 の前に stdin を
#     消費 (producer 側 SIGPIPE 防止) し、stderr へ GIT_GUARD_DISABLED 監査行
#     (使用 fact + 対象コマンド) を出力。stdout には書かない (exit 0 時の
#     stdout は hook 出力として解釈されうるため)。
#   - AC-4 heredoc→interpreter bypass: heredoc の受取コマンド語 (セグメント先頭
#     トークンを basename 正規化) が bash/sh/ash/dash/zsh/ksh/csh/tcsh の場合、
#     本文を再帰的にトークナイズしてスキャン対象セグメントへ合流。再帰は同一
#     jq プロセス内で完結するためプロセス fan-out は増えない (AC-1 維持)。
#     cat 等への heredoc は従来どおり本文除外 (newcase-a 維持)。
#   - AC-1 perf fan-out 解消 (cycle-44 major-1 / T5 remediation): パターン照合を
#     grep プロセス起動 (旧: セグメント毎 × ALWAYS_BLOCK 11 パターン = grep 11×S 回・
#     push セグメント毎に trunk_check の grep+sed) から bash ネイティブ照合
#     ([[ =~ ]] / case / トークンフィルタ、0-spawn) へ全面移行。プロセス起動は
#     1 コマンド解析あたり定数 (cat×1 + jq×2 + timeout×1。裸 push のみ現在ブランチ
#     解決で git×1 — 呼び出し内で 1 回だけキャッシュ)。実測 evidence:
#     .omc/logs/run-manual/t5-perf-evidence.txt (修復前: grep 11/22/44/88 @
#     1/2/4/8 セグメント → 修復後: 全ペイロード grep 0)。
# 2026-06-24 改訂: feature ブランチへの git push を許可し、master/main（本線）への
#   直接 push のみブロック（org-feedback #150/#151）。破壊的リセット/削除・force
#   push・--all/--mirror は常に拒否（本線保護維持）。

# ── UC-3: エスケープハッチ ── フックプロセスの環境変数のみから読む。
#   AC-3 (cycle-44 major-3): ハッチ使用は無言にしない。stdin を先に消費してから
#   (producer 側 jq の SIGPIPE 書き込みエラーも防止) stderr へ監査行を出力して
#   exit 0 する。stdout には書かない (exit 0 時の stdout は hook 出力として
#   解釈されうるため)。コマンド本文中の `DISABLE_GIT_GUARD=1 git ...` 前置きは
#   本プロセスへ伝播しないため無効 (ハーネス newcase-i 敵対ケースで検証)。
if [ "${DISABLE_GIT_GUARD:-}" = "1" ]; then
  HATCH_INPUT=$(cat)
  HATCH_CMD=$(printf '%s' "$HATCH_INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null)
  echo "GIT_GUARD_DISABLED: escape hatch used (DISABLE_GIT_GUARD=1) — command NOT scanned: ${HATCH_CMD:-$HATCH_INPUT}" >&2
  exit 0
fi

set -f  # トークン反復で glob 展開しない (ワイルドカードトークンはリテラル扱い)

INPUT=$(cat)
# CRLF (\r) はパース前に strip (Windows checkout / CRLF 混入コマンド対策)。
#   AC-1: 純 bash 置換 (tr プロセス起動なし)。
INPUT=${INPUT//$'\r'/}

# ── 常にブロック（Wiki でも拒否）─────────────────────────────────────────
# 破壊的リセット/削除 + force push + 全ブランチ一括 push
#   ※ --all / --mirror は feature だけでなく master/main も含めて push するためブロック
#   ※ §4 の 11 パターン。セグメント正規化文字列 (非クォートトークン列) に適用し、
#     `push --force` 等のトークン隣接性は旧 RAW grep と等価に維持する。
ALWAYS_BLOCK_PATTERNS=(
  "git reset --hard"
  "git clean -fd"
  "git clean -f"
  "git branch -D"
  "git checkout \."
  "git restore \."
  "push --force"
  "push -f"
  "reset --hard"
  "push --all"
  "push --mirror"
)

DISPLAY=""

block() {  # $1: 理由 (stderr 出力 + exit 2)
  echo "BLOCKED: '$DISPLAY' — $1 The user has prevented you from doing this." >&2
  exit 2
}

# --force-with-lease 系トークン (bare / =ref / =expect:update) を単語単位で除去。
#   ※ 除去後のトークン列に --force が残れば無条件 force push として BLOCK (UC-4)。
strip_lease_tokens() {  # $1: 文字列 → stdout
  local w out=""
  for w in $1; do
    case "$w" in
      --force-with-lease|--force-with-lease=*) ;;
      *) out="$out $w" ;;
    esac
  done
  printf '%s' "$out"
}

# セグメントの qflag トークン列 (クォート剥がし済み単語トークン含む) が
#   `git ... push` を含むか (push セグメント判定)。`git "push"` 等のクォート
#   付き push 語も検出 (AC-2)。git より後ろに push が出現すればよい
#   (git -C repo push 等の global flag を許容)。
is_push_segment() {  # $1: qflag
  local w seen_git=0
  for w in $1; do
    if [ "$w" = "git" ]; then seen_git=1; fi
    if [ "$w" = "push" ] && [ "$seen_git" = "1" ]; then return 0; fi
  done
  return 1
}

# ── §6 本線保護 (意味不変) ── push セグメントの full 文字列 (クォート済 refspec
#   含む) に適用。refspec 判定 + 裸 push の現在ブランチ解決。
# AC-1 (T5 remediation): 本線保護判定も bash ネイティブ照合 ([[ =~ ]] / トークン
#   フィルタ) で行い grep/sed プロセスを起動しない。TRUNK_RE は §6.1 の境界
#   アンカー正規表現 (変数保持 — [[ =~ ]] での括弧類パース安定化のため)。
#   入力文字列は jq fmtseg が改行/タブを空白へ正規化済みのため、grep -E の行頭
#   アンカー解釈と bash =~ の文字列頭解釈は一致する。
TRUNK_RE='(^|[^[:alnum:]./_-])(master|main)([^[:alnum:]./_-]|$)'
# 裸 push 判定の除去対象トークン (旧 sed 置換群のトークン化。完全一致のみ除去 —
#   旧 sed の前方部分一致 (`-next` → 残り `ext`) は「残り非空」で同一判定)。
BENIGN_PUSH_FLAGS='-u|--set-upstream|--force-with-lease|--tags|--no-tags|--dry-run|-n|--quiet|-q|--verbose|-v|--follow-tags'
# 現在ブランチ解決 — 呼び出し内 1 回だけ git を起動してキャッシュ (AC-1:
#   `git push && git push && ...` の複数裸 push セグメントでも git 起動は 1 回)。
_BRANCH_CACHE=""
current_branch() {  # _BRANCH_CACHE へ結果を設定 (stdout 返しではない — コマンド
  #   置換 `$(current_branch)` はサブシェルで動きキャッシュ書込みが消えるため)。
  #   "resolved:" 接頭辞は「解決結果が空」もキャッシュ済みと区別する sentinel。
  if [ -z "$_BRANCH_CACHE" ]; then
    _BRANCH_CACHE="resolved:$(git rev-parse --abbrev-ref HEAD 2>/dev/null || printf '')"
  fi
}

trunk_check() {  # $1: 判定対象文字列
  local s="$1" w refspec="" skip=0 i cur
  # (1) refspec に master/main がスタンドアロントークンとして含まれる → BLOCK。
  #     境界文字類 [^[:alnum:]./_-] で挟むことで feat/master-fix 等は誘爆しない。
  if [[ $s =~ $TRUNK_RE ]]; then
    block "push の refspec が master/main を指している。feature ブランチへの push は許可されますが、master/main への直接 push は禁止です。PR 経由でマージしてください。"
  fi
  # (2) 裸 push (`git push` / `git push origin` / `git push origin HEAD`) は現在
  #     ブランチを push する → 現在ブランチが master/main なら BLOCK。
  #     旧 sed 群と同じく、文字列がちょうど `git push` で始まる場合のみ接頭辞を
  #     落とす (`git -C repo push ...` は接頭辞不一致 → 全トークン残存 → 非裸扱い)。
  local -a toks=($s)
  if [ "${#toks[@]}" -ge 2 ] && [ "${toks[0]}" = "git" ] && [ "${toks[1]}" = "push" ]; then
    skip=2
  fi
  for (( i=skip; i<${#toks[@]}; i++ )); do
    w="${toks[i]}"
    case "$w" in
      $BENIGN_PUSH_FLAGS) ;;
      origin) ;;
      https://*|http://*) ;;
      git@*) ;;
      HEAD) ;;
      *) refspec="$refspec $w" ;;
    esac
  done
  if [ -z "${refspec# }" ]; then
    current_branch
    cur="${_BRANCH_CACHE#resolved:}"
    if [ "$cur" = "master" ] || [ "$cur" = "main" ]; then
      block "現在ブランチ '$cur' 上の裸 push（本線へ直接 push される）。"
    fi
  fi
}

# ── fail-closed: 旧 RAW 全文スキャン (フォールバック) ──
#   jq 解析エラー・空出力・タイムアウト時に到達。ガードを弱めないため
#   危険 payload は従来どおり BLOCK する (ハーネス newcase-j)。
raw_fallback() {  # $1: 生テキスト (INPUT 全文)
  # AC-1 (T5 remediation): RAW フォールバックも grep/sed プロセスなしの bash
  #   ネイティブ照合。lease トークン除去はトークンフィルタで旧 sed 置換
  #   (`--force-with-lease` / `--force-with-lease=...` 語の除去) と等価に実装。
  #   パターン照合はトークンを単一空白で再結合した文字列に対する [[ =~ ]]
  #   (旧 grep -qE と同じ POSIX ERE 部分一致。旧実装は連続空白で一致漏れが
  #   あった点を正規化 — fail-closed 方向の強化)。
  local text="$1" pattern stripped="" w
  local -a kept=()
  for w in $text; do
    case "$w" in
      --force-with-lease|--force-with-lease=*) ;;
      *) kept+=("$w") ;;
    esac
  done
  stripped="${kept[*]}"
  for pattern in "${ALWAYS_BLOCK_PATTERNS[@]}"; do
    if [[ $stripped =~ $pattern ]]; then
      block "(RAW fallback) dangerous pattern '$pattern' に一致。"
    fi
  done
  if [[ $text =~ (^|[[:space:]])git[[:space:]]+push ]]; then
    trunk_check "$text"
  fi
  exit 0
}

# ── セグメント照合 ── $1: plain (非クォートトークン列), $2: qflag (完全クォート
#   「単語」トークンをクォート剥がしで合流させたトークン列 — 多語クォートは
#   除外済み), $3: full (全トークン列)
check_segment() {
  local plain="$1" qflag="$2" full="$3"
  local pattern s full_s w w0 is_git=0
  # git コマンドセグメント判定: qflag 列に standalone `git` トークンを含む。
  #   git セグメントでは完全クォート単語トークン (`"--hard"` 等) もシェル展開後
  #   (= クォート剥がし後) は同一トークンになるため ALWAYS_BLOCK/§5 照合へ
  #   合流する (AC-2, cycle-44 major-2)。非 git セグメントは従来どおり plain のみ。
  for w0 in $qflag; do
    if [ "$w0" = "git" ]; then is_git=1; break; fi
  done
  if [ "$is_git" = "1" ]; then s="$qflag"; else s="$plain"; fi
  s=$(strip_lease_tokens "$s")
  full_s=$(strip_lease_tokens "$full")

  # (A) ALWAYS_BLOCK 11 パターン → 正規化文字列 (lease strip 済み)。
  #   AC-1 (T5 remediation): [[ =~ ]] の bash ネイティブ照合 (変数パターンは
  #   非引用で ERE 適用 = 旧 grep -qE と同じ POSIX ERE 部分一致)。パターン数・
  #   セグメント数に比例した grep プロセス起動なし (0-spawn)。BLOCK メッセージの
  #   パターン帰属表示 ('dangerous pattern X に一致') は旧実装と同一。
  for pattern in "${ALWAYS_BLOCK_PATTERNS[@]}"; do
    if [[ $s =~ $pattern ]]; then
      block "dangerous pattern '$pattern' に一致。"
    fi
  done

  if is_push_segment "$qflag"; then
    # (B) token-aware 強化: push セグメント内の無条件 force / 一括 push トークン。
    #     `git push origin --force feat` (flag が remote 後) の gap closure。
    #     lease 系は strip 済みなので `--force-with-lease origin feat` は引っかからない。
    for w in $full_s; do
      case "$w" in
        --force|-f|--all|--mirror)
          block "push セグメント内の無条件 force / 一括 push トークン '$w'。" ;;
        *) ;;
      esac
    done
    # (C) §6 本線保護 (refspec 判定 + 裸 push 現在ブランチ解決)。
    trunk_check "$full"
  fi
}

# ── jq トークナイザ (Issue #108) ──
#   コマンド文字列を文字単位の状態機械で走査し、セグメント毎に
#   "<plain>\x1f<qflag\x1f<full>" を 1 行ずつ出力する (US=\x1f)。
#     plain = 非クォートトークン列 (パターン照合対象)
#     qflag = plain + fully-quoted SINGLE-WORD tokens, quote-stripped
#             (multi-word quoted strings stay excluded as data) - AC-2
#     full  = 全トークン列 (クォート済み refspec 照合用)
#   - 演算子 && || ; | 改行 (と文頭の & 、`&>` を除く &) でセグメント分割
#   - 完全クォートトークンは plain から除外 / 部分クォート (mas"ter") は非クォート扱い
#   - $(...) と `...` は実行可能につき非クォート扱い (ダブルクォート内も cs スタックで追跡)
#   - heredoc 本体 (<<DELIM / <<-DELIM / <<'DELIM"、複数キュー) はスキップ
#     EXCEPT when the receiver command word is an interpreter
#     (bash/sh/ash/dash/zsh/ksh/csh/tcsh, basename-normalized): the body
#     is then re-scanned recursively as segments (AC-4). Unterminated
#     interpreter heredocs also contribute their accumulated body
#     (fail-closed).
#   - # コメント (単語先頭のみ) は行末までスキップ
#   - 未終端クォートは fail-closed (非クォート扱いで plain に残す)
#   文字コード: tab=9 nl=10 cr=13 sp=32 "=34 #=35 $=36 '=39 (=40 )=41 -=45
#               ;=59 <=60 >=62 \=92 `=96 |=124
JQ_TOKENIZER='
def endword:
  (if (.wu or .wq) then .cur += [{t: (.w | implode), q: (.wq and (.wu | not))}] else . end)
  | .w = [] | .wq = false | .wu = false;

def flushseg:
  endword
  | (if (.cur | length) > 0 then .segs += [.cur] else . end)
  | .cur = [];

def hdnl:
  if (.hq | length) > 0 then .mode = "h" | .hline = [] else .mode = "n" end;

# heredoc receiver command word is an interpreter? (AC-4) Normalize the
#   first segment token via basename (/bin/bash -> bash) and compare.
#   cat / grep etc. keep their heredoc body excluded as data.
def isinterp($w):
  (["bash","sh","ash","dash","zsh","ksh","csh","tcsh"] | index($w | sub(".*/"; ""))) != null;

def hdreg:
  (.cur | length) as $n
  | (if ($n > 0) then (.cur[0].t | sub(".*/"; "")) else "" end) as $cw
  | (if (.hdelim | length) > 0 then
      .hq += [{d: (.hdelim | implode), t: .hdash,
               ic: (($n > 0) and isinterp($cw)),
               b: []}]
    else . end)
  | .hdelim = [] | .hdash = false | .hmp = 0 | .mode = "n";

def normal($ch):
  if $ch.c == 10 then flushseg | hdnl
  elif ($ch.c == 32 or $ch.c == 9 or $ch.c == 13) then endword
  elif ($ch.c == 59 or $ch.c == 124) then flushseg
  elif $ch.c == 38 then (if $ch.a == 62 then .w += [$ch.c] | .wu = true else flushseg end)
  elif $ch.c == 35 then (if (.wu or .wq) then .w += [$ch.c] | .wu = true else .mode = "c" end)
  elif $ch.c == 39 then .mode = "s"
  elif $ch.c == 34 then .mode = "d"
  elif $ch.c == 92 then .mode = "be"
  elif $ch.c == 40 then
    (endword
     | (if .skipp then .skipp = false else .cs += [{ret: "n", bt: false}] end))
  elif $ch.c == 41 then
    (endword
     | (if (.cs | length) > 0 and (.cs[-1].bt | not)
        then .cs[-1] as $f | del(.cs[-1]) | .mode = $f.ret
        else . end))
  elif $ch.c == 96 then
    (endword
     | (if (.cs | length) > 0 and .cs[-1].bt
        then .cs[-1] as $f | del(.cs[-1]) | .mode = $f.ret
        else .cs += [{ret: "n", bt: true}] end))
  elif ($ch.c == 60 and $ch.a == 60 and $ch.b != 60) then endword | .mode = "hm" | .hmp = 0
  else .w += [$ch.c] | .wu = true
  end;

# string -> list of segments (token arrays). Interpreter-fed heredoc bodies
#   (.ihb) are re-scanned recursively and merged into the segment list; the
#   recursion stays inside this single jq process (no extra process fan-out,
#   AC-1 preserved).
def scan:
(explode) as $cs
| [range(0; $cs | length) as $i
   | {c: $cs[$i], a: ($cs[$i + 1] // -1), b: ($cs[$i + 2] // -1)}] as $chars
| reduce $chars[] as $ch (
    {segs: [], cur: [], w: [], wq: false, wu: false, mode: "n",
     cs: [], skipp: false,
     hq: [], hdelim: [], hdash: false, hmp: 0, hdq: 0, hline: [], esc: false,
     ihb: []};
    if .mode == "c" then
      (if $ch.c == 10 then flushseg | hdnl else . end)
    elif .mode == "s" then
      (if $ch.c == 39 then .wq = true | .mode = "n" else .w += [$ch.c] end)
    elif .mode == "d" then
      (if .esc then .w += [$ch.c] | .esc = false
       elif $ch.c == 92 then .esc = true
       elif ($ch.c == 36 and $ch.a == 40) then
         endword | .cs += [{ret: "d", bt: false}] | .skipp = true | .mode = "n"
       elif $ch.c == 96 then
         endword | .cs += [{ret: "d", bt: true}] | .mode = "n"
       elif $ch.c == 34 then .wq = true | .mode = "n"
       else .w += [$ch.c] end)
    elif .mode == "be" then
      .w += [$ch.c] | .wu = true | .mode = "n"
    elif .mode == "h" then
      (if $ch.c == 10 then
         ((.hline | implode) as $line
          | .hq[0] as $hd
          | (if ($hd.t and ($line | test("^\\t+"))) then ($line | sub("^\\t+"; "")) else $line end) as $cmp
          | (if $cmp == $hd.d
               then (.hq |= .[1:])
                    | (if (.hq | length) == 0 then .mode = "n" else . end)
                    | .hline = []
                    | (if $hd.ic then .ihb += [($hd.b | join("\n"))] else . end)
               else (.hq[0].b += [$cmp]) | .hline = [] end))
       else .hline += [$ch.c] end)
    elif .mode == "hm" then
      (if .hmp == 0 then
         (if $ch.c == 60 then .hmp = 1 else hdreg end)
       elif .hmp == 1 then
         (if $ch.c == 45 then .hdash = true
          elif ($ch.c == 34 or $ch.c == 39) then .hdq = $ch.c | .hmp = 3
          elif $ch.c == 10 then hdreg | flushseg | hdnl
          elif ($ch.c == 59 or $ch.c == 124) then hdreg | flushseg
          elif ($ch.c == 32 or $ch.c == 9) then hdreg
          else .hdelim += [$ch.c] | .hmp = 2 end)
       elif .hmp == 2 then
         (if $ch.c == 10 then hdreg | flushseg | hdnl
          elif ($ch.c == 59 or $ch.c == 124) then hdreg | flushseg
          elif ($ch.c == 32 or $ch.c == 9) then hdreg
          else .hdelim += [$ch.c] end)
       else
         (if $ch.c == .hdq then hdreg else .hdelim += [$ch.c] end)
       end)
    else normal($ch)
    end)
| (if .mode == "s" or .mode == "d" then .wu = true else . end)
| (if .mode == "h" and ((.hq | length) > 0) and .hq[0].ic
   then ((.hq[0].b + [(.hline | implode)]) | join("\n")) as $body
   | .ihb += [$body] else . end)
| flushseg
| .segs + ([.ihb[] | scan] | flatten(1));

# segment (token array) -> 3 fields (AC-2):
#   p = unquoted tokens / m = p + fully-quoted single-word tokens (quote-
#       stripped; multi-word quoted excluded via select) / f = all tokens
def fmtseg:
  {p: ([.[] | select(.q | not) | .t] | join(" ")),
   m: ([.[] | select((.q | not) or ((.t | test("\\s")) | not)) | .t] | join(" ")),
   f: ([.[].t] | join(" "))};

([31] | implode) as $us
| scan
| [.[] | fmtseg]
| .[]
| ((.p | gsub("[\n\r\t]"; " ") | gsub($us; " ")) + $us
   + (.m | gsub("[\n\r\t]"; " ") | gsub($us; " ")) + $us
   + (.f | gsub("[\n\r\t]"; " ") | gsub($us; " ")))'

# ── 1) コマンド抽出 (失敗 / 空 → RAW fallback) ──
COMMAND=$(printf '%s' "$INPUT" | jq -er '.tool_input.command | select(type == "string")' 2>/dev/null) || COMMAND=""
if [ -z "$COMMAND" ]; then
  DISPLAY="$INPUT"
  raw_fallback "$INPUT"
fi
DISPLAY="$COMMAND"

# ── 2) jq 構造解析 (タイムアウト付き; 失敗 / 空出力 → RAW fallback) ──
JQ_BIN=(jq)
if command -v timeout >/dev/null 2>&1; then
  JQ_BIN=(timeout 5 jq)
fi
SEGS=$(printf '%s' "$COMMAND" | "${JQ_BIN[@]}" -Rs -r "$JQ_TOKENIZER" 2>/dev/null) || SEGS=""
if [ -z "$SEGS" ]; then
  DISPLAY="$INPUT"
  raw_fallback "$INPUT"
fi

# ── 3) セグメント毎照合 ──
US=$(printf '\037')
while IFS= read -r seg; do
  case "$seg" in "") continue ;; esac
  plain="${seg%%"$US"*}"
  rest="${seg#*"$US"}"
  qflag="${rest%%"$US"*}"
  full="${rest#*"$US"}"
  check_segment "$plain" "$qflag" "$full"
done <<< "$SEGS"

exit 0
