use anaden_studio::history::{DEFAULT_MAX_RECORDS, RunHistory, RunOutcome, RunRecord};
use anaden_studio::tasks_def::{TaskDefinition, TaskError};
use std::path::Path;

fn good_line(strategy: &str, started: u64) -> String {
    let r = RunRecord::new(started, strategy, RunOutcome::Success, Some(0), vec![]);
    serde_json::to_string(&r).unwrap()
}

fn write_history(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
}

#[test]
fn unknown_outcome_variant_parses_instead_of_dropping_the_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.jsonl");
    write_history(
        &path,
        &format!(
            "{{\"started_at_unix\":1,\"strategy\":\"newer-build\",\"outcome\":\"SomeFutureVariant\",\"exit_code\":0,\"log_tail\":[]}}\n{}\n",
            good_line("old", 2)
        ),
    );
    let h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
    let rs = h.records();
    assert_eq!(rs.len(), 2, "unknown outcome must not drop the whole line");
    assert_eq!(rs[0].outcome, RunOutcome::Unknown);
    assert_eq!(rs[0].strategy, "newer-build");
}

#[test]
fn record_missing_core_fields_parses_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.jsonl");
    write_history(
        &path,
        &format!(
            "{{\"started_at_unix\":7,\"outcome\":\"Success\"}}\n{}\n",
            good_line("ok", 1)
        ),
    );
    let h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
    let rs = h.records();
    assert_eq!(rs.len(), 2, "missing core fields must not drop the line");
    let r = &rs[0];
    assert_eq!(r.started_at_unix, 7);
    assert_eq!(r.strategy, "");
    assert_eq!(r.exit_code, None);
    assert!(r.log_tail.is_empty());
    assert_eq!(r.outcome, RunOutcome::Success);
}

#[test]
fn unparsed_lines_survive_persist_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.jsonl");
    write_history(
        &path,
        &format!("{{broken json\n{}\n\nnot-a-record\n", good_line("ok", 1)),
    );
    let mut h = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
    assert_eq!(h.records().len(), 1);
    h.append(RunRecord::new(
        2,
        "new",
        RunOutcome::Success,
        Some(0),
        vec![],
    ))
    .unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("{broken json"),
        "unparsed line erased by persist"
    );
    assert!(
        after.contains("not-a-record"),
        "unparsed line erased by persist"
    );
    assert!(after.contains("\"new\""), "appended record missing");
    let h2 = RunHistory::open(&path, DEFAULT_MAX_RECORDS);
    assert_eq!(h2.records().len(), 2);
}

#[test]
fn unknown_task_key_is_rejected() {
    let toml = "\nid = \"x\"\ntitle = \"X\"\nkind = \"pipeline_run\"\nimplemented = true\npipeline_dir = \"p\"\nstat_task = \"start\"\n";
    let err = TaskDefinition::parse_toml(toml, Path::new("x.toml")).unwrap_err();
    assert!(
        matches!(err, TaskError::Parse { .. }),
        "typo key must fail closed, got {err:?}"
    );
}

#[test]
fn start_task_typo_variant_is_rejected_too() {
    let toml = "\nid = \"x\"\ntitle = \"X\"\nkind = \"pipeline_run\"\nimplemented = true\npipeline_dir = \"p\"\nstart_tasks = \"start\"\n";
    let err = TaskDefinition::parse_toml(toml, Path::new("x.toml")).unwrap_err();
    assert!(matches!(err, TaskError::Parse { .. }), "got {err:?}");
}
