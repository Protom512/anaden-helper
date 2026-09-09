//! テンプレートバンク一括監査 (Issue #187 提案 1・2)。
//!
//! Issue #184 (PR #186) の Release Review lane3 がバンク内 PNG を一括スキャンし、
//! `templates/pipelines/field_loop/bottom_stable.png` (輝度 stddev 2.89 — 監査実測
//! 2.80) が無構造 (恒久 NoMatch) であることを発見した。#184 で導入した loader warn は
//! 実行時にしか火かないため、CI で常時実行される **監査テスト** として資産側の
//! 品質を固定する:
//!
//! - 参照抽出: `templates/pipelines/**/*.toml` + `templates/scenes/**/*.toml` の各
//!   TOMLから template 参照を parse する (TaskDef schema の `template` キー、
//!   または旧 schema sidecar の `<stem>.png` 慣例) → 実在 PNG を解決。
//! - 各 PNG の輝度 stddev を [`anaden_vision::template_luma_stddev`] (Issue #184 の
//!   単一実装) で測定し、stddev < [`anaden_vision::TEMPLATE_MIN_LUMA_STDDEV`] の
//!   無構造テンプレートを検出する。
//! - allowlist ([`KNOWN_UNSTRUCTURED`]) 外に無構造テンプレートがあれば FAIL する
//!   (テスト失敗時に検出リストを表示)。逆に allowlist 登録済みテンプレートが
//!   再撮影などで構造を取り戻した場合も FAIL させ、allowlist の陳腐化を検出する。
//! - 全件一覧 (パス・stddev) と統計 (件数・最小 stddev) を stdout へ出力し、
//!   レポートを `target/template-bank-audit-report.txt` へ書き出す (コミットされない
//!   ビルド産物置き場。実行の evidence は「テスト green + 本レポート」)。
//!
//! bottom_stable.png (field_loop/android) は実機再撮影が現状不可能 (ゲームは PC 版を
//! 起動中・android 用 pipeline) のため allowlist 登録で経過観察する。再撮影手順は
//! `templates/pipelines/field_loop/tap_bottom.toml` の doc comment に記載している。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anaden_vision::{TEMPLATE_MIN_LUMA_STDDEV, template_luma_stddev};

/// 監査対象のテンプレートバンクルート (workspace `templates/`)。
/// anaden-vision クレート (crates/anaden-vision) からは `../../templates`。
fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates")
}

/// 既知の無構造テンプレート (監査 allowlist)。templates/ ルート相対・
/// フォワードスラッシュ形式で指定する。
///
/// 登録条件: (1) stddev < 閾値で恒久 NoMatch が確定済み、(2) 当面の再生成が不可能
/// (実機再撮影が必要な android 向け資産で、現状ゲームは PC 版を起動中)。
///
/// - `pipelines/field_loop/bottom_stable.png` (監査実測 stddev 2.80 — Issue #184 lane3
///   報告 2.89 と同一資産。TapBottomStable が参照。Issue #182 version_label と同種の
///   ccoeff 恒久 NoMatch)。
///
/// allowlist 登録テンプレートが再撮影で構造を取り戻したらこのリストから削除する
/// (監査テストの stale 検出が削除を促す)。再撮影手順は tap_bottom.toml の doc 参照。
const KNOWN_UNSTRUCTURED: &[&str] = &["pipelines/field_loop/bottom_stable.png"];

/// 監査対象バンクの件数下限。参照抽出やパス解決のバグでスキャンが空になった場合の
/// 偽 green (vacuous pass) を防ぐ fail-closed (pipeline-evidence-verification.md)。
/// 現行バンクは 31 件 (2026-09 実測) — 大幅な減はバンク破壊を疑う。
const MIN_AUDITED_TEMPLATES: usize = 30;

/// `dir` 以下の `*.toml` を再帰的に収集する (決定論的のためソート済みを返す)。
fn collect_toml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_toml_files(&p, out);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
        {
            out.push(p);
        }
    }
}

/// パスを字句正規化する (`.`/`..` 成分の解決。symlink 解決はしない)。
///
/// `pipelines/fishing/../field_loop_pc/hud_tr.png` のような `..` 参照を
/// `pipelines/field_loop_pc/hud_tr.png` へ畳み、同一物理 PNG の重複エントリを
/// 防ぐ (共有参照は 1 エントリに集約する監査の契約)。
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// TOML 1ファイルから template 参照 (実在する PNG パス) を抽出する。
///
/// - TaskDef schema (`templates/pipelines/*`, `scenes/{field_pc,menu_pc,title_pc}`):
///   `template = "..."` キーを TOML 親ディレクトリ基準で解決する (`../` 参照含む)。
/// - 旧 schema (`scenes/{field,menu}` の `method=` + `[roi]` テーブル): `template`
///   キーを持たないため、TemplateStore の sidecar 慣例 `<stem>.png` (同一ディレクトリ)
///   を実在する場合に限り採用する。
/// - どちらも解決できなければ `None` (`pipeline.toml` manifest・template 未宣言 等)。
///
/// `template` キーが存在するのに参照先 PNG が存在しない場合は panic する
/// (dangling 参照 = detect 時エラーになる実害のある資産欠陥。監査は fail-loud)。
fn template_ref(toml_path: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    let value: toml::Value =
        toml::from_str(&content).unwrap_or_else(|e| panic!("parse {}: {e}", toml_path.display()));

    if let Some(rel) = value.get("template").and_then(|v| v.as_str()) {
        let parent = toml_path.parent().unwrap_or_else(|| Path::new(""));
        let resolved = normalize_lexical(&parent.join(rel));
        assert!(
            resolved.is_file(),
            "template ref `{rel}` in {} does not resolve to a file ({})",
            toml_path.display(),
            resolved.display()
        );
        return Some(resolved);
    }
    // 旧 schema sidecar: <stem>.png が同ディレクトリにあれば参照とみなす。
    let stem = toml_path.file_stem()?.to_str()?;
    let sidecar = toml_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{stem}.png"));
    if sidecar.is_file() {
        Some(sidecar)
    } else {
        None
    }
}

/// 参照されている全テンプレート PNG の「templates/ ルート相対パス → stddev」一覧。
///
/// 複数 TOML から共有参照される PNG (例: scenes/title_pc/version_label.png は 3 TOML
/// から参照) は 1 エントリに重複排除する。
fn audited_templates() -> BTreeMap<String, f32> {
    // root を正規化してから使う (CARGO_MANIFEST_DIR/../../templates の `..` 成分が
    // 残ると、normalize_lexical 済みの参照解決パスと strip_prefix できず
    // 絶対パスがキーに漏れ、allowlist/重複排除が壊れる)。
    let root = normalize_lexical(&templates_root());
    let mut tomls = Vec::new();
    for seg in ["pipelines", "scenes"] {
        collect_toml_files(&root.join(seg), &mut tomls);
    }
    tomls.sort();

    let mut bank: BTreeMap<String, f32> = BTreeMap::new();
    for toml_path in &tomls {
        let Some(png) = template_ref(toml_path) else {
            continue;
        };
        let rel = png
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| png.to_string_lossy().replace('\\', "/"));
        let rel = normalize_lexical(Path::new(&rel))
            .to_string_lossy()
            .replace('\\', "/");
        if bank.contains_key(&rel) {
            continue; // 共有参照の重複排除 (測定済み)
        }
        let img =
            image::open(&png).unwrap_or_else(|e| panic!("open template {}: {e}", png.display()));
        let stddev = template_luma_stddev(&img);
        bank.insert(rel, stddev);
    }
    bank
}

/// stddev 一覧レポート (全件 + 統計 + allowlist 状態) を構築して返す。
/// stdout 出力と target/ 書き出しの両方で使う単一情報源。
fn build_report(bank: &BTreeMap<String, f32>) -> String {
    let mut report = String::new();
    report.push_str(&format!(
        "template bank audit (Issue #187) — referenced PNGs: {} files, \
         min stddev {:.2}, threshold {:.1}\n",
        bank.len(),
        bank.values().copied().fold(f32::INFINITY, f32::min),
        TEMPLATE_MIN_LUMA_STDDEV
    ));
    for (rel, stddev) in bank {
        let flag = if *stddev < TEMPLATE_MIN_LUMA_STDDEV {
            "UNSTRUCTURED"
        } else {
            "ok"
        };
        report.push_str(&format!("{flag:12} stddev={stddev:7.2}  {rel}\n"));
    }
    report.push_str(&format!(
        "allowlist (known unstructured, excluded from FAIL): {KNOWN_UNSTRUCTURED:?}\n"
    ));
    report
}

/// 監査レポートを `target/template-bank-audit-report.txt` へ書き出す (best-effort)。
/// target/ は .gitignore 済み (コミットされない)。書き出し失敗は eprintln で明示
/// するがテストは fail させない — evidence の本体は stdout 一覧とテスト green。
fn persist_report(report: &str) {
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("template-bank-audit-report.txt");
    match std::fs::write(&target, report) {
        Ok(()) => println!("audit report written to {}", target.display()),
        Err(e) => eprintln!("audit report write failed ({}): {e}", target.display()),
    }
}

/// 一括監査 (Issue #187 提案 1): 参照抽出 → stddev 測定 → 全件一覧と統計の出力。
///
/// カバレッジ下限 (fail-closed) を検証し、レポートを stdout + target/ へ出力する。
/// 無構造検出の合否は `referenced_bank_has_no_unstructured_outside_allowlist` が担う
/// (本テストは統計の取得と永続化が責務)。
#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn audit_report_covers_referenced_template_bank() {
    let bank = audited_templates();
    println!("{}", build_report(&bank));
    persist_report(&build_report(&bank));

    assert!(
        bank.len() >= MIN_AUDITED_TEMPLATES,
        "audit covered only {} referenced templates (floor {}): extraction or path \
         resolution may be broken — a vacuous green hides real unstructured assets \
         (fail-closed per pipeline-evidence-verification.md). Covered: {:?}",
        bank.len(),
        MIN_AUDITED_TEMPLATES,
        bank.keys().collect::<Vec<_>>()
    );
    // 最小 stddev は無構造テンプレ (allowlist) を除けば閾値超過であることを統計として
    // 明示 (合否の詳細は allowlist テストで担保)。
    let min_structured = bank
        .iter()
        .filter(|(rel, _)| !KNOWN_UNSTRUCTURED.contains(&rel.as_str()))
        .map(|(_, s)| *s)
        .fold(f32::INFINITY, f32::min);
    assert!(
        min_structured >= TEMPLATE_MIN_LUMA_STDDEV,
        "minimum stddev among non-allowlisted templates is {min_structured:.2} \
         (threshold {:.1})",
        TEMPLATE_MIN_LUMA_STDDEV
    );
}

/// 無構造テンプレート検出 (Issue #187 提案 2): stddev < 閾値のテンプレートは
/// [`KNOWN_UNSTRUCTURED`] (allowlist) に登録されたもののみ許容する。
///
/// - allowlist 外の無構造 = 新規欠陥資産 → FAIL (検出リストを表示)。
/// - allowlist 登録済みなのに構造を取り戻した (再撮影された) = stale エントリ →
///   FAIL (allowlist からの削除を促す。stddev も表示して再撮影の確認を容易にする)。
#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn referenced_bank_has_no_unstructured_outside_allowlist() {
    let bank = audited_templates();
    let unstructured: Vec<&String> = bank
        .iter()
        .filter(|(_, s)| **s < TEMPLATE_MIN_LUMA_STDDEV)
        .map(|(rel, _)| rel)
        .collect();

    // (1) allowlist 外の無構造は FAIL (検出リスト付き)。
    let unexpected: Vec<&String> = unstructured
        .iter()
        .copied()
        .filter(|rel| !KNOWN_UNSTRUCTURED.contains(&rel.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "unstructured templates (stddev < {:.1}) found outside the audit allowlist — \
         these can never match under TM_CCOEFF_NORMED (permanent NoMatch). Either \
         regenerate them from a real capture (see templates/pipelines/field_loop/\
         tap_bottom.toml for the procedure) or, if re-capture is impossible, register \
         them in KNOWN_UNSTRUCTURED with a rationale: {:?}",
        TEMPLATE_MIN_LUMA_STDDEV,
        unexpected
            .iter()
            .map(|rel| format!("{rel} (stddev {:.2})", bank[*rel]))
            .collect::<Vec<_>>()
    );

    // (2) allowlist 内の全エントリが実在し、かつ今も無構造であること (stale 検出)。
    //     再撮影で構造を取り戻したテンプレートが allowlist に残ると「恒久 NoMatch
    //     資産がある」という誤った申告になるため、削除を強制する。
    for entry in KNOWN_UNSTRUCTURED {
        match bank.get(*entry) {
            None => panic!(
                "allowlist entry `{entry}` is not referenced by any TOML in the bank — \
                 remove it from KNOWN_UNSTRUCTURED (stale entry)"
            ),
            Some(stddev) => assert!(
                *stddev < TEMPLATE_MIN_LUMA_STDDEV,
                "allowlist entry `{entry}` is now structured (stddev {stddev:.2} >= \
                 {:.1}) — it was regenerated or replaced. Remove it from \
                 KNOWN_UNSTRUCTURED so the audit protects it again",
                TEMPLATE_MIN_LUMA_STDDEV
            ),
        }
    }
}
