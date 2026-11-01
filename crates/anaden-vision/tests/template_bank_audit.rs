use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anaden_vision::{TEMPLATE_MIN_LUMA_STDDEV, needle_fits_roi, template_luma_stddev};

fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates")
}

const KNOWN_UNSTRUCTURED: &[&str] = &[];

const MIN_AUDITED_TEMPLATES: usize = 25;

const MIN_AUDITED_REFERENCES: usize = 32;

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

fn template_ref(toml_path: &Path) -> Option<(PathBuf, Option<[u32; 4]>)> {
    let content = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    let value: toml::Value =
        toml::from_str(&content).unwrap_or_else(|e| panic!("parse {}: {e}", toml_path.display()));
    let roi = detection_roi(&value, toml_path);

    if let Some(rel) = value.get("template").and_then(|v| v.as_str()) {
        let parent = toml_path.parent().unwrap_or_else(|| Path::new(""));
        let resolved = normalize_lexical(&parent.join(rel));
        assert!(
            resolved.is_file(),
            "template ref `{rel}` in {} does not resolve to a file ({})",
            toml_path.display(),
            resolved.display()
        );
        return Some((resolved, roi));
    }
    let stem = toml_path.file_stem()?.to_str()?;
    let sidecar = toml_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{stem}.png"));
    if sidecar.is_file() {
        Some((sidecar, roi))
    } else {
        None
    }
}

fn detection_roi(value: &toml::Value, toml_path: &Path) -> Option<[u32; 4]> {
    let raw = value.get("roi")?;
    if let Some(arr) = raw.as_array() {
        let nums: Option<Vec<u32>> = arr
            .iter()
            .map(|v| v.as_integer().and_then(|i| u32::try_from(i).ok()))
            .collect();
        let nums = nums.unwrap_or_else(|| {
            panic!(
                "roi in {} must be an array of 4 non-negative integers, got {arr:?}",
                toml_path.display()
            )
        });
        match <[u32; 4]>::try_from(nums) {
            Ok(slice) => return Some(slice),
            Err(nums) => panic!(
                "roi in {} must have exactly 4 elements [x, y, w, h], got {nums:?}",
                toml_path.display()
            ),
        }
    }
    if let Some(tbl) = raw.as_table() {
        let get = |key: &str| -> Option<u32> {
            tbl.get(key)?
                .as_integer()
                .and_then(|i| u32::try_from(i).ok())
        };
        let (Some(x), Some(y), Some(w), Some(h)) =
            (get("x"), get("y"), get("width"), get("height"))
        else {
            panic!(
                "roi table in {} must have non-negative integer x/y/width/height, got {tbl:?}",
                toml_path.display()
            )
        };
        return Some([x, y, w, h]);
    }
    panic!(
        "roi in {} must be a [x, y, w, h] array or a legacy x/y/width/height table, got {raw:?}",
        toml_path.display()
    );
}

struct TemplateRef {
    toml_rel: String,
    png_rel: String,
    roi: Option<[u32; 4]>,
}

fn audited_references() -> Vec<TemplateRef> {
    let root = normalize_lexical(&templates_root());
    let mut tomls = Vec::new();
    for seg in ["pipelines", "scenes"] {
        collect_toml_files(&root.join(seg), &mut tomls);
    }
    tomls.sort();

    let rel_to_fwd = |p: &Path| normalize_lexical(p).to_string_lossy().replace('\\', "/");
    let mut refs = Vec::new();
    for toml_path in &tomls {
        let Some((png, roi)) = template_ref(toml_path) else {
            continue;
        };
        let toml_rel = toml_path
            .strip_prefix(&root)
            .map(&rel_to_fwd)
            .unwrap_or_else(|_| rel_to_fwd(toml_path));
        let png_rel = png
            .strip_prefix(&root)
            .map(&rel_to_fwd)
            .unwrap_or_else(|_| rel_to_fwd(&png));
        refs.push(TemplateRef {
            toml_rel,
            png_rel,
            roi,
        });
    }
    refs
}

fn audited_templates() -> BTreeMap<String, f32> {
    let root = normalize_lexical(&templates_root());
    let mut bank: BTreeMap<String, f32> = BTreeMap::new();
    for r in audited_references() {
        if bank.contains_key(&r.png_rel) {
            continue;
        }
        let png = root.join(&r.png_rel);
        let img =
            image::open(&png).unwrap_or_else(|e| panic!("open template {}: {e}", png.display()));
        let stddev = template_luma_stddev(&img);
        bank.insert(r.png_rel, stddev);
    }
    bank
}

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

    let unexpected: Vec<&String> = unstructured
        .iter()
        .copied()
        .filter(|rel| !KNOWN_UNSTRUCTURED.contains(&rel.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "unstructured templates (stddev < {:.1}) found outside the audit allowlist — \
         these can never match under TM_CCOEFF_NORMED (permanent NoMatch). Either \
         regenerate them from a real capture or, if re-capture is impossible, register \
         them in KNOWN_UNSTRUCTURED with a rationale: {:?}",
        TEMPLATE_MIN_LUMA_STDDEV,
        unexpected
            .iter()
            .map(|rel| format!("{rel} (stddev {:.2})", bank[*rel]))
            .collect::<Vec<_>>()
    );

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

#[test]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
fn referenced_needles_fit_their_detection_rois() {
    let refs = audited_references();
    assert!(
        refs.len() >= MIN_AUDITED_REFERENCES,
        "fit audit covered only {} references (floor {}): extraction or path \
         resolution may be broken — a vacuous green hides real needle/roi \
         violations (fail-closed per pipeline-evidence-verification.md). \
         Covered: {:?}",
        refs.len(),
        MIN_AUDITED_REFERENCES,
        refs.iter()
            .map(|r| format!("{} -> {}", r.toml_rel, r.png_rel))
            .collect::<Vec<_>>()
    );

    let root = normalize_lexical(&templates_root());
    let mut violations = Vec::new();
    for r in &refs {
        let png = root.join(&r.png_rel);
        let (nw, nh) = image::image_dimensions(&png)
            .unwrap_or_else(|e| panic!("read dimensions of {}: {e}", png.display()));
        let [_, _, rw, rh] = match r.roi {
            None => {
                println!("fit ok (fullscreen): {} -> {}", r.toml_rel, r.png_rel);
                continue;
            }
            Some(roi) => roi,
        };
        if needle_fits_roi((nw, nh), r.roi) {
            println!(
                "fit ok: {} -> {} needle {nw}x{nh} in roi {rw}x{rh}",
                r.toml_rel, r.png_rel
            );
        } else {
            violations.push(format!(
                "{} -> {}: needle {}x{} exceeds roi {:?} ({}x{})",
                r.toml_rel, r.png_rel, nw, nh, r.roi, rw, rh
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "needles that do not fit their detection roi (permanent NoMatch — the needle \
         cannot be placed inside the cropped haystack, see Issue #182/#192): {:?}. \
         Update the consumer roi as a pair whenever a needle is regenerated/re-cropped",
        violations
    );
}
