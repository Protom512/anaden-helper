# weekly_exchange — ノポウ・ロイヤルカンパニー週次交換 pipeline 設計

ステータス: 設計 v1。実装前。優先順 3 番。小さな対話パイプライン。

## 1. コンテンツ仕様

- 場所: 次元の狭間にある「ノポウ・ロイヤルカンパニー」。
- 交換: ツブラの玉 → カードキー等 (**キー 1 = 玉 5**)。各キー **週 10 個まで** (緑・赤それぞれ)。
- リセット: **毎週火曜 12 時**。
- 供給源: VH アナダン クリアでツブラの玉 20 個 / クリア、週間上限 120 個 (120 = 玉で言えば 6 周 分)。
- つまり週次サイクル: ticket_digest が玉を貯める → 火曜 12 時リセット後 (または週末) に玉をキーへ交換 → 交換キーで ticket_digest がさらに回る。

## 2. 状態機械

```
[field_hud (次元の狭間にいる状態で開始)]
  │ 会社入口 (nopou_sign) 検出 → タップ
  ▼
[shop_menu]
  │ 交換リストから対象キー (exchange_key_green / exchange_key_red) 検出 → タップ
  ▼
[exchange_confirm]
  │ 個数確認 (最大まで) → 決定
  │ ├─ 週間上限到達 dialog → 対象をもう一方のキーへ / 両方上限なら終了
  │ └─ 玉不足 → 終了
  ▼
[exchange_done] → ok → 次のキー / [shop_exit] → [field_hud]
```

## 3. 実装メモ

- 配置: `templates/pipelines/weekly_exchange/`。開始前提 = 次元の狭間にいること (前段 pipeline または nav_routes で次元の狭間へ移動してから起動)。
- goal: `Any{ TemplateMatch(both_keys_capped), Timeout(600) }`。
- 実行契機: 火曜 12:05 (OS スケジューラ)。
- ユーザー設定 `exchange.toml`: 交換するキーの優先順 (緑/赤) と在庫目標値。

## 4. 受け入れ基準

- 週次で玉枯渇 or 上限到達まで交換、2 週連続で recovery 0
- 証跡: run.log + report.json を Issue に添付
