use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::panic::catch_unwind;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Url};

use crate::indexer::live_tree::LiveDoc;
use crate::indexer::Indexer;

use super::code_lens::detect_expect_actual;

pub(crate) fn kmp_expect_diagnostics(
    indexer: &Indexer,
    uri: &Url,
    doc: &LiveDoc,
) -> Vec<Diagnostic> {
    match catch_unwind(std::panic::AssertUnwindSafe(|| {
        kmp_expect_diagnostics_inner(indexer, uri, doc)
    })) {
        Ok(diags) => diags,
        Err(_) => {
            log::error!("kmp_expect_diagnostics panicked for {}", uri.as_str());
            vec![]
        }
    }
}

fn kmp_expect_diagnostics_inner(indexer: &Indexer, uri: &Url, doc: &LiveDoc) -> Vec<Diagnostic> {
    let cur_set = super::code_lens::classify_source_set(uri.as_str());
    if cur_set.as_deref() != Some("common") {
        return vec![];
    }

    let symbols = detect_expect_actual(doc);
    let expects: Vec<&(String, tower_lsp::lsp_types::Range, bool)> = symbols
        .iter()
        .filter(|(_, _, is_expect)| *is_expect)
        .collect();
    if expects.is_empty() {
        return vec![];
    }

    let pkg = indexer
        .file_data_for(uri.as_str())
        .and_then(|d| d.package.clone());

    let Some(pkg) = pkg else {
        return vec![];
    };

    let active_sets = detect_active_sets(indexer);
    let mut diagnostics = Vec::new();

    for (name, range, _) in expects {
        let mut found_sets: HashSet<Cow<'static, str>> = HashSet::new();
        let mut cache: HashMap<String, Vec<(String, tower_lsp::lsp_types::Range, bool)>> =
            HashMap::new();

        for loc in indexer.definition_locations(name) {
            if loc.uri.as_str() == uri.as_str() {
                continue;
            }
            let Some(data) = indexer.file_data_for(loc.uri.as_str()) else {
                continue;
            };
            if data.package.as_deref() != Some(&pkg) {
                continue;
            }
            let Some(target_set) = super::code_lens::classify_source_set(loc.uri.as_str()) else {
                continue;
            };
            if target_set.as_ref() == "common" {
                continue;
            }
            if super::code_lens::symbol_has_modifier(indexer, &loc.uri, name, true, &mut cache) {
                found_sets.insert(target_set);
            }
        }

        for set in &active_sets {
            if set.as_ref() == "common" {
                continue;
            }
            if !found_sets.contains(set) {
                diagnostics.push(Diagnostic {
                    range: *range,
                    severity: Some(DiagnosticSeverity::WARNING),
                    source: Some("kotlin-lsp".into()),
                    message: format!(
                        "Missing 'actual' implementation for {}",
                        super::code_lens::target_label(set)
                    ),
                    data: Some(serde_json::json!({
                        "source_set": set,
                        "symbol": name,
                    })),
                    ..Default::default()
                });
            }
        }
    }

    diagnostics
}

pub(crate) fn detect_active_sets(indexer: &Indexer) -> HashSet<Cow<'static, str>> {
    let Some(root) = indexer.workspace_root.get() else {
        return HashSet::new();
    };
    let src_dir = root.join("src");
    let Ok(entries) = std::fs::read_dir(&src_dir) else {
        return HashSet::new();
    };

    let mut sets = HashSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with("Main") && !name.ends_with("Test") {
            continue;
        }
        let has_kotlin = path.join("kotlin").is_dir();
        let has_java = path.join("java").is_dir();
        if !has_kotlin && !has_java {
            continue;
        }
        sets.insert(super::code_lens::classify_source_set_name(name));
    }
    sets
}

#[cfg(test)]
#[path = "kmp_expect_diagnostics_tests.rs"]
mod tests;
