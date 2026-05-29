use std::fs;
use std::path::Path;

use tower_lsp::lsp_types::{DiagnosticSeverity, Url};

use crate::indexer::live_tree::parse_live;
use crate::indexer::Indexer;

fn uri_path(path: &str) -> Url {
    Url::parse(&format!("file://{path}")).unwrap()
}

fn write_file(dir: &Path, rel_path: &str, content: &str) -> Url {
    let full_path = dir.join(rel_path);
    fs::create_dir_all(full_path.parent().unwrap()).unwrap();
    fs::write(&full_path, content).unwrap();
    uri_path(full_path.to_str().unwrap())
}

fn setup_workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, Indexer) {
    let dir = tempfile::tempdir().unwrap();
    let idx = Indexer::new();
    idx.workspace_root.set(dir.path().to_path_buf());
    for (rel_path, content) in files {
        let u = write_file(dir.path(), rel_path, content);
        idx.store_live_tree(&u, content);
        idx.index_content(&u, content);
    }
    (dir, idx)
}

#[test]
fn test_no_active_platforms() {
    let (_dir, idx) = setup_workspace(&[(
        "src/commonMain/kotlin/Foo.kt",
        "package test\nexpect fun greet(name: String)",
    )]);
    let uri = uri_path(&format!(
        "{}/src/commonMain/kotlin/Foo.kt",
        _dir.path().to_str().unwrap()
    ));
    let doc =
        parse_live("package test\nexpect fun greet(name: String)", tree_sitter_kotlin::language())
            .unwrap();
    let diags = super::kmp_expect_diagnostics(&idx, &uri, &doc);
    assert!(diags.is_empty(), "expected no diagnostics, got: {diags:?}");
}

#[test]
fn test_missing_actual_diagnostic() {
    let (_dir, idx) = setup_workspace(&[(
        "src/commonMain/kotlin/Foo.kt",
        "package test\nexpect fun greet(name: String)",
    )]);
    fs::create_dir_all(_dir.path().join("src/androidMain/kotlin")).unwrap();

    let uri = uri_path(&format!(
        "{}/src/commonMain/kotlin/Foo.kt",
        _dir.path().to_str().unwrap()
    ));
    let doc =
        parse_live("package test\nexpect fun greet(name: String)", tree_sitter_kotlin::language())
            .unwrap();
    let diags = super::kmp_expect_diagnostics(&idx, &uri, &doc);
    assert_eq!(diags.len(), 1, "expected 1 diagnostic for missing android actual");
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    assert!(
        diags[0].message.contains("Android"),
        "message should mention Android: {}",
        diags[0].message
    );
}

#[test]
fn test_actual_present_no_diagnostic() {
    let (_dir, idx) = setup_workspace(&[
        (
            "src/commonMain/kotlin/Foo.kt",
            "package test\nexpect fun greet(name: String)",
        ),
        (
            "src/androidMain/kotlin/Foo.kt",
            "package test\nactual fun greet(name: String) {}",
        ),
    ]);
    fs::create_dir_all(_dir.path().join("src/androidMain/kotlin")).unwrap();

    let uri = uri_path(&format!(
        "{}/src/commonMain/kotlin/Foo.kt",
        _dir.path().to_str().unwrap()
    ));
    let doc =
        parse_live("package test\nexpect fun greet(name: String)", tree_sitter_kotlin::language())
            .unwrap();
    let diags = super::kmp_expect_diagnostics(&idx, &uri, &doc);
    assert!(diags.is_empty(), "expected no diagnostics when actual present");
}

#[test]
fn test_skips_non_common() {
    let (_dir, idx) = setup_workspace(&[(
        "src/androidMain/kotlin/Foo.kt",
        "package test\nexpect fun greet(name: String)",
    )]);
    let uri = uri_path(&format!(
        "{}/src/androidMain/kotlin/Foo.kt",
        _dir.path().to_str().unwrap()
    ));
    let doc =
        parse_live("package test\nexpect fun greet(name: String)", tree_sitter_kotlin::language())
            .unwrap();
    let diags = super::kmp_expect_diagnostics(&idx, &uri, &doc);
    assert!(diags.is_empty(), "should skip non-common files");
}
