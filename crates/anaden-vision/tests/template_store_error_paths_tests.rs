#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_vision::{TemplateStore, TemplateStoreError};

#[test]
fn corrupt_png_reports_load_failed_and_keeps_store_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("battle");
    std::fs::create_dir_all(&dir).unwrap();
    let img_path = dir.join("broken.png");
    std::fs::write(&img_path, b"definitely not a png").unwrap();

    let mut store = TemplateStore::new();
    let err = store.load_from_directory(tmp.path()).unwrap_err();
    assert!(
        matches!(
            err,
            TemplateStoreError::LoadFailed { ref path, .. } if path == &img_path
        ),
        "err: {err}"
    );
    assert!(store.is_empty(), "failed load must not leave partial state");
}

#[test]
fn base_dir_being_a_regular_file_reports_read_dir_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("plain_file");
    std::fs::write(&file_path, b"x").unwrap();

    let mut store = TemplateStore::new();
    let err = store.load_from_directory(&file_path).unwrap_err();
    assert!(
        matches!(
            err,
            TemplateStoreError::ReadDirFailed { ref path, .. } if path == &file_path
        ),
        "err: {err}"
    );
    assert!(store.is_empty());
}
