// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

use rusqlite::Connection;
use sct_rs::commands::{markdown, ndjson, sqlite};
use sct_rs::schema::ConceptRecord;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("fixture.ndjson");
    let db = dir.path().join("fixture.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")],
        locale: "en-GB".into(),
        output: Some(input.clone()),
        include_inactive: false,
        refsets: ndjson::RefsetMode::Simple,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: input.clone(),
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    (dir, input, db)
}

fn sct(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sct"));
    cmd.current_dir(dir)
        .env_remove("SCT_CONFIG")
        .env("SCT_CONFIG_HOME", dir)
        .env("SCT_DATA_HOME", dir);
    cmd
}

fn myocardial_infarction(input: &Path) -> ConceptRecord {
    let record = std::fs::read_to_string(input)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<ConceptRecord>(line).ok())
        .find(|record| record.id == "22298006")
        .unwrap();
    assert_eq!(record.preferred_term, "Myocardial infarction");
    record
}

#[test]
fn output_boundaries_diagram_rejects_nested_graph_syntax_before_writing() {
    let (dir, _, db) = fixture();
    let conn = Connection::open(&db).unwrap();
    let output = dir.path().join("diagram.txt");
    let mut formats = vec!["tree", "dot", "mermaid"];
    if cfg!(feature = "diagram-svg") {
        formats.push("svg");
    }
    for format in &formats {
        let result = sct(dir.path())
            .args(["diagram", "22298006", "--format", format, "--db"])
            .arg(&db)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.contains("22298006"));
        assert!(text.contains("74281007")); // Myocardium structure, the finding site.
    }
    for field in ["destination_id", "type_id"] {
        for id in [
            "999\"; \"22298006\" -> \"46635009\"; \"999",
            "999\nc1 --> c2",
            "",
            "+22298006",
        ] {
            // The column choice is test-owned, never input data.
            conn.execute(
                &format!(
                    "UPDATE concept_relationships SET {field} = ?1 WHERE source_id = '22298006'"
                ),
                [id],
            )
            .unwrap();
            for view in ["definition", "neighbourhood"] {
                for format in &formats {
                    std::fs::write(&output, "unchanged").unwrap();
                    let result = sct(dir.path())
                        .args([
                            "diagram", "22298006", "--view", view, "--format", format, "--db",
                        ])
                        .arg(&db)
                        .arg("--output")
                        .arg(&output)
                        .output()
                        .unwrap();
                    assert!(
                        !result.status.success(),
                        "accepted {field}={id:?}, {view}/{format}"
                    );
                    assert!(result.stdout.is_empty());
                    assert!(
                        String::from_utf8_lossy(&result.stderr).contains("invalid diagram SCTID")
                    );
                    assert_eq!(std::fs::read_to_string(&output).unwrap(), "unchanged");
                }
            }
        }
        conn.execute("UPDATE concept_relationships SET destination_id = '74281007', type_id = '363698007' WHERE source_id = '22298006'", []).unwrap();
    }
}

#[test]
fn output_boundaries_diagram_rejects_isa_syntax_before_numeric_coercion() {
    let (dir, _, db) = fixture();
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE concept_isa SET parent_id = '404684003; forged' WHERE child_id = '22298006'",
        [],
    )
    .unwrap();
    for view in ["definition", "ancestors", "neighbourhood"] {
        let result = sct(dir.path())
            .args([
                "diagram", "22298006", "--view", view, "--format", "dot", "--db",
            ])
            .arg(&db)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("invalid diagram SCTID"));
    }
    conn.execute(
        "INSERT INTO concept_isa (child_id, parent_id) VALUES ('999; forged', '22298006')",
        [],
    )
    .unwrap();
    let result = sct(dir.path())
        .args(["diagram", "22298006", "--view", "descendants", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("invalid diagram SCTID"));
}

#[test]
fn output_boundaries_diagram_labels_remain_label_text() {
    let (dir, _, db) = fixture();
    let conn = Connection::open(&db).unwrap();
    let label = "Term\r\n\"]\nforged -->|edge| c999\n<img src=x> & #34;";
    conn.execute(
        "UPDATE concepts SET preferred_term = ?1 WHERE id IN ('22298006', '363698007')",
        [label],
    )
    .unwrap();
    let result = sct(dir.path())
        .args(["diagram", "22298006", "--format", "mermaid", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(!text.contains('\r'));
    assert!(!text.contains("\nforged"));
    assert!(!text.contains("<img"));
    assert!(!text.contains("|edge|"));
    assert!(text.contains("#34;#93;"));
    assert!(text.contains("#35;34#59;"));
    let result = sct(dir.path())
        .args(["diagram", "22298006", "--format", "dot", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(!text.contains('\r'));
    assert!(!text.contains("\nforged"));
    assert!(text.contains("Term\\r\\n\\\""));

    #[cfg(feature = "diagram-svg")]
    {
        let result = sct(dir.path())
            .args(["diagram", "22298006", "--format", "svg", "--db"])
            .arg(&db)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.contains("<svg "));
        assert!(text.trim_end().ends_with("</svg>"));
        assert!(text.contains("&lt;img src=x&gt;"));
        assert!(!text.contains("<img"));
        assert!(text.contains("22298006"));
        assert!(text.contains("74281007"));
    }
}

#[test]
fn output_boundaries_markdown_rejects_unsafe_ids_and_preserves_files() {
    let (dir, input, _) = fixture();
    let original = serde_json::to_string(&myocardial_infarction(&input)).unwrap();
    let sentinel = dir.path().join("victim.md");
    std::fs::write(&sentinel, "unchanged").unwrap();
    let absolute = dir.path().join("victim").to_string_lossy().into_owned();
    for id in [
        "../../victim",
        "..\\..\\victim",
        absolute.as_str(),
        "",
        "22298006`\n## forged",
    ] {
        for field in ["id", "parent", "attribute"] {
            let mut record: ConceptRecord = serde_json::from_str(&original).unwrap();
            match field {
                "id" => record.id = id.into(),
                "parent" => record.parents[0].id = id.into(),
                _ => record.attributes.values_mut().next().unwrap()[0].id = id.into(),
            }
            std::fs::write(&input, serde_json::to_string(&record).unwrap()).unwrap();
            for mode in [
                markdown::OutputMode::Concept,
                markdown::OutputMode::Hierarchy,
            ] {
                let output = dir.path().join("export");
                let error = markdown::run(markdown::Args {
                    input: input.clone(),
                    output: Some(output.clone()),
                    mode,
                })
                .unwrap_err();
                assert!(error.to_string().contains("invalid Markdown SCTID"));
                assert_eq!(std::fs::read_dir(output).unwrap().count(), 0);
                assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "unchanged");
            }
        }
    }
}

#[test]
fn output_boundaries_markdown_keeps_display_values_in_their_sections() {
    let (dir, input, _) = fixture();
    let mut record = myocardial_infarction(&input);
    let value = "Clinical text\r\n\n## Forged\n<script>alert(1)</script> [link](https://example.invalid) &amp; `code`";
    record.preferred_term = value.into();
    record.fsn = value.into();
    record.synonyms = vec![value.into()];
    record.hierarchy = value.into();
    record.hierarchy_path = vec![value.into()];
    record.parents[0].fsn = value.into();
    record.attributes.clear();
    record.attributes.insert(
        value.into(),
        vec![sct_rs::schema::ConceptRef {
            id: "74281007".into(),
            fsn: value.into(),
        }],
    );
    std::fs::write(&input, serde_json::to_string(&record).unwrap()).unwrap();
    for mode in [
        markdown::OutputMode::Concept,
        markdown::OutputMode::Hierarchy,
    ] {
        let output = dir.path().join(format!("{mode:?}"));
        markdown::run(markdown::Args {
            input: input.clone(),
            output: Some(output.clone()),
            mode: mode.clone(),
        })
        .unwrap();
        let slug = markdown::slugify(&record.hierarchy);
        let file = match mode {
            markdown::OutputMode::Concept => output.join(slug).join("22298006.md"),
            markdown::OutputMode::Hierarchy => output.join(format!("{slug}.md")),
        };
        let text = std::fs::read_to_string(file).unwrap();
        assert!(!text.contains('\r'));
        assert!(!text.contains("<script>"));
        assert!(!text.contains("[link]("));
        assert!(!text.contains("\n## Forged"));
        assert!(text.contains("Clinical text \\#\\# Forged &lt;script&gt;"));
        assert!(text.contains("&amp;amp\\;"));
        let headings: Vec<_> = text
            .lines()
            .filter(|line| line.starts_with("## "))
            .collect();
        match mode {
            markdown::OutputMode::Concept => assert_eq!(
                headings,
                [
                    "## Synonyms",
                    "## Relationships",
                    "## Hierarchy",
                    "## Parents"
                ]
            ),
            markdown::OutputMode::Hierarchy => {
                assert_eq!(headings.len(), 1);
                assert!(headings[0].ends_with("`22298006`"));
            }
        }
    }
}

#[test]
fn output_boundaries_completion_hints_quote_shell_literals() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir
        .path()
        .join("space ' $(throw) `tick`; marker \u{2018}\u{2019}\u{201a}\u{201b}");
    for shell in ["zsh", "powershell"] {
        let result = sct(dir.path())
            .args(["completions", "install", "--shell", shell, "--dir"])
            .arg(&destination)
            .output()
            .unwrap();
        assert!(result.status.success());
        let text = String::from_utf8(result.stdout).unwrap();
        if shell == "zsh" {
            let quoted = destination.to_string_lossy().replace('\'', "'\\''");
            assert!(text.contains(&format!("  fpath=('{quoted}' $fpath)")));
        } else {
            let quoted = destination
                .join("sct.ps1")
                .to_string_lossy()
                .replace('\'', "''")
                .replace('\u{2018}', "\u{2018}\u{2018}")
                .replace('\u{2019}', "\u{2019}\u{2019}")
                .replace('\u{201a}', "\u{201a}\u{201a}")
                .replace('\u{201b}', "\u{201b}\u{201b}");
            assert!(text.contains(&format!("  . '{quoted}'")));
        }
    }
}

#[test]
fn output_boundaries_gui_never_interpolates_data_into_inline_handlers() {
    let html = include_str!("../assets/index.html");
    for line in html.lines().filter(|line| line.contains("onclick=")) {
        assert!(!line.contains("${"), "dynamic inline handler: {line}");
        assert!(!line.contains("loadConcept("));
        assert!(!line.contains("loadChildren("));
    }
    assert!(html.contains("data-concept-id=\"${esc(p.id)}\""));
    assert!(html.contains("data-concept-id=\"${esc(v.id)}\""));
    assert!(html.contains("data-concept-id=\"${esc(ch.id)}\""));
    assert!(html.contains("data-children-id=\"${esc(c.id)}\""));
}
