use std::borrow::Cow;
use std::collections::HashMap;

use tower_lsp::lsp_types::Url;

use crate::indexer::live_tree::LiveDoc;
use crate::indexer::Indexer;
use crate::queries::*;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn parse_doc(src: &str) -> LiveDoc {
    let lang = tree_sitter_kotlin::language();
    crate::indexer::live_tree::parse_live(src, lang).expect("parse failed")
}

fn with_first_decl<R>(src: &str, f: impl FnOnce(&tree_sitter::Node, &[u8]) -> R) -> R {
    let doc = parse_doc(src);
    let root = doc.tree.root_node();
    let bytes = &doc.bytes;
    for i in 0..root.child_count() {
        let child = root.child(i).unwrap();
        if matches!(
            child.kind(),
            KIND_FUN_DECL | KIND_PROP_DECL | KIND_CLASS_DECL | KIND_OBJECT_DECL | KIND_TYPE_ALIAS
        ) {
            return f(&child, bytes);
        }
    }
    panic!("no declaration node found in: {src}")
}

fn index_multi(files: &[(&str, &str)]) -> (Vec<Url>, Indexer) {
    let idx = Indexer::new();
    let uris: Vec<Url> = files.iter().map(|(path, _)| uri(path)).collect();
    for ((_path, src), u) in files.iter().zip(uris.iter()) {
        idx.index_content(u, src);
    }
    (uris, idx)
}

#[track_caller]
fn assert_lens_for(
    lenses: &[tower_lsp::lsp_types::CodeLens],
    label_substr: &str,
    target_uri_substr: &str,
) {
    let found = lenses.iter().any(|l| {
        l.command
            .as_ref()
            .map(|c| c.title.contains(label_substr))
            .unwrap_or(false)
            && l.command.as_ref().map_or(false, |c| {
                c.arguments
                    .as_ref()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .map(|s| s.contains(target_uri_substr))
                    .unwrap_or(false)
            })
    });
    assert!(
        found,
        "no lens found with label containing '{label_substr}' and target '{target_uri_substr}'"
    );
}

// ─── classify_source_set ──────────────────────────────────────────────────────

#[test]
fn classify_source_set_common_main() {
    let result =
        super::classify_source_set("/home/project/src/commonMain/kotlin/com/example/Foo.kt");
    assert_eq!(result, Some(Cow::Borrowed("common")));
}

#[test]
fn classify_source_set_android_main() {
    let result =
        super::classify_source_set("/home/project/src/androidMain/kotlin/com/example/Foo.kt");
    assert_eq!(result, Some(Cow::Borrowed("android")));
}

#[test]
fn classify_source_set_no_match() {
    let result = super::classify_source_set("/home/project/src/main/kotlin/com/example/Foo.kt");
    assert_eq!(result, None);
}

// ─── target_label ─────────────────────────────────────────────────────────────

#[test]
fn target_label_android() {
    assert_eq!(&*super::target_label("android"), "\u{25A6} Android");
}

#[test]
fn target_label_jvm() {
    assert_eq!(&*super::target_label("jvm"), "\u{2B22} JVM");
}

#[test]
fn target_label_common() {
    assert_eq!(&*super::target_label("common"), "\u{25C9} common");
}

// ─── check_modifier ───────────────────────────────────────────────────────────

#[test]
fn check_modifier_expect_fun() {
    with_first_decl("expect fun foo()", |node, bytes| {
        assert_eq!(super::check_modifier(node, bytes), Some(true));
    });
}

#[test]
fn check_modifier_actual_fun() {
    with_first_decl("actual fun foo() {}", |node, bytes| {
        assert_eq!(super::check_modifier(node, bytes), Some(false));
    });
}

#[test]
fn check_modifier_plain_fun() {
    with_first_decl("fun foo() {}", |node, bytes| {
        assert_eq!(super::check_modifier(node, bytes), None);
    });
}

#[test]
fn check_modifier_expect_class() {
    with_first_decl("expect class PlatformContext", |node, bytes| {
        assert_eq!(super::check_modifier(node, bytes), Some(true));
    });
}

#[test]
fn check_modifier_expect_val() {
    with_first_decl("expect val x: Int", |node, bytes| {
        assert_eq!(super::check_modifier(node, bytes), Some(true));
    });
}

// ─── detect_expect_actual ─────────────────────────────────────────────────────

#[test]
fn detect_expect_actual_mixed() {
    let src = "expect fun greet(name: String)\nactual fun greet(name: String) {}\nfun helper() {}";
    let doc = parse_doc(src);
    let results = super::detect_expect_actual(&doc);
    assert_eq!(
        results.len(),
        2,
        "expect+actual+plain → 2 results, got {}",
        results.len()
    );
    let names: Vec<&str> = results.iter().map(|(n, _, _)| n.as_str()).collect();
    assert!(names.contains(&"greet"), "results should include 'greet'");
}

// ─── resolve_counterparts ─────────────────────────────────────────────────────

#[test]
fn resolve_counterparts_expect_in_common_to_android() {
    let (uris, idx) = index_multi(&[
        (
            "/src/commonMain/kotlin/Foo.kt",
            "package test\nexpect fun greet(name: String)",
        ),
        (
            "/src/androidMain/kotlin/Foo.kt",
            "package test\nactual fun greet(name: String) {}",
        ),
    ]);
    let mut cache = HashMap::new();
    let results = super::resolve_counterparts(
        &idx,
        "greet",
        Some("test"),
        uris[0].as_str(),
        true,
        &mut cache,
    );
    assert_eq!(
        results.len(),
        1,
        "common expect → should find 1 counterpart"
    );
    assert!(
        results[0].0.contains("Android"),
        "label should mention Android, got: {}",
        results[0].0
    );
}

#[test]
fn resolve_counterparts_actual_in_android_to_common() {
    let (uris, idx) = index_multi(&[
        (
            "/src/commonMain/kotlin/Foo.kt",
            "package test\nexpect fun greet(name: String)",
        ),
        (
            "/src/androidMain/kotlin/Foo.kt",
            "package test\nactual fun greet(name: String) {}",
        ),
    ]);
    let mut cache = HashMap::new();
    let results = super::resolve_counterparts(
        &idx,
        "greet",
        Some("test"),
        uris[1].as_str(),
        false,
        &mut cache,
    );
    assert_eq!(
        results.len(),
        1,
        "android actual → should find 1 counterpart"
    );
    assert!(
        results[0].0.contains("common"),
        "label should mention 'common', got: {}",
        results[0].0
    );
}

#[test]
fn resolve_counterparts_skips_wrong_modifier() {
    let (uris, idx) = index_multi(&[
        (
            "/src/commonMain/kotlin/Foo.kt",
            "package test\nexpect fun greet(name: String)",
        ),
        (
            "/src/androidMain/kotlin/Foo.kt",
            "package test\nfun greet(name: String) {}",
        ),
    ]);
    let mut cache = HashMap::new();
    let results = super::resolve_counterparts(
        &idx,
        "greet",
        Some("test"),
        uris[0].as_str(),
        true,
        &mut cache,
    );
    assert!(
        results.is_empty(),
        "expect → plain (without actual) should be skipped, got {} results",
        results.len()
    );
}

// ─── compute_code_lens integration ────────────────────────────────────────────

#[test]
fn compute_code_lens_kmp() {
    let common_src = "package test\nexpect fun greet(name: String)";
    let android_src = "package test\nactual fun greet(name: String) {}";
    let ios_src = "package test\nactual fun greet(name: String) {}";

    let (uris, idx) = index_multi(&[
        ("/src/commonMain/kotlin/Foo.kt", common_src),
        ("/src/androidMain/kotlin/Foo.kt", android_src),
        ("/src/iosMain/kotlin/Foo.kt", ios_src),
    ]);

    idx.store_live_tree(&uris[0], common_src);
    idx.store_live_tree(&uris[1], android_src);
    idx.store_live_tree(&uris[2], ios_src);

    let lenses = super::compute_code_lens(&idx, &uris[0]);
    assert_eq!(
        lenses.len(),
        2,
        "expected 2 lenses (android + ios), got {}",
        lenses.len()
    );

    assert_lens_for(&lenses, "Android", "androidMain");
    assert_lens_for(&lenses, "iOS", "iosMain");
}
