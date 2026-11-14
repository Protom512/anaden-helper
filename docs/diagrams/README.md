# diagrams

本ディレクトリの Archify 生成図（runtime-architecture / sense-think-act）は
**Issue #188 (2026-09-10) で Android 端末サポートを削除する以前**のアーキテクチャを
描いた生成物です（Android Adapter / orchestrator 等を含む）。
現行のアーキテクチャは docs/anaden-helper.wiki/Architecture.md を参照してください。

**生成物 (html/png) はソース JSON より古い**: 2026-11-14 にソース JSON 内の
正規化フィルタ表記を修正した (Lanczos3 → フレーム正規化は Triangle、needle
リサイズのみ Lanczos3)。html/png の再生成は未実施で旧表記のまま — 図の
フィルタ表記は `.architecture.json` / `.sequence.json` を正とすること。
（図の再生成は archify スキルで可能 — follow-up 候補）
