# RULES — 実測で確定した一行ルール

構造・復旧・検証で「破ると実際に何が起きたか」を生ログで示せたものだけを載せる。
追記は 1 行 1 ルール。根拠は `defects_log.md` と `.omc/logs/` の生ログ。

## pipeline / recovery

- boot grace は「プロセスが実際に起動した」ときだけ張る — 配置 clamp のように何も起動しない回復で張ると、grace 中は回復も発火診断も止まり、90s goal が 180s grace に飲まれた (DEF-012: `live-20261111-def012/e2e-run.log`, 13 iters / 0 fires)。
- 回復の判断は純関数に閉じる — `plan_recovery(cause, attempt, app_alive)` は Win32 もゲームも async driver も無しで表として検証できる。
- 配置 fault (`GateRejected`) は kill しない — clamp だけ。プロセスの生存は再起動の理由にならない (DEF-011/012)。
- `PipelineState::tick` はアクション実行前に `current` を進める — execute 失敗と verify capture 失敗では `current` を matching task に戻す。戻さないと以降の全サイクルが未到達 task を照合し、発火 0 のまま goal timeout になる (`live-20261111-def012-e2e`: `per_task_matches: []`)。
- `verify_after_fire` は「発火後に消えるテンプレート」にだけ使う — タイトルロゴのように常時見えるテンプレートを verify に使うと必ず失敗し、チェーンが進まない。
- 回復 budget が尽きたら `RecoveryExhausted` で clean stop する — 無限再試行でゲーム状態を壊さない。

## vision / templates

- タップ座標をテンプレート中心や過去値から流用しない — 追跡下の実機フレームで bbox を取り、正規化して出す。旧 `LoadGamePc` 固定 rect は実ラベルより約 280px 上だった (DEF-011)。
- ROI・テンプレート・閾値はコードに置かない — scene TOML / pipeline TOML に置く。
- 実機フレームは `tests/fixtures/` に追跡下で置く — 回帰はその 1 枚を production preprocessing に通して判定する。
- 新規ボタンは `click_self` で押す — `click_rect` の固定 rect は解像度・レイアウト変更で外れる。
- `scenes/title_pc` の scene def ROI は 180px 以下 (`pc_title_pc_scene_templates_load_and_validate`) — 実ラベル幅 192px の針は def にできない。def を諦めて pipeline task 側に ROI を持つ (DEF-013)。
- 到達判定は `watch/` の sentinel (`action = stop`) で行う — `hud_top` のような固定 HUD アンカーは街に存在しない (DEF-008)。チェーンに足すと単一チェーン契約が壊れるので物理分離する。

## 進め方 / ゲート

- pinned 構造テストを書き換えて通さない — データ配置 (`watch/` 分離、新規 `tests/` ファイル) で構造的に満たす。tests-untouched gate は semantic 変更を許さない。
- データ (TOML) を変える前に、そのデータを assert している pinned test を探す — `tap_title.toml` の `next` 変更は engine の `run_once_fires_on_real_title_fixture...` を落とし、テスト側の修正は gate が拒否する (DEF-013)。
- 失敗した試行も残す — 捨てた案は「なぜ捨てたか」とセットで記録する (`docs/architecture/login-e2e-attempts-20261111.md`)。
- Windows の cargo は成功時も stderr に書く — 判定は `Summary` 行で行い、`$LASTEXITCODE` に頼らない。
