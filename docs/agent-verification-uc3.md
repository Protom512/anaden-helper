# エージェント実動作検証 UC-3: 不正入力系 graceful 挟み込みテスト (Issue #69)

実施日: 2026-08-24 / 実施ブランチ: master (未コミット作業ツリー上、実データ破壊なし)

## 検証パターンと結果

| # | パターン | 入力 | 結果 | エラー報告の構造 |
|---|---------|------|------|-----------------|
| 1 | 存在しないエージェント名 | `SendMessage(to="nonexistent-agent-xyz")` | 即座に構造化失敗 | `{"success":false,"message":"No agent named 'nonexistent-agent-xyz' is currently addressable. Spawn a new one or use the agent ID."}` |
| 2 | 不完全なプロンプト（空メッセージ/空宛先） | `SendMessage(to="", message="", summary="")` | ツール呼び出し段階で拒否 | `to must not be empty` (tool_use_error、実行前バリデーション) |
| 3 | 不正パス形式の宛先 | `SendMessage(to="../../etc/passwd")` | 即座に構造化失敗 | `{"success":false,"message":"No agent named '../../etc/passwd' is currently addressable..."}` — パスとして解釈されずエージェント名として扱われ拒否 |

## 結論

- 3 パターンすべてで **構造化されたエラー報告**（success=false + 人間可読メッセージ）が返った
- **暴走なし**: リトライループ・無限待機・連鎖スパンは一切発生しなかった
- **黙死なし**: すべての失敗が呼び出し元へ即座に可視化された
- パターン2は実行前のスキーマバリデーションで拒否されており、不正入力がエージェント実行層に到達しない二段防御を確認
- pipeline-ledger.json 等の実データは不変（検証中にパイプライン実行なし）

## 備考

- 検証はハーネス組み込みの SendMessage 経路で実施。Task1 固定の検証対象エージェント一覧（.claude/agents/ 配下 10 定義 + OMC 内蔵 executor）の前提を変更していない
- 不正入力はエージェント定義側でなくハーネス側で弾かれるため、定義改修を要する失敗は今回なし（別チケット化対象なし）
