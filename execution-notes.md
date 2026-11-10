# Goal execution notes

Canonical tracker: canonical_feature_stories.csv (markdown 版は凡例のみ)。

## Done (live session 2, 2026-11-10 午後)

前ライブセッション (同日午前) の続行。アナザーエデン起動中・無人実行で
DEF-008 を実機込みで修正:

- **DEF-008 修正 (街到達 watch レイヤー)**: nav_to_field_pc に
  `watch/field_town_bar.toml` (FieldTownBarPc, Map ボタン針 40x98, ROI
  [400,600,120,108], threshold 0.80, action=stop) を追加。針は実機タウン
  キャプチャから `cut_needle` example で切断し、動的バッジ ("!" / New) を
  含まないよう位置調整。単一チェーン契約 (next.len()<=1 / 唯一 start) を
  **一切変更せず** 到達 OR を実現するため、watch def は `watch/` サブ
  ディレクトリへ物理分離 (load_pipeline は非再帰 → 構造テスト無変更で
  green) + CLI run が `load_watch_defs` (anaden-vision 新 API) を tasks に
  マージ。回帰 `nav_town_arrival_live_regression.rs` (fixture 経由 ≥0.85
  buffer + menu/title 負 + 通貨バー無しの負記録) と分離契約テスト
  `watch_defs_isolation.rs` を追加。
- **ライブ実行証明 (タウン attach 状態)**: `anaden run
  templates/pipelines/nav_to_field_pc TapToStartPc` → cycle 1 で
  `watch stop: FieldTownBarPc` → 停止理由「Stop アクション到達(正常)」、
  **実 exit 0 (パイプ無しで取得)**。report:
  nav_to_field_pc/.anaden-run/20261110-124737*/。US-PIP-01 の「ロード後到達
  判定が街で不能」を watch 経由で解消。
- **テスト不改変 gate との整合**: pinned 構造テスト (cold-start chain /
  walk order) は 1 行も変更せず、新規 tests/ ファイルとデータ配置で契約を
  満した (tests-untouched gate の allowlist は semantic 変更を許さないため
  構造で解決)。
- **studio UC4 ラウンドトリップの watch 対応**: manifest への watch 行追加で
  `all_repo_pipelines_roundtrip_losslessly` が検出 (編集なし保存で watch が
  消える = UC4 契約の正当な検出)。ScenarioEditorState に watch を追加し
  load→save で損失なく保持 (studio 86 tests green)。studio は watch/ subdir
  の def を表示・編集対象にしない (チェーン外レイヤーのまま)。

## Done (live session 2 続き — DEF-009/010, 同日)

無人継続 (人の手ゼロ)。window 操作と実機発火を伴う実験系列:

- **DEF-009 修正**: win32_input の SendInput 経路 (click/swipe) に
  `point_on_any_monitor` ゲート (MONITOR_DEFAULTTONEAREST + rcMonitor 判定)。
  実機陽性: ウィンドウ (2500,100) (右縁 6299 > 3840) で発火座標 (3865,2088)
  が gate 拒否 → 発火 0 / exit 1 (RecoveryExhausted) / log に DEF-009 メッセージ。
  同一実験の gate 前バイナリでは幽霊発火 1 (report 20261110-130613612:
  fired=1, verify.failed=1) — before/after が同一手順で対照証明。
  実機陰性: (0,0) 配置で同発火が gate 通過 → World Map 実オープン (発火 1,
  verify は map 遷移アニメ中の capture で failed=1 — 朝の live run は passed
  でタイミング揺らぎあり)。geometry tests `offscreen_fire_gate_tests.rs`。
- **DEF-010 検出器**: `capture_mode_verdict` (letterbox crop 幅基準,
  閾値 100px) を anaden-vision に追加、driver が Clean↔Chromed 遷移を warn。
  ライブ計測: (a) clean は content 3773/3783 固定、(b) chrome は 3646
  (黒帯 ~137px + タイトルバー混入 + 座標系の縦横比変化 norm 722→694)。
  chrome は recovery 再起動直後 2/2 で発生、移動で解消しない例も観測
  (停滞は非決定的)。chrome 下では針 mismatch が連鎖するため、chrome 空間の
  針を scratch pipeline で切って発火すれば操作可能 (実機で load-click 成功)。
  自動復帰は後続 (relaunch は chrome を再誘発するため要設計)。
- **新規欠陥 DEF-011/012**: nav コールドスタート (recovery がロード中に
  再起動する無限ループ、LoadGamePc 座標は June 時代の疑い) と
  recovery 再起動によるゲーム状態破壊 (town→title, 既定配置 (442,442)
  が DEF-009 状態を再現)。details は defects_log.md。
- **運用知見**: (1) ゲームの再起動で window 位置は前回終了位置を引き継ぐ
  (restart 2 では (442,442), 3 では (0,0))。(2) `--goal-file` は Goal 単体
  TOML (name + [stop]) を取る ([[goal]] 配列でない)。(3) PrintWindow の
  キャプチャは window の画面外移動に非依存 (移動前後で variance 同値)。
  (4) LoopCount ゴールは発火前に到達判定される場合がある (fire 前に stop)。

## Done (ai-coding-loop, 2026-10-10)

- loop 01-05 (前回): StepOutcome enum 構造化 / recovery budget / report channel /
  studio max-iters outcome / template cache (b6c15cb, f8f131b, 9de769d,
  ce26a81, 9bee08c)。
- loop 06 (f7394a3): DEF-006 — studio 開始タスク解決の stem→TaskDef 名修正 +
  manifest 優先 (US-STU-08/09)。nav_to_field_pc に pipeline.toml 整備。
- loop 07 (fd13384): DEF-004 — `anaden run` が未指定時 pipeline.toml の [[goal]]
  を既定ゴールとして採用 (US-GOAL-01)。複数 goal は Any 合成。
- loop 08 (4bb74d3): DEF-005/DEF-007 — launch.toml の --target 誤記、main.rs の
  「PipelineManifest 削除済み」誤主張コメント、README フラグ表の解消
  (US-STU-06/US-DOC-01)。
- loop 09 (ae6dfb6): routine サブコマンドに `--report-file` を追加
  (03-report-channel follow-up 解消)。engine の RoutineSummary を Serialize 化 +
  outcome_label()、studio は PendingReportKind で run/routine のレポート種別を
  明示切替し routine 実行の成否・発火数を履歴/ステータスバーへ反映。
  ゲート: nextest 1014 passed, 10 skipped / clippy -D warnings / fmt /
  ast-grep すべて green。
- loop 09 (検証パス): CSV 67 Unverified を一括検証 — live CLI (--help/--version/
  dry-run) + ストーリー別テストフィルタ実行 (run_exit_code 13 / recovery 20 /
  parse_goal_flag 8 / manifest 42 / goal 135 / studio 452 ほか) + コード突合で
  63 Pass / 4 Blocked (US-PIP-01/02/03/05 = 実ゲーム必要) に確定
  (72 stories: 68 Pass / 4 Blocked / 0 Unverified / 0 Fail)。
- loop 10 (10-routine-default-persist): routine サブコマンドに run と対称な
  既定永続化を追加 (loop 09 follow-up 解消・US-CLI-26)。未指定時は
  `<routine_dir>/.anaden-routine/<ts>/routine-report.json` + `routine.log`、
  `--no-persist` で無効化、明示 `--report-file` は最優先、dry-run は書かない、
  書込失敗は fail-closed (exit 1)。resolver は run_report.rs の共通コア
  (resolve_artifacts_at) へ DRY 化。studio 経路 (明示 --report-file) は無影響。
  ゲート: nextest 1020 passed, 10 skipped / clippy -D warnings / fmt /
  ast-grep すべて green。CSV は 73 stories: 69 Pass / 4 Blocked。

## Done (live session, 2026-11-10)

実機ライブセッション (ツールブート + スプラッシュクリック復旧のゲームで検証):

- **field_loop_pc ライブ検証 (US-PIP-02 Pass 化)**: bottom_stable 再ターゲット
  (ROI [400,600,120,110]→[400,600,120,108]、710>708 の構造ゲート違反修正) +
  hud_tr.png をライブ fixture から再切断 (trio needle 62x30、ROI [1080,90,76,44]、
  threshold 0.80、menu 誤陽性 0.7774 との識別)。tracked fixture
  `field_live_20261110_3783x2128.png` + 回帰テスト
  `field_loop_pc_live_regression.rs` (production preprocessing 経由の detect ≥0.85
  buffer + cross-screen negatives)。
- **ライブ実行証明**: `anaden run --goal-file LoopCount=1` → Tap(1357,1957) 発火 →
  ワールドマップ実オープン → verify-after-fire 1/1 PASS → goal 停止 exit 0
  (report: `.anaden-run/20261110-080226528/report.json`)。
  ゴール停止機構のライブ実証 (US-GOAL-01 の機構部分)。
- **新規欠陥 DEF-008/009/010** (詳細は defects_log.md): 街 field に FieldHudTopPc
  通貨バー無し / ウィンドウ画面外はみ出し時の発火座標が画面外へ (verify failed=1
  でも exit 0) / PrintWindow 取得モード切り替え (chrome 混入 DWM フォールバック、
  signature: キャプチャ内にタイトルバー)。
- **運用知見**: (1) ツールブートの splash-hang はスプラッシュパネルへの実クリック
  1 回で復旧可能 (過去 0/5 諦め前例を覆す)。(2) ウィンドウリサイズ
  (SetWindowPos) でレンダラが停滞しキャプチャが chrome 混入モードへ落ちる
  (D1/D2 実験系列)。(3) 発火前にウィンドウ位置がモニタ内に収まっていることの
  確認が必要 (DEF-009)。

## Open (次回ループ)

1. Blocked 実機検証の残り (US-PIP-01/03/05): タイトル画面到達に再ログインが必要
   (attach-only セッションでは不可能) + US-PIP-01 は DEF-008 によりロード後の
   到達判定自体が街で不能。DEF-008 修正 (街用アンカー再選定、候補: 下部バー
   Map ボタン領域) が先決。
3. routine --report-file / 既定永続化の実 GUI・実機確認: studio から routine
   を spawn しレポート読込が履歴/ステータスバーへ反映されることを実機で確認
   (unit テスト済み・GUI 操作は未実施)。CLI 直実行の既定永続化
   (.anaden-routine/<ts>/ への書込) も実ゲーム実行を要するため live 未確認
   (resolver/prepare/書込/dry-run 不書込は各 unit テストで担保)。
4. F 系 backlog の未消化分 (F5, F8-F10, F12, F14-F16, F18-F33 の一部) は
   探索テーブルが永続化されていないため、再探索が必要な場合は CSV の
   Blocked/notes 行から再導出すること。

## 前ループからの継続エスカレーション (対処禁止 — 人間判断待ち)

1. 複数 `[[goal]]` の Any 合成セマンティクス (loop 07 で暫定採用)。
2. manifest ゴール既定採用の実機検証 (Open 2 と同一)。
3. 未追跡ディレクトリ `.grok/`, `.hermes/`, `personas/`, `skills/`,
   `docs/explainers/` のコミット/gitignore 判断 — これらはそのまま放置する。
