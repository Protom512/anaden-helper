# pr-reviewer-protom512

- 作成日 2026-10-07 / 材料: 依頼文 + Issue #212 本文 + repo の git log。質問は未回答
- 読み手と書き手が同一人物か: no (読み手 = リポジトリ owner、書き手 = agent)

## 背景（事実）

- anaden-helper の owner。Issue #212 を起票し、座標系の依存構造
  (capture → crop_to_content → normalize(1280) → roi detect、発火は CropInfo で逆変換)
  を自分で文章化している — Issue #212 本文
- 直近の文脈: #210 recovery 再起動ストーム、#209 login/nav 検出アンカー切替
  (title_logo_corner) を経て #212 に到達 — git log
- PR 運用: evidence は生コマンド出力・実測値で示す契約
  (`.claude/rules/pipeline-evidence-verification.md`)

## この分野で既に知っていること（事実）

- Issue #212 の内容すべて (自ら起票): offline 再現手順、既知の座標系依存構造、
  対応案 3 つ、受け入れ基準 4 つ
- MAA 流の 720p 基準座標系 (幅 1280 基準・X/Y 別比スケール) と
  raw-1258x708 空間でのテンプレート/ROI 定義 — `crates/anaden-vision/src/scale.rs` の
  doc コメント群 (本人の書いたコード)
- テンプレートマッチングの仕組み (OpenCV matchTemplate 相当) — repo 全体で运用

## 怪しいところ（推測。資料の芯になる）

- 「offline のテストが green」と「実機 live ループで発火する」の差。
  #209 の時も live 発火は別 verify run で確認しており、offline green だけでは
  受け入れ基準 4 (実機発火) を満たしたと言えないと扱っている — issue 本文の
  受け入れ基準構成からの推測
- aspect guard の適用範囲: 16:9 ±5% という前提が PC 版キャンバス限定であること。
  20:9 縦長端末 (Pixel 7a) ではこの guard の前提が変わる — scale.rs の
  2400x1080 → 1280x576 という実装からの推測 (旧 Android バックエンドは #188 で
  削除済みなので現状は影響なし、と書くのが正しい)
- 受け入れ基準 2「現在の実キャプチャで roi 付き pipeline がマッチ」の字面通りは
  成立しないこと (capture_probe.png が起動途中の過渡フレームで needle が不在) を、
  「マッチしないのが正解」と言い切ってよいか — ここを誤魔化さず見たいはず

## 読み方

- 日本語。コード先・実測値先。図は補助 (コミットメッセージも日本語で詳細)
- PR 本文は 2 分で眺めて差分と対応づけられる長さ

## この資料で省くもの

- Issue #212 の再現手順・座標系の依存構造の再説明 (起票者本人なので既知)
- テンプレートマッチングの一般論、MAA スケーリングの説明
- nextest の使い方など開発環境の説明

## 未確認（本人に聞くこと）

- 実機での login 発火確認 (受け入れ基準 4) をこの PR に含めるか、別 verify run に
  分けるか
- 説明画像 (Mermaid SVG) を PR 添付に使う運用を続けるか

## 出典

- Issue #212 本文 (gh issue view 212)
- git log: 484d613, 9cd11d8, 2e10837
- `.claude/rules/pipeline-evidence-verification.md`
- `crates/anaden-vision/src/scale.rs`, `crates/anaden-vision/src/letterbox.rs`
