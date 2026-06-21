# PC版(Windows) キャプチャ実測寸法 — T2 (Issue #5)

PC版(Windows, 16:9)シーン認識テンプレート作成に先立ち、`Win32Capture`
(PrintWindow + GetDIBits) で取得した PC 版フレームの実測寸法と、それが
正規化パイプラインへどう影響するかを固定化する参照ドキュメント。
T3/T4/T5(テンプレート作成)・T6(E2E 検証) は本ドキュメントの数値を前提とする。

## 1. 実測寸法(GetClientRect)

| 項目 | 値 | 根拠 |
|------|----|----|
| クライアント領域 幅 | **1258 px** | `capture_probe.png` (`file` → `1258 x 708`) |
| クライアント領域 高さ | **708 px** | 同上 |
| アスペクト比 | 16:9 (1258/708 = 1.776...) | 16:9 = 1.777... |
| 平均輝度 mean | 54.85 | RGBA → Y=0.299R+0.587G+0.114B の全画素平均 |
| 輝度分散 σ² | 4091.33 | 黒フレーム(σ²→0)ではない実描画の証拠 |
| 黒フレーム判定 | **content (非黒)** | `BLACK_FRAME_MEAN_THRESHOLD=10.0` (`source.rs:246`) に対し mean=54.85 > 10.0 |

取得経路: `probe_windows_capture` example
(`crates/anaden-device/examples/probe_windows_capture.rs`) が
AnotherEden.exe の可視トップレベル HWND を解決し、`GetClientRect` で
クライアントサイズを取得 → `PrintWindow(PW_RENDERFULLCONTENT=0x2)` +
`GetDIBits` で RGBA ピクセルを取り出す。`capture_blocking()`
(`crates/anaden-device/src/win32_capture.rs:161`) および studio の
`start_windows_inner` (`crates/anaden-studio/src/source.rs:186`) は
同一の GDI 連鎖を使うため、寸法は一致する。

`capture_probe.png` は本リポジトリ直下に実測アーティファクトとして存在する。

## 2. 正規化は走らない(RAW パススルー) — 最高重要度リスク

`ScreenScaler::normalize` (`crates/anaden-vision/src/scale.rs:45`) は
**幅が BASE_WIDTH(1280) 以下ならリサイズせず生画像の複製を返す**:

```rust
pub fn normalize(&self, img: &DynamicImage) -> DynamicImage {
    let sw = img.width();
    if sw <= self.base_w {   // 1258 <= 1280 → ここで早期 return
        return img.clone(); // RAW パススルー
    }
    // ...リサイズ(拡大はしない)...
}
```

PC 版キャプチャ幅 1258 は 1280 以下のため **正規化(1280 基準への縮小)は
一切走らず、1258x708 の生画像がそのまま下流へ渡る**。

### 影響: テンプレート/ROI は RAW 1258x708 空間で定義すること

既存の 20:9 実機テンプレート群(`templates/pipelines/field_loop/*.toml`,
`templates/scenes/field/hud_top.toml` 等)は **幅1280基準の正規化座標系**
で定義されている(実機 Pixel7a 2400x1080 → 正規化 1280x576)。例:

```
# field_loop/tap_bottom.toml — 20:9 正規化空間(高さ 576 に収まる)
roi = [820, 470, 200, 90]   # bottom = 470+90 = 560 <= 576 ✓
```

これを PC 版(raw 1258x708)へ流用すると、y 座標系が異なるためズレ/NoMatch になる。
事実、20:9 テンプレ `hud_tr` は PC 版で conf 0.99 → 0.67 へ劣化(T7 で検証)。

**PC 版テンプレート/ROI はすべて RAW 1258x708 ピクセル空間で定義すること。**
新規の `[roi]` テーブル形式 toml(`templates/scenes/field/diary.toml` 等)は
既に raw-1258 空間で定義されている:

```
# diary.toml — RAW 1258x708 空間(高さ 708 に収まり、576 を超える)
[roi]
x = 337
y = 604        # bottom = 604+94 = 698 <= 708 ✓, かつ 698 > 576(20:9正規化高さ)
width  = 89
height = 94
```

`y+height = 698 > 576` であることが、この ROI が正規化空間ではなく
raw-1258 空間にある決定的証拠(正規化空間なら画面外へはみ出す)。

この不変条件はテスト `pc_capture_1258_wide_passes_through_normalize_raw`
および `pc_roi_in_raw_space_fits_1258x708_bounds`
(`crates/anaden-vision/src/scale.rs`) で固定化されている。

### T3/T4/T5 への要件(approval condition #3)

PC 版テンプレート作成時は **すべての新規 ROI とテンプレ PNG を RAW 1258x708
ピクセル空間で作成**し、各 toml のコメントにその旨を明記すること。
1280-base 正規化空間で作成してはならない。

## 3. 寸法が 1258x708 と異なる場合(DPI / ウィンドウ状態)

`GetClientRect` はプロセスの DPI アウェア状態とウィンドウ状態(最大化/最小化/
DPI スケール変更)に依存する。万一実測が 1258x708 からズレた場合:

- **`--width <measured>` で `device_width` を固定**すること
  (`crates/anaden-cli/src/main.rs:394` 参照)。
  `run_with_windows` は未指定時のみ初回 capture の width で実測するが、
  ズレがある環境では手動ピンが安全。
- 実測値を本ドキュメント §1 の表へ追記し、その値に合わせて ROI を再導出すること。
  ROI の raw 座標はクライアントサイズに線形に依存する。

既定の `Win32Capture::new` / `default_process` は `set_process_dpi_aware`
(PER_MONITOR_AWARE_V2) を呼ぶため、通常は物理ピクセル = 1258x708 が返る。
studio は egui と衝突するため `new_without_dpi` を使う(`source.rs:194`)。

## 4. T1 外部ゲート(KB5094126) — 実測時の制約

PC 版ライブキャプチャには T1 ゲートがある。`launcher-opengl-fix.md` に従い
**KB5094126 がインストール済み(OS build 26200.8655)の場合、AnotherEden.exe が
0xC0000005 でクラッシュ**し、ライブキャプチャ不可(PROCESS_NOT_FOUND になる)。

- OS build 26200.8524 = KB5094126 **未インストール**(キャプチャ可)
- OS build 26200.8655 = KB5094126 **インストール済み**(クラッシュ、キャプチャ不可)

ライブ 3-5 フレーム取得(T2 本文)は KB5094126 未インストール状態で実施すること。
本ドキュメント §1 の数値は KB5094126 適用前に取得した `capture_probe.png` を
実測アーティファクトとする。

## 5. 関連ファイル

- `crates/anaden-vision/src/scale.rs` — `ScreenScaler::normalize`、RAW パススルー保証テスト
- `crates/anaden-device/src/win32_capture.rs` — `Win32Capture::capture_blocking`、`client_size`(GetClientRect)
- `crates/anaden-studio/src/source.rs` — `start_windows_inner` / `capture_windows`(黒フレーム除外 mean<10.0)
- `crates/anaden-cli/src/main.rs` — `run_with_windows`、`--width` による device_width ピン
- `capture_probe.png` — 実測アーティファクト(1258x708, mean=54.85, σ²=4091.33)
