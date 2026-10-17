Closes #212

## 何が変わるか

1. **offline 再現が本番と一致する**: `anaden-tool run-pipeline` が engine live 経路 (`PipelineDriver::run_once` / `verify_capture`) と同一の前処理 `ScreenScaler::normalize_capture` を呼ぶ。両経路の前処理は関数として単一化され、テストで pin される (受け入れ基準 1)。
2. **起動途中フレームで座標系が壊れなくなる**: 黒帯クロップに 16:9 ±5% のキャンバス アスペクト保護 (`crop_to_canvas_with_info`) を追加。キャンバス内部の未描画黒 (実測: 1952x1098 の下 519 行が純黒) を黒帯と誤判定して 1952x577 に crop → 1280x378 → roi の Y 比 0.53 倍、という幾何破壊経路を棄却する (受け入れ基準 3)。

![前処理の単一化 — engine と tool が同じ関数を呼んでいれば合格](./preprocess-single-source.png)

## 根因 (offline 段階診断・数値実測)

issue の「全文探索 89% MATCH」は**デジェネレート一致 (SSE 偽陽性)**: capture_probe.png は #210 ストーム期の PrintWindow 過渡フレーム (上 579 行のみ描画・下 519 行純黒・needle は存在せず最良 conf 0.31)。その下に実在した構造バグが 2 つ:

1. **crop 幾何破壊**: `crop_to_content_with_info` が「端から連続する黒行」しか見ないため、端に接するキャンバス内部の黒を真の黒帯と区別できず 1952x577 に crop → normalize **1280x378** → roi Y スケール 0.53 倍に圧縮。
2. **前処理経路乖離**: tool `run-pipeline` は normalize のみ (crop なし) — engine と単一情報源違反。

![キャンバス アスペクト保護 — 内部黒由来のクロップが棄却され幾何が保たれれば合格](./aspect-guard.png)

### 修正 (roi・テンプレ PNG は不変)

| 項目 | 内容 |
|---|---|
| `crop_to_canvas_with_info` | クロップ結果が 16:9±5% 内のみ受理・外れたら棄却して全面使用 (座標系常時保全) |
| `normalize_capture` | 保護付き crop → 1280 正規化を 1 関数に束ね、engine `run_once`/`verify_capture` と tool `run-pipeline` が**同一関数**を呼ぶ単一化 |
| 回帰テスト | 過渡幾何 fixture (`title_boot_degenerate_1952x1098.png`) + 健全幾何 (#208 fixture) の対比 pin・engine 本番経路の発火座標 pin |

弁別の根拠となる閾値の実測 (pin 値 1952x577 / 1942x1098 から機械再計算):

```text
$ node calc-aspect.mjs   # docs/explainers/issue212-pr/ (図と本文の数値の単一源)
degenerate crop 1952x577: aspect=3.383 deviation=+90.3%
unguarded normalize height: 577*1280/1952 = 378 → 1280x378
roi Y factor: 378/708 = 0.53
healthy crop 1942x1098: deviation=0.51% (tolerance 5%)
```

## 実測 — 修正前後の経路比較 (staged example・生出力)

新設 example `issue212_staged` は 1 枚のショットを 4 経路 (A: 無前処理 / B: 素の黒帯クロップ / C: `normalize_capture` = 本番 / D: 旧 tool 相当) に通して突き合わせる。

**過渡ブートフレーム** (Issue #212 の障害フレーム):

```text
$ cargo run --release -p anaden-vision --example issue212_staged -- crates/anaden-vision/tests/fixtures/title_boot_degenerate_1952x1098.png
shot: 1952x1098 "crates/anaden-vision/tests/fixtures/title_boot_degenerate_1952x1098.png"
profile: rows with mean>=8: first=Some(0) last=Some(578) (h=1098; bottom black rows = 519)
[A raw] frame=1952x1098 roi=Some([604, 223, 160, 180]) thr=0.8: None | best(full-scan): conf=0.2046 at (806,31) 186x186
B crop_to_content: offset=(0,0) size=1952x577
[B crop-only] frame=1952x577 roi=Some([604, 223, 160, 180]) thr=0.8: None | best(full-scan): conf=0.3080 at (180,6) 186x98
C normalize_capture: content 1952x1098 (offset 0,0) -> 1280x720
[C normalize_capture (production)] frame=1280x720 roi=Some([604, 223, 160, 180]) thr=0.8: None | best(full-scan): conf=0.2057 at (529,21) 122x122
[D normalize-only (pre-#212 tool)] frame=1280x720 roi=Some([604, 223, 160, 180]) thr=0.8: None | best(full-scan): conf=0.2057 at (529,21) 122x122
```

B が罠の実測 (1952x577)。C は guard がクロップを棄却して全面のまま 1280x720 に正規化し、roi は正しい座標系で評価されて NoMatch — このフレームには needle が存在しないため (全文探索 best 0.2057 < 閾値 0.80)、これが正解。

**健全タイトル画面** (#208 由来の実ショット・左 8px 黒帯):

```text
$ cargo run --release -p anaden-vision --example issue212_staged -- crates/anaden-vision/tests/fixtures/title_live_1952x1098.png
shot: 1952x1098 "crates/anaden-vision/tests/fixtures/title_live_1952x1098.png"
profile: rows with mean>=8: first=Some(0) last=Some(1097) (h=1098; bottom black rows = 0)
[A raw] frame=1952x1098 roi=Some([604, 223, 160, 180]) thr=0.8: conf=0.9856 at (976,406) 186x186 | best(full-scan): conf=0.9856 at (976,406) 186x186
B crop_to_content: offset=(10,0) size=1942x1098
[B crop-only] frame=1942x1098 roi=Some([604, 223, 160, 180]) thr=0.8: conf=0.9822 at (967,406) 185x186 | best(full-scan): conf=0.9822 at (967,406) 185x186
C normalize_capture: content 1942x1098 (offset 10,0) -> 1280x724
[C normalize_capture (production)] frame=1280x724 roi=Some([604, 223, 160, 180]) thr=0.8: conf=0.9944 at (637,268) 122x123 | best(full-scan): conf=0.9944 at (637,268) 122x123
[D normalize-only (pre-#212 tool)] frame=1280x720 roi=Some([604, 223, 160, 180]) thr=0.8: conf=0.9753 at (640,267) 122x122 | best(full-scan): conf=0.9753 at (640,267) 122x122
```

健全幾何では真の黒帯 (1942x1098 = 差 0.51% ≤ 5%) は受理され、C 経路の roi 付き detect が conf 0.9944 で MATCH する。

## テスト — 挙動の pin

回帰テスト (anaden-vision・新規) は (a) 素クロップが幾何を壊すこと (罠の pin)、(b) `normalize_capture` が 1280x720 を維持すること (幾何保全)、(c) roi 付き detect が正しく NoMatch を返すことを保証する。engine 側は実 asset (実キャプチャ fixture + 実 login pipeline TOML) で end-to-end を pin する: 健全 fixture は `Tap { x: 1423, y: 746 }` (rescale 済み実機生座標) で Fired、過渡 fixture は `CropInfo::full(1952, 1098)` のまま NoMatch。

```text
$ cargo nextest run -p anaden-vision --test geometry_drift_regression
        PASS [   1.947s] (1/2) anaden-vision::geometry_drift_regression degenerate_boot_frame_preserves_geometry_and_reports_nomatch
        PASS [   4.634s] (2/2) anaden-vision::geometry_drift_regression healthy_title_geometry_and_degenerate_geometry_both_green
     Summary [   4.634s] 2 tests run: 2 passed, 0 skipped
```

```text
$ cargo nextest run -p anaden-engine -E 'test(run_once_fires_on_real_title_fixture_through_guarded_preprocessing) + test(run_once_on_degenerate_frame_keeps_canvas_geometry)'
        PASS [   1.770s] (1/2) anaden-engine pipeline_driver::tests::run_once_on_degenerate_frame_keeps_canvas_geometry
        PASS [   2.424s] (2/2) anaden-engine pipeline_driver::tests::run_once_fires_on_real_title_fixture_through_guarded_preprocessing
     Summary [   2.424s] 2 tests run: 2 passed, 139 skipped
```

## 検証 (gate・生出力)

- `cargo nextest run --workspace` = **940 passed / 10 skipped** (929→+11)
- 手元再確認 (2026-10-07・クリーン再ビルド後): `cargo nextest run --workspace --no-fail-fast` = 940 tests run: **940 passed** (1 slow), 10 skipped, exit=0
- pc-e2e **30/30 維持**・clippy/fmt/ast-grep/doc 全 green
- **実機検証は未実施** (ゲーム未起動) — offline 代替で「幾何保全 1280x720 + 内容真の NoMatch」を機械保証。実機発火は次のゲーム起動時に実施予定

Release Review 3レーンの VERDICT は review コメント参照。

## 受け入れ基準 (Issue #212) との対応

| AC | 判定 | 根拠 |
|---|---|---|
| 1. run-pipeline が engine live 経路と同一の前処理 (テストで pin) | ✅ | 両経路が `ScreenScaler::normalize_capture` を呼ぶ (`template_tool.rs` / `pipeline_driver.rs`)。engine pin テスト・`title_live_regression` が同関数前提で green |
| 2. 現在の実キャプチャで roi 付き pipeline がマッチ | ⚠️ 要再解釈 | 現行 fixture (capture_probe 由来) は起動途中の過渡フレームで needle が不在 — 全文探索 best conf 0.2057 < 閾値 0.80 のため「マッチしない」が正 (issue の 89% は SSE 偽陽性)。健全幾何 (#208 fixture) は conf 0.9944 で MATCH |
| 3. 幾何が変化しても壊れない構造 (または検出して警告) | ✅ | `crop_to_canvas_with_info` の 16:9 ±5% guard + `geometry_drift_regression` (罠 pin・幾何保全・対比保証) |
| 4. 実機: タイトル画面で login 発火 1 以上 | ❌ 未検証 | 本 PR の検証は offline のみ。実機確認は次のゲーム起動時 |

## 影響範囲と残リスク

- guard の 16:9 前提は **PC 版キャンバス限定**。旧 Android 実機バックエンドは Issue #188 で削除済みのため現状の影響は無いが、将来 20:9 縦長端末経路を復活させる場合は guard の前提を見直す必要がある。
- 過渡フレームは引き続き NoMatch (発火しない)。誤マッチしない方向の fail-safe であり、遷移はリトライ/リカバリに委ねる。
- `MARGIN_PX`・閾値 (8.0 / 64px / 5%) は従来値のまま。5% は健全実測 0.51% と破壊実測 +90.3% の中間に十分広く置いた閾値 (両群の弁別 margin は約 10 倍)。

## 理解度チェック

<details>
<summary>3 問 (クリックで回答)</summary>

1. **`anaden-tool run-pipeline` と engine live 経路の前処理は、この PR の後どう違う?**
   → 違いなし。両方とも `ScreenScaler::normalize_capture` (アスペクト保護付き黒帯クロップ → 幅 1280 正規化) を呼び、テストで pin されている。
2. **起動途中フレーム (下 519 行が純黒) で guard は何を返し、detect は何を返すか?**
   → guard は 1952x577 へのクロップを棄却して全面 (`CropInfo::full(1952, 1098)`) を返し、normalize は 1280x720。detect は needle が不在のため NoMatch (これが正解)。
3. **真の黒帯 (左 8px・crop 後 1942x1098) が棄却されない理由は?**
   → 1942x1098 の 16:9 からの偏差が 0.51% で、許容 5% 以内のため受理。棄却されるのは +90.3% に崩れる内部黒由来のクロップのみ。

</details>

---

- 図 2 枚は Mermaid ソース + 事実シート (`figure-check.mjs` で重なり・はみ出し・枠越え・事実照合を機械検査、両図 CLEAN) から生成し PNG 化したもの。
- 本文の出力引用はローカルの検証資材 (docs/explainers/issue212-pr/: checks.json + verify-doc.mjs) で実行・照合済み (branch には未コミット)。

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: glm 4.7 <noreply@zhipuai.cn>
