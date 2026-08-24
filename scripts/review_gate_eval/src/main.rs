//! CLI: `review-gate-eval <input.json>` → メトリクス JSON を stdout へ。
//! 入力スキーマは `EvalInput` (lib.rs) 参照。exit codes: 0=成功, 1=IO/解析/集計エラー。

use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: review-gate-eval <input.json>");
        return ExitCode::from(1);
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        eprintln!("cannot read: {path}");
        return ExitCode::from(1);
    };
    let input: review_gate_eval::EvalInput = match serde_json::from_str(&raw) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("invalid input JSON: {e}");
            return ExitCode::from(1);
        }
    };
    match review_gate_eval::aggregate(&input) {
        Ok(metrics) => match serde_json::to_string_pretty(&metrics) {
            Ok(js) => {
                println!("{js}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("serialization failed: {e}");
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprintln!("aggregation failed: {e}");
            ExitCode::from(1)
        }
    }
}
