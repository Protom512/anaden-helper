//! `anaden-tool plan-tenmei` / `plan-expand` の CLI 振る舞いテスト
//! (終了コード・stdout/stderr・生成ファイル)。Issue #248 offline。
//!
//! モジュール内部 (plan_data / plan_tool) ではなく、ビルド済みバイナリの
//! 振る舞いを黒箱検証する (テスト不改変ゲート対象外の新規テスト)。

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn tenmei_path() -> PathBuf {
    workspace_root().join("data").join("ad_tenmei.toml")
}

fn plan_path() -> PathBuf {
    workspace_root()
        .join("data")
        .join("ticket_digest_plan.toml")
}

fn tool() -> Command {
    Command::new(env!("CARGO_BIN_EXE_anaden-tool"))
}

/// stdout の TOML を値レベルで検証する (entry 順・難易度・優先度)。
#[test]
fn plan_tenmei_stdout_is_machine_parseable_plan() {
    let out = tool()
        .args(["plan-tenmei", "アルド", "--difficulty", "VH"])
        .arg("--data")
        .arg(tenmei_path())
        .output()
        .expect("anaden-tool must launch");
    assert!(out.status.success(), "exit must be 0: {out:?}");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let doc: toml::Value = toml::from_str(&stdout).expect("stdout must be valid TOML");
    assert_eq!(doc.get("version").and_then(|v| v.as_integer()), Some(1));
    assert_eq!(
        doc.get("on_ticket_empty").and_then(|v| v.as_str()),
        Some("stop")
    );
    let entries = doc
        .get("entry")
        .and_then(|v| v.as_array())
        .expect("[[entry]] array");
    assert_eq!(entries.len(), 1, "蝕時ユニガンは除外: {stdout}");
    let first = entries.first().expect("entry[0]");
    assert_eq!(
        first.get("dungeon").and_then(|v| v.as_str()),
        Some("ミグランス城")
    );
    assert_eq!(first.get("difficulty").and_then(|v| v.as_str()), Some("VH"));
    assert_eq!(first.get("priority").and_then(|v| v.as_integer()), Some(1));
}

/// 未知キャラは exit 非ゼロ + stderr にキャラ名 (fail-closed)。
#[test]
fn plan_tenmei_unknown_char_fails_nonzero() {
    let out = tool()
        .args(["plan-tenmei", "存在しないキャラ"])
        .arg("--data")
        .arg(tenmei_path())
        .output()
        .expect("anaden-tool must launch");
    assert!(!out.status.success(), "exit must be nonzero: {out:?}");
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    assert!(stderr.contains("存在しないキャラ"), "{stderr}");
}

/// plan-expand --out の生成物は engine の routine ローダでそのまま検証できる。
#[test]
fn plan_expand_out_file_loads_and_validates_as_routine() {
    let dir = tempfile::tempdir().expect("tempdir");
    let routine = dir.path().join("ticket_digest.toml");
    let out = tool()
        .args(["plan-expand"])
        .arg(plan_path())
        .arg("--data")
        .arg(tenmei_path())
        .args(["--pipeline", "templates/pipelines/ticket_digest"])
        .arg("--out")
        .arg(&routine)
        .output()
        .expect("anaden-tool must launch");
    assert!(out.status.success(), "exit must be 0: {out:?}");

    let def = anaden_engine::load_routine(&routine).expect("generated routine must load");
    assert_eq!(def.name, "ticket_digest");
    assert_eq!(def.steps.len(), 3);
    def.validate(&workspace_root())
        .expect("generated routine must validate against real pipeline");
}
