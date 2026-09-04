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
# 2026-06-24 改訂: feature ブランチへの git push を許可し、master/main（本線）への
#   直接 push のみブロック（org-feedback #150/#151）。破壊的リセット/削除・force
#   push・--all/--mirror は常に拒否（本線保護維持）。

# ── UC-3: エスケープハッチ ── フックプロセスの環境変数のみから読む。
if [ "${DISABLE_GIT_GUARD:-}" = "1" ]; then
  exit 0
fi

set -f  # トークン反復で glob 展開しない (ワイルドカードトークンはリテラル扱い)

INPUT=$(cat)
# CRLF (\r) はパース前に strip (Windows checkout / CRLF 混入コマンド対策)
INPUT=$(printf '%s' "$INPUT" | tr -d '\r')

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

# セグメントの非クォートトークン列が `git ... push` を含むか (push セグメント判定)。
#   git より後ろに push が出現すればよい (git -C repo push 等の global flag を許容)。
is_push_segment() {  # $1: plain
  local w seen_git=0
  for w in $1; do
    if [ "$w" = "git" ]; then seen_git=1; fi
    if [ "$w" = "push" ] && [ "$seen_git" = "1" ]; then return 0; fi
  done
  return 1
}

# ── §6 本線保護 (意味不変) ── push セグメントの full 文字列 (クォート済 refspec
#   含む) に適用。refspec 判定 + 裸 push の現在ブランチ解決。
trunk_check() {  # $1: 判定対象文字列
  local s="$1" refspec cur
  # (1) refspec に master/main がスタンドアロントークンとして含まれる → BLOCK。
  #     境界文字類 [^[:alnum:]./_-] で挟むことで feat/master-fix 等は誘爆しない。
  if printf '%s' "$s" | grep -qE "(^|[^[:alnum:]./_-])(master|main)([^[:alnum:]./_-]|$)"; then
    block "push の refspec が master/main を指している。feature ブランチへの push は許可されますが、master/main への直接 push は禁止です。PR 経由でマージしてください。"
  fi
  # (2) 裸 push (`git push` / `git push origin` / `git push origin HEAD`) は現在
  #     ブランチを push する → 現在ブランチが master/main なら BLOCK。
  refspec=$(printf '%s' "$s" | sed -E \
    -e 's/^git push//' \
    -e 's/[[:space:]]+(-u|--set-upstream|--force-with-lease|--tags|--no-tags|--dry-run|-n|--quiet|-q|--verbose|-v|--follow-tags)//g' \
    -e 's/[[:space:]]+origin([[:space:]]|$)/ /g' \
    -e 's/[[:space:]]+https?:\/\/[^[:space:]]+//g' \
    -e 's/[[:space:]]+git@[^[:space:]]+//g' \
    -e 's/(^|[[:space:]])HEAD([[:space:]]|$)/ /g' \
    -e 's/^[[:space:]]+//' -e 's/[[:space:]]+$//')
  if [ -z "$refspec" ]; then
    cur=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || printf '')
    if [ "$cur" = "master" ] || [ "$cur" = "main" ]; then
      block "現在ブランチ '$cur' 上の裸 push（本線へ直接 push される）。"
    fi
  fi
}

# ── fail-closed: 旧 RAW 全文スキャン (フォールバック) ──
#   jq 解析エラー・空出力・タイムアウト時に到達。ガードを弱めないため
#   危険 payload は従来どおり BLOCK する (ハーネス newcase-j)。
raw_fallback() {  # $1: 生テキスト (INPUT 全文)
  local text="$1" pattern stripped
  stripped=$(printf '%s' "$text" | sed -E 's/[[:space:]]+--force-with-lease(=[^[:space:]]*)?([[:space:]]|$)/ /g')
  for pattern in "${ALWAYS_BLOCK_PATTERNS[@]}"; do
    if printf '%s' "$stripped" | grep -qE "$pattern"; then
      block "(RAW fallback) dangerous pattern '$pattern' に一致。"
    fi
  done
  if printf '%s' "$text" | grep -qE "(^|[[:space:]])git push"; then
    trunk_check "$text"
  fi
  exit 0
}

# ── セグメント照合 ── $1: plain (非クォートトークン列), $2: full (全トークン列)
check_segment() {
  local plain="$1" full="$2" pattern plain_s full_s w
  plain_s=$(strip_lease_tokens "$plain")
  full_s=$(strip_lease_tokens "$full")

  # (A) ALWAYS_BLOCK 11 パターン → 正規化文字列 (lease strip 済み)。
  for pattern in "${ALWAYS_BLOCK_PATTERNS[@]}"; do
    if printf '%s' "$plain_s" | grep -qE "$pattern"; then
      block "dangerous pattern '$pattern' に一致。"
    fi
  done

  if is_push_segment "$plain"; then
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
#   "<plain>\x1f<full>" を 1 行ずつ出力する (US=\x1f)。
#     plain = 非クォートトークン列 (パターン照合対象)
#     full  = 全トークン列 (クォート済み refspec 照合用)
#   - 演算子 && || ; | 改行 (と文頭の & 、`&>` を除く &) でセグメント分割
#   - 完全クォートトークンは plain から除外 / 部分クォート (mas"ter") は非クォート扱い
#   - $(...) と `...` は実行可能につき非クォート扱い (ダブルクォート内も cs スタックで追跡)
#   - heredoc 本体 (<<DELIM / <<-DELIM / <<'DELIM"、複数キュー) はスキップ
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

def hdreg:
  (if (.hdelim | length) > 0 then .hq += [{d: (.hdelim | implode), t: .hdash}] else . end)
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

(explode) as $cs
| [range(0; $cs | length) as $i
   | {c: $cs[$i], a: ($cs[$i + 1] // -1), b: ($cs[$i + 2] // -1)}] as $chars
| reduce $chars[] as $ch (
    {segs: [], cur: [], w: [], wq: false, wu: false, mode: "n",
     cs: [], skipp: false,
     hq: [], hdelim: [], hdash: false, hmp: 0, hdq: 0, hline: [], esc: false};
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
          | if $cmp == $hd.d
            then (.hq |= .[1:])
                 | (if (.hq | length) == 0 then .mode = "n" else . end)
                 | .hline = []
            else .hline = [] end)
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
| flushseg
| [.segs[] | {p: ([.[] | select(.q | not) | .t] | join(" ")),
              f: ([.[].t] | join(" "))}]
| .[]
| ([31] | implode) as $us
| ((.p | gsub("[\n\r\t]"; " ") | gsub($us; " ")) + $us
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
  full="${seg#*"$US"}"
  check_segment "$plain" "$full"
done <<< "$SEGS"

exit 0
