use std::borrow::Cow;
use std::collections::HashMap;

use tower_lsp::lsp_types::{CodeLens, Command, Location, Position, Range, Url};
use tree_sitter::Node;

use crate::indexer::live_tree::LiveDoc;
use crate::indexer::{Indexer, NodeExt};
use crate::queries::*;

pub(crate) fn compute_code_lens(indexer: &Indexer, uri: &Url) -> Vec<CodeLens> {
    let Some(doc) = indexer.live_doc_or_parse(uri) else {
        return vec![];
    };

    let symbols = detect_expect_actual(&doc);
    if symbols.is_empty() {
        return vec![];
    }

    let cur_pkg = indexer
        .file_data_for(uri.as_str())
        .and_then(|d| d.package.clone());

    let mut lenses: Vec<CodeLens> = Vec::new();
    let mut symbol_cache: HashMap<String, Vec<(String, Range, bool)>> = HashMap::new();

    for (name, sel_range, is_expect) in symbols {
        let counterparts = resolve_counterparts(
            indexer,
            &name,
            cur_pkg.as_deref(),
            uri.as_str(),
            is_expect,
            &mut symbol_cache,
        );

        for (label, target_loc) in &counterparts {
            lenses.push(make_navigate_lens(sel_range, label, target_loc));
        }
    }

    lenses
}

pub(crate) fn detect_expect_actual(doc: &LiveDoc) -> Vec<(String, Range, bool)> {
    let root = doc.tree.root_node();
    let bytes = &doc.bytes;
    let mut results = Vec::new();

    let mut stack: Vec<Node> = vec![root];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        let is_decl = matches!(
            kind,
            KIND_FUN_DECL | KIND_PROP_DECL | KIND_CLASS_DECL | KIND_OBJECT_DECL | KIND_TYPE_ALIAS
        );

        if is_decl {
            if let Some(is_expect) = check_modifier(&node, bytes) {
                if let Some((name, sel_range)) = extract_decl_name(&node, bytes) {
                    results.push((name, sel_range, is_expect));
                }
            }
        }

        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            stack.push(child);
        }
    }

    results
}

pub(crate) fn check_modifier(node: &Node, bytes: &[u8]) -> Option<bool> {
    for i in 0..node.child_count() {
        let child = node.child(i)?;
        if child.kind() != KIND_MODIFIERS {
            continue;
        }
        for j in 0..child.child_count() {
            let mc = child.child(j)?;
            if let Ok(text) = mc.utf8_text(bytes) {
                match text.trim() {
                    "expect" => return Some(true),
                    "actual" => return Some(false),
                    _ => {}
                }
            }
        }
    }
    None
}

pub(crate) fn extract_decl_name(node: &Node, bytes: &[u8]) -> Option<(String, Range)> {
    if let Some(id) = node.first_child_of_kind(KIND_SIMPLE_IDENT) {
        if let Ok(text) = id.utf8_text(bytes) {
            return Some((text.to_owned(), ts_range_to_lsp(id.range())));
        }
    }
    if let Some(id) = node.first_child_of_kind(KIND_TYPE_IDENT) {
        if let Ok(text) = id.utf8_text(bytes) {
            return Some((text.to_owned(), ts_range_to_lsp(id.range())));
        }
    }
    None
}

fn resolve_counterparts(
    indexer: &Indexer,
    name: &str,
    pkg: Option<&str>,
    cur_uri: &str,
    is_expect: bool,
    symbol_cache: &mut HashMap<String, Vec<(String, Range, bool)>>,
) -> Vec<(Cow<'static, str>, Location)> {
    let Some(pkg) = pkg else {
        return vec![];
    };

    let cur_set = classify_source_set(cur_uri);
    let mut results: Vec<(Cow<'static, str>, Location)> = Vec::new();

    for loc in indexer.definition_locations(name) {
        let loc_uri = loc.uri.as_str();
        if loc_uri == cur_uri {
            continue;
        }

        let Some(data) = indexer.file_data_for(loc_uri) else {
            continue;
        };
        if data.package.as_deref() != Some(pkg) {
            continue;
        }

        let target_set = match classify_source_set(loc_uri) {
            Some(s) => s,
            None => continue,
        };

        if cur_set.as_deref() == Some(target_set.as_ref()) {
            continue;
        }

        let must_be_actual = if is_expect {
            true
        } else {
            target_set.as_ref() != "common"
        };

        if !symbol_has_modifier(indexer, &loc.uri, name, must_be_actual, symbol_cache) {
            continue;
        }

        let label = target_label(target_set.as_ref());
        results.push((label, loc));
    }

    results
}

pub(crate) fn symbol_has_modifier(
    indexer: &Indexer,
    uri: &Url,
    name: &str,
    must_be_actual: bool,
    cache: &mut HashMap<String, Vec<(String, Range, bool)>>,
) -> bool {
    let symbols = cache.entry(uri.to_string()).or_insert_with(|| {
        indexer
            .live_doc_or_parse(uri)
            .map(|doc| detect_expect_actual(&doc))
            .unwrap_or_default()
    });
    for (sym_name, _, is_expect) in symbols.iter() {
        if sym_name == name {
            return *is_expect != must_be_actual;
        }
    }
    false
}

pub(crate) fn classify_source_set(uri: &str) -> Option<Cow<'static, str>> {
    for segment in uri.split('/') {
        if segment.ends_with("Main") || segment.ends_with("Test") {
            return Some(classify_source_set_name(segment));
        }
    }
    None
}

pub(crate) fn classify_source_set_name(name: &str) -> Cow<'static, str> {
    if name.starts_with("common") {
        Cow::Borrowed("common")
    } else if name.starts_with("android") {
        Cow::Borrowed("android")
    } else if name.starts_with("apple") {
        Cow::Borrowed("apple")
    } else if name.starts_with("ios") {
        Cow::Borrowed("ios")
    } else if name.starts_with("jvm") {
        Cow::Borrowed("jvm")
    } else if name.starts_with("js") {
        Cow::Borrowed("js")
    } else if name.starts_with("native") {
        Cow::Borrowed("native")
    } else if name.starts_with("macos") {
        Cow::Borrowed("macos")
    } else if name.starts_with("linux") {
        Cow::Borrowed("linux")
    } else if name.starts_with("wasm") {
        Cow::Borrowed("wasm")
    } else {
        Cow::Owned(
            name.strip_suffix("Main")
                .or_else(|| name.strip_suffix("Test"))
                .unwrap_or(name)
                .to_string(),
        )
    }
}

pub(crate) fn target_label(target_set: &str) -> Cow<'static, str> {
    match target_set {
        "common" => Cow::Borrowed("\u{25C9} common"),
        "android" => Cow::Borrowed("\u{25A6} Android"),
        "apple" => Cow::Borrowed("\u{25C8} Apple"),
        "ios" => Cow::Borrowed("\u{25C7} iOS"),
        "jvm" => Cow::Borrowed("\u{2B22} JVM"),
        "js" => Cow::Borrowed("\u{2726} JS"),
        "native" => Cow::Borrowed("\u{25A0} Native"),
        "macos" => Cow::Borrowed("\u{25C7} macOS"),
        "linux" => Cow::Borrowed("\u{25B2} Linux"),
        "wasm" => Cow::Borrowed("\u{25A2} Wasm"),
        _ => Cow::Owned(format!("\u{25C8} {target_set}")),
    }
}

fn make_navigate_lens(range: Range, title: &str, target: &Location) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command {
            title: title.to_owned(),
            command: "editor.action.goToLocations".to_owned(),
            arguments: Some(vec![
                serde_json::json!(target.uri.as_str()),
                serde_json::json!({
                    "line": target.range.start.line,
                    "character": target.range.start.character,
                }),
                serde_json::json!([target]),
                serde_json::json!("goto"),
            ]),
        }),
        data: None,
    }
}

fn ts_range_to_lsp(r: tree_sitter::Range) -> Range {
    Range {
        start: Position {
            line: r.start_point.row as u32,
            character: r.start_point.column as u32,
        },
        end: Position {
            line: r.end_point.row as u32,
            character: r.end_point.column as u32,
        },
    }
}

#[cfg(test)]
#[path = "code_lens_tests.rs"]
mod tests;
