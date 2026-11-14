# diagrams

Archify 生成図 (runtime-architecture / sense-think-act) の **ソース JSON** のみを
リポジトリに置く (オペレーター指示 2026-11-14: 生成物 html/png はリポジトリ外管理)。

- `runtime-architecture.architecture.json` / `sense-think-act.sequence.json` が正。
- 2026-11-14 にソース JSON 内の正規化フィルタ表記を修正済み (Lanczos3 → フレーム
  正規化は Triangle、needle リサイズのみ Lanczos3)。#188 以前の構図
  (Android Adapter 等) を含む点は変わらず — 現行アーキテクチャは
  docs/anaden-helper.wiki/Architecture.md を参照。
- 再生成は archify スキルで可能 — follow-up 候補。
