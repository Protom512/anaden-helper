# ticket_digest — ねこの手特急券スキップ周回 pipeline 設計

ステータス: 設計 v1。実装前。優先順 1 番 (FSM 最小・戦闘ゼロ・日次価値最大)。

## 1. コンテンツ仕様 (wiki 調査結果)

- 特急券は**クリア済みアナダン (AD) の周回をスキップ**するアイテム。対象 AD の挑戦時に画面下中央「スキップ」から使用。**特急券 1 枚 + 難易度対応キー 1 枚**を消費し、クリア画面まで一気に進む。ダンジョン内移動・戦闘ゼロ。
- 自動獲得: クリア報酬 + 道中宝箱分 + エネミードロップ + **配布キャラの天冥値アップ**。
- 供給: プレミアム導証 (星天 250 / 大地 50 枚分/月、所持上限 50 枚)。
- 制約: 未クリアの AD はスキップ不可 (スキップボタンが出ない)。レアエリアは未遭遇だと出現しない。
- 補足: 特急券枚数がキー所持数に対して十分ある場合、律速はキー (4枚/日)。キー切れ = その日の消化完了。

### 対象 AD 一覧 (Ver3.10.10 時点, 計 36 + 異境 9)

- メイン 20: 月影の森 / ナダラ火山 / 人喰い沼 / ミグランス城 / 工業都市廃墟 / ゼノ・ドメイン / 次元戦艦 / 時の塔 / 星の塔 / 時層回廊 / 魔獣城 / 蛇肝ダマク / トト・ドリームランド / 現代ガルレア大陸 / 古代ガルレア大陸 / 未来ガルレア大陸 / 冥峡界 / オメガポリス / エントラーナ / ジスモンデ
- 外伝 11: ミグランス地下迷宮 / サキの夢意識 / 竜宮城 / マユの夢意識 / ドグマの塔 / 旧KMS本社 / 燃える魔獣城 / クアドラ平原 / ドラマク海洞 / 冥竜王の領域 / プロトエデン
- 外典 3: 古代ゼルベリヤ雷 / 晶 / 陰
- 協奏 2: 異夢・時の塔 / 欲深き者の旅路
- 異境: バルオキー / ラトル / エルジオン地方の **1〜3層** (4層は対象外)

### キー対応

| 難易度 | 消費キー | 備考 |
|---|---|---|
| ハード (H) | グリーン 1 | 上限 9 |
| ベリーハード (VH) | レッド 1 | 上限 5 |
| 異境 (H/VH) | グリーン 2 | 1〜3層スキップ可 |

## 2. 前提条件 (初回のみ手動 — チェックリスト)

- [ ] 対象 AD を全てクリア済みにすること (スキップはクリア済みのみ)
- [ ] オート/速度設定は本 pipeline では不要 (戦闘が発生しないため)
- [ ] 特急券を所持していること (導証加入)
- [ ] ホワイトキーを保持している場合、AD 扉が光り「幻璃境」選択肢が追加される → 分岐処理で対応 (§4)

## 3. 状態機械 (FSM)

状態 = 検出可能な画面。各状態のアンカー template は [ui-atlas.md](ui-atlas.md) 参照。

```
[field_hud]
  │ menu_btn 検出 → クリック
  ▼
[menu]
  │ map_btn 検出 → クリック
  ▼
[world_map]
  │ era_tab (現代/古代/未来/…) 検出 → 対象大陸タブへ
  │ region_item 検出 → 対象地方へ (スクロール必要なら swipe)
  ▼
[dungeon_select]
  │ 対象 AD のリスト項目 (dungeon_list_item_*) 検出 → クリック
  ▼
[ad_difficulty_dialog]  ← ここで分岐が集中する
  ├─ whitekey_glow 検出 (ホワイトキー所持) → genrikyo_option を避けて通常難易度を選ぶ
  ├─ key_count == 0 検出 (digit template) → 次のダンジョンへ (計画テーブル参照) / 全滅なら終了
  ├─ skip_btn 検出 (クリア済み) → クリック → [skip_confirm] → [loading]
  └─ skip_btn 非検出 (未クリア) → その AD を計画から除外し次へ (fail-closed で報告)
  ▼
[result_chain]  (リザルトは 1〜3 段: クリア報酬 / 天冥アップ / 宝箱・ドロップ)
  │ result_ok 検出 → クリック (画面が変わるまで連打)
  ▼
[field_hud] → 計画テーブルの次のエントリへ (loop)
```

常時 watch (将来の engine 拡張、当面は状態として列挙): レベルアップ / 実績 / クロノス石回復 / アップデート告知 / 週間ツブラ上限お知らせ → close_btn で閉じる。

## 4. 分岐網羅 (「必ずそうなるとは限らない」への回答)

| 分岐 | 検出方法 | アクション |
|---|---|---|
| ホワイトキー所持で幻璃境の選択肢が出る | `whitekey_glow` / `genrikyo_option` | 通常難易度 (H or VH) を選択。幻璃境には入らない |
| キー切れ (難易度選択でキー不足) | `key_count_0` (digit 0) + 購入誘導 dialog | 石で購入**しない** (設定)。次の難易度/ダンジョンへ。全滅で goal 到達 |
| 特急券切れ | `ticket_count_0` + skip_btn 無効表示 | 設定 `on_ticket_empty`: `stop` (既定) or `normal_entry` (通常入場へ切替 — 戦闘前提のため別 pipeline に委譲) |
| 未クリア AD (skip_btn なし) | skip_btn 非検出 | 計画から除外、report に記録 (fail-closed) |
| リザルト多段 (宝箱/ドロップ/天冥) | `result_ok` の連続検出 | OK を画面変化まで連打 (`run_once_verified` で検証) |
| 6時間毎のキー回復後の再実行 | routine スケジューラ側 | 0:05 / 6:05 / 12:05 / 18:05 の各契機で起動 |
| ローディング長滞在 | `loading_indicator` 連続 | 既定 Timeout (goal) で打ち切り + RecoveryHook |

## 5. 観測仕様

- キー/特急券の所持数: 難易度 dialog 内の数字を digit template (`digit_0`〜`digit_9`) で読む。OCR 不要。
- 「クリア済み判定」: `skip_btn` の有無そのもの。
- リザルト完走判定: `field_hud` アンカー (既存 `hud_tr.png` 系) の再検出。

## 6. データ

[`data/ticket_digest_plan.toml`](data/ticket_digest_plan.toml):

```toml
# plan_version = 1
[[entry]]
dungeon = "ミグランス城"   # data/ad_tenmei.toml の dungeon id と突合
difficulty = "VH"
priority = 1                # 小さい順に消化
note = "アルド天冥"
```

[`data/ad_tenmei.toml`](data/ad_tenmei.toml): ダンジョン×天冥アップキャラ対応 + 報酬枠の天/冥 alignment。
ツール: `anaden-tool plan-tenmei <char>` (新規) — 「上げたいキャラ」から計画 TOML を逆引き生成する。

## 7. pipeline 実装メモ

- 配置: `templates/pipelines/ticket_digest/` (pipeline.toml + task TOML 群)
- 開始 task: `DetectFieldHud`。goal: `Any{ TemplateMatch("key_zero_banner", 0.9), LoopCount(計画回数×係数), Timeout(3600) }`
- ループ構造: 1 ダンジョン 1 サイクル。計画テーブルは実行時に静的展開 (routine 生成時に task 列へ) — engine 変更不要。
- report: 既定の `.anaden-run/<ts>/report.json`。スキップ成功回数・キー残・未クリア検出を progress に含める。

## 8. 受け入れ基準

- 連続 5 日、recovery 発動 0 でキー枯渇まで消化
- 分岐 4 種 (幻璃境 / キー切れ / 特急券切れ / 未クリア) のうち少なくともキー切れと特急券切れを実際に踏んで正常処理することを 1 回以上確認
- 証跡: run.log + report.json を Issue に添付 (生コマンド出力)
