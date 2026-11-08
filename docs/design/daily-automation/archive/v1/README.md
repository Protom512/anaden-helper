# デイリー自動化 — 全体設計 (daily-automation)

ステータス: 設計書 v1 (2026-11-01)。実装前。アルテマ攻略wiki 深掘り調査 + コードベース機能インベントリに基づく。

## 1. 背景

これまで本リポジトリは品質ゲート・バグ修正中心で、「何を自動化するか」が未定義のままインフラ (capture/match/act ループ) だけが整備されていた。ユーザーが実際に自動化したいデイリーは以下の 3 系統であり、それぞれ wiki 調査で実コンテンツを同定した。

| ユーザー語 | 実コンテンツ | 根拠 |
|---|---|---|
| 猫日記 | **ねこにっき** (Ver3.7.10, 毎日0時リセット) | <https://altema.jp/anaden/nekonikki> |
| ローグライク | **メビウスの迷宮** (外史「彷徨える少女と久遠の渦」プロローグ後解放。毎回ランダム生成・装備なしLv1・20層) | <https://altema.jp/anaden/mebiusunomeikyuu> — アナデン唯一のローグライク |
| チケット消化 | **ねこの手特急券によるアナダンスキップ周回** (挑戦画面「スキップ」→特急券1+キー消費でクリア画面まで一気に進む。報酬・宝箱・ドロップ・天冥値まで自動獲得) | <https://altema.jp/anaden/nekonotetokkyuken> |

`templates/tasks/{neko_nikki,roguelike,ticket_digest}.toml` の task stub (未実装) がこの 3 つと正確に対応する。

## 2. 公式の日課・週課 (自動化対象の全体像)

「復帰勢がやるべきこと」(2026-04-14 更新) による公式の日課・週課と、本設計での扱い:

| 種別 | コンテンツ | 所要時間 (手動) | 本設計での扱い |
|---|---|---|---|
| 日課 | ねこにっき (ヒント→ネコ探索) | 約5分 | pipeline `neko_nikki` → [neko_nikki.md](neko_nikki.md) |
| 日課 | カードキー消費 (アナダン挑戦) | 約60分 (スキップで短縮可能) | pipeline `ticket_digest` → [ticket_digest.md](ticket_digest.md) |
| 日課 | 広告視聴で石回収 (10個/日) | 約1分 | pipeline `ad_stone` — **PC版に広告機能があるか要確認** (なければ対象外) |
| 週課 | 結実の誓約 (顕現ミッション) | 約30分 | 対象だが難度高 (挑戦バトル) — MVP外・後段 |
| 週次 | ツブラの玉→キー交換 (火曜12時リセット) | 数分 | pipeline `weekly_exchange` → [weekly_exchange.md](weekly_exchange.md) |

自動化対象外と判明したもの:

- **幻象館** (Ver3.12.30): 謎解き・脱出ゲーム (一回限り・日次リセットなし)。ローグライクではない。
- **時層回廊**: 通常アナダン。特急券スキップ対象リストに含まれるのみで単独対象外。
- **釣り**: 既存 `templates/pipelines/fishing` が担当。日次ゲートなし。

## 3. リソース経済とリセット時刻 (スケジューリングの基礎)

| リソース | 供給 | 上限 | 消費先 |
|---|---|---|---|
| グリーンキー | 毎日 0/6/12/18時 +1 (計4/日) | 9 | ハードAD(1), 異境(2) |
| レッドキー | 同上 (共通枠) | 5 | ベリーハードAD(1), 封域(1) |
| ねこの手特急券 | プレミアム導証 (星天250/大地50 枚分/月) | 50 | スキップ周回 (特急券1+対応キー1) |
| ツブラの玉 | VHクリア 20個, 週間上限120 | — | ノポウ・ロイヤルカンパニーでキー等と交換 (キー1=玉5, 週10個/種, 火曜12時リセット) |
| ねこにっき | 毎日0時リセット (ランダム報酬) / スタンプは火曜0時リセット | 枠は第2部(84章)クリアで3枠 | — |

アナダン報酬枠の解放天冥値: VH 24/60/120, H 条件なし/24/96, 異境 48/120/180 (括弧内H), ガルレア大陸 24/60/120/240/360 (5枠)。詳細な天冥アップキャラ対応は [data/ad_tenmei.toml](data/ad_tenmei.toml)。

## 4. 設計原則: 観測駆動 FSM (マクロ再生ではない)

「手動でやった通りをスナップショット再生する」方式は、実行時に同じ画面列が現れる保証がないため採らない。本設計:

1. **状態 = 検出可能な画面**。各コンテンツで到達しうる画面を網羅列挙し、各画面に一意なアンカー template (ボタン/アイコン/枠) を割り当てる ([ui-atlas.md](ui-atlas.md))。実行は毎 tick「今どの画面か」を判定してから行動する closed-loop。
2. **分岐は画面検出結果で決める**。猫が移動する/しない、幻璃境の選択肢が出る/出ない、キー切れ — 全て観測で分岐。固定シーケンスに依存しない。
3. **テンプレート収集はセンサーキャリブレーション**であって設計ではない。設計書 (本ディレクトリ) が先で、収集は UI アトラスの仕様に従って studio collector で行う。
4. **fail-closed**: NoMatch が閾値 (既定 5) 続いたら停止 + 診断スクショ (`PipelineDriver` の snapshot/diagnose dumper を使用)。偽 green を出さない。

## 5. フレームワーク能力とギャップ

使えるもの (実装済み):

- capture: `PrintWindow(PW_RENDERFULLCONTENT)` → レターボックス黒帯クロップ → 1258x708 正規化空間 (`anaden-vision/src/scale.rs`, `letterbox.rs`)
- match: SSE / CCOEFF、per-task ROI + threshold (`anaden-vision/src/pipeline.rs: TaskDef`)。OCR なし
- act: `Action::{ClickSelf, ClickRect, Swipe, DoNothing, Stop}` (マウスのみ)
- loop: capture→match→act→goal、`StopCondition::{LoopCount, TemplateMatch, Timeout, All, Any}`、NoMatch 連続で RecoveryHook (ゲーム再起動)、`run_once_verified`
- routine: `templates/routines/daily.toml` で複数 pipeline 直列、report.json / run.log 証跡
- studio: テンプレート collector、batch confusion matrix、パイプライン編集

ギャップ (設計で補う / Issue 化):

| # | ギャップ | 対処 |
|---|---|---|
| 1 | `advance_next` は `next[0]` のみで条件分岐なし | 分岐は「どのテンプレートがマッチしたか」で表現 (現行モデルの範囲内で設計) |
| 2 | 常時監視タスク (interrupt/watcher) なし — レベルアップ・実績・アップデート告知等の不定期ポップアップ | エンジン拡張 Issue |
| 3 | OCR なし | 固定文字列テンプレート方式 (ねこにっきヒント73種は固定文、数字は 0-9 の 10 template) |
| 4 | ルーチン間変数共有なし (残り特急券数等) | データ駆動タスク列の静的展開で回避 |
| 5 | タップジッタ/ランダム待ちなし (固定 interval) | エンジン拡張 Issue |
| 6 | フィールド歩行ナビなし | ミニマップ羽根アイコン追従方式で設計 (neko_nikki.md) |

## 6. 実行スケジューリング (エンジン外)

`templates/routines/daily.toml` を以下に再構成する:

```
login → neko_nikki (0時リセット後) → ad_stone (PC版対応確認後) → ticket_digest
weekly (火曜12時後): weekly_exchange / 結実の誓約 (後段)
```

実行契機は OS タスクスケジューラ: 毎日 0:05 / 6:05 / 12:05 / 18:05 (キー回復 0/6/12/18時の直後) + 火曜 12:05。ゲーム起動は `anaden run --ensure-open --recover-launch` が担う。

## 7. 実装優先順

1. **ticket_digest** — FSM 最小・戦闘ゼロ・日次価値最大 (キー4本/日相当)。まずここ。
2. **neko_nikki** — 日次5分の省力化。ヒントテンプレート収集が律速。
3. **weekly_exchange** — 週1回の小さな対話パイプライン。
4. **roguelike (メビウス)** — ランダムマップ探索制御という新能力が必要なエピック。別フェーズ。

## 8. 検証方法

- 各 pipeline: `.anaden-run/report.json` (iterations / fired_commands / stop_reason / recovery) を evidence として保存 (プロジェクト evidence 規約に準拠 — 生コマンド出力のみ evidence)。
- オフライン: `anaden-tool detect` + キャプチャ corpus (状態×分岐の PNG セット) でアンカー検出試験、studio batch confusion matrix。
- 実機受け入れ基準 (例):
  - ticket_digest: 連続5日、キー/特急券枯渇までノンストップ消化、recovery 発動 0
  - neko_nikki: 3枠 × 連続3日完了、猫再移動分岐も完走
  - いずれも run.log の該当行を Issue に貼る

## 9. 明示的な未解決 / 要実機確認

誤魔化さないリスト。設計はこれらを「実機確認後に確定するパラメータ」として扱う:

- [ ] スキップボタンの正確な出現画面 (難易度 dialog 内か挑戦確認画面か)
- [ ] ヒント文の描画が固定スケールか (正規化空間でテンプレート一致するか)
- [ ] PC版にミニマップ表示があるか・羽根マーカーがミニマップに映るか
- [ ] PC版に広告視聴機能 (ad_stone) があるか
- [ ] メビウス 4 層直行の実現可能性 (ランダム性の度合いの実測)
- [ ] 入力 (SendInput の foreground 制約) は本設計の前提条件 (設計スコープ外)

## Sources

- 日課・週課の公式まとめ: <https://altema.jp/anaden/hukki>
- ねこにっき: <https://altema.jp/anaden/nekonikki>
- 特急券: <https://altema.jp/anaden/nekonotetokkyuken>
- アナダン一覧 (天冥・報酬枠): <https://altema.jp/anaden/anotherdungeon>
- カードキー: <https://altema.jp/anaden/getgreenkeys-3642>
- メビウスの迷宮: <https://altema.jp/anaden/mebiusunomeikyuu>
- 幻象館 (対象外判定): <https://altema.jp/anaden/gensyoukan>
