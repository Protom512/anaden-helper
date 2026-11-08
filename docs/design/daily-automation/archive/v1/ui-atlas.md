# UIアトラス — テンプレート (アンカー) 収集仕様

ステータス: v1。ROI・threshold は取り決めの初期値で、収集時に studio collector の弁別性スコアで確定させる。**テンプレート収集はセンサーキャリブレーションであり、設計 (状態機械) は各 md を正とする。**

共通ルール:

- 座標系は正規化空間 (1258x708、`anaden-vision/src/scale.rs` の `PC_CLIENT_*_MEASURED`)。task TOML の `roi = [x, y, w, h]` はこの空間で記述 (`ClickRect` は raw 空間仕様なので注意 — `roi_to_normalized` で変換)。
- algorithm は原則 `ccoeff` (文字・アイコン)、`sse` (ボタンなど単純形状) を試し、batch confusion matrix で誤検出 0 を確認してから確定。
- threshold 初期値 0.90 (文字系) / 0.85 (アイコン) / 0.80 (ボタン)。弁別性が低ければ ROI を狭める。
- 保管先: `templates/scenes/<category>_pc/<anchor>.png` + 同名 `.toml` (task 定義)。

## 1. 共通 (`scenes/common_pc/`)

| anchor | 用途 | 初期 ROI 候補 | 備考 |
|---|---|---|---|
| `menu_btn` | フィールド左下メニュー | 画面左下 | 既存 scenes 流用可 |
| `close_btn` | 各種ダイアログ右上 ✕ | 右上 | 既存 `common/close_btn.png` あり |
| `ok_btn` | リザルト/確認の OK | 中央下 | |
| `cancel_btn` | キャンセル | 中央下 | |
| `levelup_popup` | 実績/レベルアップ通知 | 中央 | watch 対象 |
| `stone_popup` | クロノス石獲得通知 | 中央 | watch 対象 |
| `update_notice` | アップデート告知 | 中央 | watch 対象 |
| `loading_indicator` | ローディング演出 | 中央 | Timeout 判定補助 |
| `battle_auto_on` | 戦闘オート有効アイコン | 戦闘 HUD | neko_nikki 前提チェック |
| `battle_end` | 戦闘終了画面アンカー | 中央 | |

## 2. ねこにっき (`scenes/neko_pc/`)

| anchor | 用途 | 数 |
|---|---|---|
| `diary_tab` | メニュー内 Diary タブ | 1 |
| `diary_slot_frame` | 日記リストの枠 | 1 (枠位置は ROI で 3 分) |
| `pawmark_done` | 完了済み枠の足跡マーク | 1 |
| `hint_text_{01..73}` | ヒント文 (固定 73 種、1 行目 crop) | 73 |
| `feather_marker` | フィールド/ミニマップ上の羽根ペン アイコン | 1 |
| `cat_sprite_{a,b,c}` | 猫スプライト (色/姿バリエーション) | 実機観測で確定 |
| `nekozara_barrel` | 次元の狭間上部タル (ネコの皿入口) | 1 |

hint_text の収集運用: その日のヒント表示画面を collector で 1 枚 → 対応する hint_id の ROI を crop して `template_status = collected` に更新。73 種の進捗は `data/nekonikki_hints.toml` で管理 (未収集ヒントは fail-closed skip)。

## 3. 特急券 (`scenes/ticket_pc/`)

| anchor | 用途 | 数 |
|---|---|---|
| `worldmap_era_{modern,ancient,future,other}` | 世界地図の時代タブ | 4 |
| `region_item_{...}` | 地方一覧の項目 (スクロール対応) | 対象地域分 |
| `dungeon_list_item_{...}` | AD 選択リスト項目 | 対象 36+ |
| `difficulty_hard` / `difficulty_vh` | 難易度 H / VH ボタン | 2 |
| `skip_btn` | 画面下中央スキップボタン | 1 |
| `skip_confirm` | スキップ実行確認 | 1 |
| `key_icon_green` / `key_icon_red` | キー所持数のアイコン (数字 ROI の基準) | 2 |
| `digit_{0..9}` | 所持数数字 (キー/特急券) | 10 |
| `ticket_icon` | 特急券アイコン | 1 |
| `key_zero_dialog` | キー不足ダイアログ | 1 |
| `whitekey_glow` | ホワイトキー所持時の扉発光 | 1 |
| `genrikyo_option` | 幻璃境の選択肢 | 1 |
| `result_ok` | リザルト OK | 1 (共通と同素材の可能性) |
| `tsubura_cap_notice` | 週間ツブラ上限通知 | 1 |

## 4. メビウス (`scenes/mobius_pc/`)

| anchor | 用途 | 数 |
|---|---|---|
| `ararat_hub` | アララト広場アンカー | 1 |
| `labyrinth_entry` | 迷宮入口 | 1 |
| `party_confirm` | 編成確認画面 | 1 |
| `charm_slot_{1..3}` | チャーム 3 枠 | 3 |
| `floor_banner_{1..20}` | 階層バナー (層数判定) | 20 (数字部のみで digit 流用も可) |
| `area_exit_{next,warp}` | 次エリア / ワープ移動点 | 2+ |
| `rune_shop` | ルーンショップ対話 | 1 |
| `trap_dialog` | 罠ダイアログ | 1 |
| `sparkle` | キラキラ | 1 |
| `flee_enemy` | 逃亡者エンカウント | 1 |
| `af_button` | アナザーフォース発動 | 1 |
| `boss_select_{a,b}` | 階層ボス選択肢 | 2 |
| `boss_4f_{abetos,chimaira}` | 4 層ボス識別 | 2 |
| `exit_choice` | 脱出選択肢 | 1 |
| `strong_enemy_warn` | 強敵接近警告 | 1 |

## 5. 週次交換 (`scenes/nopou_pc/`)

| anchor | 用途 |
|---|---|
| `dimension_rift_hub` | 次元の狭間アンカー |
| `nopou_sign` | ノポウ・ロイヤルカンパニー入口 |
| `exchange_key_green` / `exchange_key_red` | 交換リストのキー項目 |
| `exchange_confirm_max` | 個数最大確認 |
| `weekly_cap_dialog` | 週間上限ダイアログ |

## 6. 収集手順 (標準フロー)

1. studio を起動しライブキャプチャで対象画面を表示。
2. 本アトラスの anchor 名で新規タスク作成 → ROI 候補 (proposals) から選択、または手動 ROI。
3. 弁別性サマリ (sensitivity/specificity) を確認 → 0.90 / 0.70 を下回る場合は ROI 調整。
4. 保存 → `templates/scenes/<category>_pc/<anchor>.{png,toml}`。
5. batch confusion matrix で同カテゴリ内の誤対を排除。
6. `data/*.toml` 側の `template_status` を `collected` に更新。
