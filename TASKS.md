# TASKS

## 現在の作業（再開ポイント）

### 🔄 TASK-004 SceneDetector（画面→GameState変換）の検証
- **目的**: 複数テンプレートを用いて、実際のゲーム画面がどの GameState に該当するかを判定する
- **再開ポイント**: 投票制誤検出修正＋探索的テンプレート収集ツール（`explore`コマンド）を実装済み（43テスト通過）。次は実機で `explore` コマンドを実行してテンプレートを自動収集し、`detect` コマンドで動作確認する
- **完了条件**: タイトル画面テンプレートで正しく判定できること。タイトル以外の画面で Unknown が返ること。自動収集したテンプレートで新規画面を判定できること
- **メモ**:
  - タイトル画面テンプレート9種: title_text, version_info, support_button, copyright, ver_label, support_text, wfs_mark, load_game_area, title_center
  - **投票制導入**: min_votes=2、単一テンプレートは信頼度0.95以上必要
  - **探索的収集**: `anaden-tool explore <serial>` でキャプチャ→グループ化→安定タイル抽出→検証→保存を自動化
  - 検証基準: 感度 ≥ 0.90（自画面で常にマッチ）、特異性 < 0.70（他画面でマッチしない）
  - 使い方: `cargo run --bin anaden-tool -- explore <serial> --duration 120`

---

## バックログ

### 📋 TASK-008 MAA式パイプラインエンジンの設計
- **目的**: MaaAssistantArknights の JSON パイプラインパターンを参考に、TOML/JSON ベースの宣言的タスク定義システムを設計する
- **完了条件**: パイプライン定義のフォーマット仕様と、Rust 実装の設計書が作成されていること
- **メモ**:
  - **設計書はWikiに完成**: [[Declarative-Tasks-Design]]（TOMLスキーマ・継承・algorithm/action/next）と [[MAA-Pipeline-System]]（MAA原典の解説）
  - 採用フォーマット: **TOML**（追加依存ゼロ・コメント可・serde相性良し）。継承チェーン/シンボル式は文字列表現
  - MAA の中核パターン: JSON で「認識アルゴリズム + アクション + 次タスク」を宣言 → エンジンが実行
  - タスク継承（baseTask）とプレフィクス（@型タスク）による DRY 定義
  - 多ファイルオーバーレイでバージョン差分対応
  - **Rust コード変更なしに** TOML + 画像追加だけで新操作フローを定義可能にする
  - **実装への移行**: `template_store.rs:177` の `parse_state_from_dir_name` ハードコードを TOML の state フィールドへ移行（移行パス段階3）
  - **参考**: https://github.com/MaaAssistantArknights/MaaAssistantArknights （コミットしない）

### 📋 TASK-009 基準座標系の導入（1280x720 基準）
- **目的**: MAA の `ControlScaleProxy` パターンを参考に、全座標を 1280x720 基準で定義し、実行時にスケーリングする
- **完了条件**: Pixel 7a（2400x1080）でも Pixel 7（1080x2400）でも同じテンプレート・ROI 定義が動くこと
- **メモ**:
  - **設計書はWikiに完成**: [[MAA-Resolution-Scaling]]（ControlScaleProxy の比ベース軸選択・screencap IO・逆スケーリング）
  - 根拠: MAA は `AsstTypes.h:28-29` で WindowWidthDefault=1280 / Height=720 を採用。anaden もこれに合わせ MAA の resource 画像/roi 値を直接参照可能に
  - **最大効果の段階1**: orchestrator がキャプチャを 720p へ正規化してから認識へ（現状 `orchestrator.rs:152` はフル解像度渡し）。ROI・テンプレートも基準解像度で統一

### 📋 TASK-010 認識アルゴリズムの戦略パターン化
- **目的**: `VisionEngine` trait を定義し、TemplateMatch / OCR / FeatureMatch を差し替え可能にする
- **完了条件**: algorithm フィールドで認識方法を切り替えられること。将来の CNN/ONNX 追加に耐えること
- **メモ**:
  - **設計書はWikiに完成**: [[Vision-Engine-Design]]（trait スケッチ・コンポーネント表・移行パス）と [[OpenCV-Integration]]（実装クレート判断）
  - **優先順位（移行パス）**: 段階1=720p正規化+ROI → 段階2=CCOEFF_NORMED化(opencv feature flag) → 段階3=宣言的TOML → 段階4=マスク+色F1 → 段階5=キャッシュ → 段階6=FFT/ONNX
  - 現状 `matcher.rs:72` の正規化SSEは照明変動に弱く毎フレーム gray/resize 反復で遅い → CCOEFF+ROI で改善
  - SceneDetector の投票制（`scene_detector.rs`）はそのまま活かす
  - MAA は algorithm フィールド1つで MatchTemplate/OcrDetect/FeatureMatch を切替（`AsstTypes.h:456-462`）
  - **進捗（2026-06-14）**: 段階2の前段として純Rust `CcoeffVisionEngine`（TM_CCOEFF_NORMED, 積分図2本）を実装・テスト緑（`ccoeff.rs`）。SSE を完全維持したまま trait 並列追加。照明+80シフトで SSE 0.52 / CCOEFF 1.00 を確認。デフォルト切替・ROI・opencv feature は残課題。

### 📋 TASK-005 Orchestrator メインループの実機検証
- **目的**: Sense→Think→Act ループを実機で回す
- **完了条件**: 手動でテンプレートを登録した状態で、自動ループが回って画面状態に応じたログ出力ができること

### 📋 TASK-006 CLI エントリポイントの動作確認
- **目的**: `anaden-cli` から起動してデバイスに接続できることを確認
- **完了条件**: `cargo run --package anaden-cli -- --device <serial>` で起動して、デバイス接続メッセージが表示されること

---

## 完了済み

### ✅ M5/M6 ライブADBキャプチャ＋720p基準座標系
- **完了日時**: 2026-06-13
- **成果**:
  - **M5 ライブADB**: 別スレッドでADBキャプチャ（同期Command）、mpscでUIに最新フレームを渡し描画スレッドをブロックしない。serial入力→ライブ開始/停止（Dropでスレッド停止）。停止時の画面でROI作成。
  - **M5 ROI自動提案（2026-06-14追加）**: `proposals.rs`。score_map の **peakiness（鋭く1箇所にマッチする度＝max−中央値）** で候補ROIを提案。collector の「安定=識別」失敗を回避。「💡ROI候補」ボタン→候補リスト→クリックでROI読込→既存スコアで検証（自動採用しない）。テスト7件追加・検証者accept。
  - **M6 720p基準**: anaden-vision に `ScreenScaler`（幅1280基準・アスペクト保存・MAA ControlScaleProxy準拠）。読込/ライブ時に正規化し、ROI・テンプレ・スコアがすべて基準座標系に。Pixel 7a(2400x1080)→1280x576。
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅（studio 18テスト緑）

### ✅ M2/M3/M4 ヒートマップ＋テンプレート保存＋バッチ混同行列
- **完了日時**: 2026-06-13
- **成果**:
  - **M2 ヒートマップ**: ROI候補が画面のどこにマッチするかを score_map（ダウンスケール4）で可視化し、最良マッチ位置をシアン枠でマーク。
  - **M3 テンプレート保存**: ROI切り出しを PNG＋sidecar TOML（状態/ROI/閾値/方式）で保存。既存 TemplateStore 互換。識別力から最適閾値（正例/負例中間）を自動算出。
  - **M4 バッチ混同行列**: ライブラリ全体×ラベル付きテスト画像で「真の状態×予測状態」行列と正答率・テンプレ別感度/特異性を算出。モード切替UI（作成/バッチ）。
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅（studio 10テスト緑）

### ✅ M1 ROI選択＋ライブ識別力スコア（GUI核心機能）
- **完了日時**: 2026-06-13
- **成果**: スクリーンショット上でドラッグROIを選ぶと、正例/負例フォルダに対する識別力（正例最低・負例最高・マージン）を即時表示。自動収集にできなかった「人間がROIを選び即座に検証」ループが実現。
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅（全50テスト緑）
- **詳細**:
  - **anaden-vision 拡張**: `VisionEngine` trait + `SseVisionEngine`（`engine.rs`）、`TemplateMatcher::score_map`（ヒートマップ用）。将来のCCOEFF実装は trait 差し替えで透過導入可能。
  - **anaden-studio**: `canvas.rs`（ドラッグROI選択・座標相互変換）、`scoring.rs`（discrimination純関数・テスト3件）、`app.rs`（正例/負例フォルダ読込＋識別力サマリUI）。
  - 評価エンジンは閾値0・1/2ダウンスケール（生スコア表示＋高速化）。
  - GUI描画はテスト対象外、スコアリング/スコアマップは単体テスト済み。

### ✅ M0 テンプレート作成GUI土台（anaden-studio）
- **完了日時**: 2026-06-13
- **成果**: 新規crate `anaden-studio`（eframe/egui 0.34）が起動しPNG画像を表示できる。DynamicImage→egui TextureHandle 変換の土台確立。
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅（全44テスト緑）
- **詳細**: eframe 0.34 は `App::ui` 必須（`update`非推奨）→ `Panel::left` + `show_inside` モデル採用。`recognition.rs` の前提テスト不整合（DEFAULT_THRESHOLD 0.85→0.95 変更で1件のconfidence更新漏れ）を修正。

### ✅ MAA画像認識ノウハウ調査とanaden-vision再設計Wikiの作成
- **完了日時**: 2026-06-13
- **成果**: MaaAssistantArknights のコードを精読（16エージェント並列）し、画像認識ノウハウ + anaden-vision 再設計の設計図 Wiki 7ページを作成・push済み
- **品質チェック**: RELIABILITY✅（file:line 引用を実検証）PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅
- **詳細**:
  - **新規Wiki 7ページ**（docs/anaden-helper.wiki/）:
    - `MAA-Matching-Algorithms.md` — TM_CCOEFF_NORMD・マスク・FFT/スパース/OpenCV 3経路・色F1・特徴点・ハッシュ
    - `MAA-Recognition-Advanced.md` — PaddleOCR/ONNX・戦闘画面複合認識・動的ROI
    - `MAA-Resolution-Scaling.md` — ControlScaleProxy・720p基準座標系
    - `MAA-Pipeline-System.md` — 宣言的JSONタスク・baseTask継承・Action/next
    - `OpenCV-Integration.md` — opencv crate主軸 + 純Rust CCOEFF フォールバック判断
    - `Vision-Engine-Design.md` — VisionEngine trait再設計図・6段階移行パス
    - `Declarative-Tasks-Design.md` — TOML宣言的タスク設計
  - **設計の核心結論**:
    - OpenCV: **ハイブリッド**（opencv crate 主軸 + 自前Rust CCOEFF フォールバック、feature flag 切替）
    - パイプライン: **TOML** 採用（追加依存ゼロ・コメント可・serde相性）
    - 解像度: **1280x720 基準**（MAA AsstTypes.h:28-29 と同一・知的資産互換）
    - 認識方式: 現状の正規化SSE → **TM_CCOEFF_NORMED + マスク + ROI** へ移行
  - **中国語混入**: 7ファイル全てで「的」助詞0件、簡体字特有字形は0件（MAA引用1箇所も日本語訳付き→更に除去済み）
  - TASK-008/009/010 の設計書が完成。次は実装フェーズ（段階1: 720p正規化+ROI が最大効果）

### ✅ TASK-013 パイプラインランナー純粋ロジックの実装
- **完了日時**: 2026-06-14
- **成果**: 宣言的パイプライン（TaskDef）の実行純粋層を anaden-engine に新設。action→InputCommand 変換・next 状態遷移・1ステップ認識ループ（tick）を、device/async 依存ゼロで実装
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅
- **詳細**:
  - **新モジュール**: `crates/anaden-engine/src/pipeline_runner.rs`（InputCommand / action_to_command / advance_next / PipelineState.tick / TickResult）
  - **StepOutcome 拡張**: `anaden-vision/src/pipeline.rs` の StepOutcome に `matched_region: ScreenRegion` を追加し run_step がマッチ領域を格納するよう更新（ClickSelf のクリック座標計算に必要）
  - InputCommand は Tap{x,y}/Swipe{from,to}（u32 ピクセル、Copy+Eq）。ADB/tokio 非依存の純粋値
  - 新テスト19件: action_to_command 6件 + advance_next 4件 + tick 4件 + state 1件 + 画像合成統合テスト（含 ClickSelf レンジ検証）
  - 実行: `cargo nextest run -p anaden-engine` 全緑 / `cargo nextest run --workspace` 122件全緑（非破壊確認）

### ✅ TASK-012 探索的テンプレート自動収集機能の実装
- **完了日時**: 2026-06-13
- **成果**: `anaden-tool explore` コマンドで、ゲームを操作するだけでテンプレートを自動収集・検証・保存できる
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅
- **詳細**:
  - **新モジュール**: `crates/anaden-vision/src/collector.rs`
  - **Phase 1 グループ化**: ピクセル類似度（MAE）で連続キャプチャを「同じ画面」にグループ化
  - **Phase 2 安定タイル抽出**: 150x100px タイルの分散を計算し、安定（分散低）なものを候補に
  - **Phase 3 厳格検証**: 感度チェック（自画面で ≥ 0.90）＋ 特異性チェック（他画面で < 0.70）
  - **Phase 4 保存**: 検証通過テンプレートを `group_XXX/` に保存。ユーザーがリネームして状態を割り当て
  - 新テスト8件: 類似度3件 + グループ化2件 + タイル抽出2件 + 検証1件

### ✅ TASK-011 テンプレートマッチング誤検出修正（投票制導入）
- **完了日時**: 2026-06-13
- **成果**: 「タイトルじゃないのにタイトルと判定される」問題を修正。投票制で2テンプレート以上の一致を要求
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅
- **詳細**:
  - `find_matches` → `find_best_match`（1テンプレート=1マッチ）
  - 投票制導入: min_votes=2、単一テンプレートは信頼度0.95以上必要
  - 新テスト4件追加

### ✅ TASK-003 テンプレートマッチング画像認識の検証
- **完了日時**: 2026-06-13
- **成果**: Pixel 7a でタイトル画面のテンプレート3種を抽出し、クロスキャプチャで 99%+ の信頼度でマッチ成功
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅

### ✅ TASK-001 Workspace & Core型の実装
- **完了日時**: 2026-06-13
- **成果**: 6 crate の Cargo workspace + ドメイン型が全て定義済み。31テスト全通過
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅

### ✅ TASK-002 ADB デバイス通信層の実装と実機確認
- **完了日時**: 2026-06-13
- **成果**: Pixel 7a にADB接続してスクリーンショット取得・アプリ起動が動作確認済み
- **品質チェック**: RELIABILITY✅ PERFORMANCE✅ EXTENSIBILITY✅ GOVERNANCE✅ SECURITY✅ INTEGRATION✅
