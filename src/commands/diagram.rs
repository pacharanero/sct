// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct diagram` - render a concept's logical definition, ancestry, or
//! descendants as a `tree`-style terminal view, Graphviz DOT, Mermaid, or
//! built-in SVG (with the `diagram-svg` feature).
//! See `spec/commands/diagram.md`.
//!
//! All formats are plain text on stdout (pipe `--format dot` into
//! `dot -Tpng` for an image); the node/edge summary goes to stderr.

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use rusqlite::Connection;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;

use crate::ecl::eval;

#[derive(Parser, Debug)]
pub struct Args {
    /// Focus concept SCTID. Pass `-` to read a single id from stdin.
    concept: String,

    /// What to draw around the focus concept.
    #[arg(long, value_enum, default_value_t = View::Definition)]
    view: View,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Tree)]
    format: Format,

    /// Max hops for ancestors/descendants (default: ancestors = to root,
    /// descendants = 1). Ignored for the definition and neighbourhood views.
    #[arg(long)]
    depth: Option<usize>,

    /// Node caption style.
    #[arg(long, value_enum, default_value_t = LabelStyle::Pt)]
    labels: LabelStyle,

    /// Restrict tree output to 7-bit ASCII instead of Unicode box-drawing.
    #[arg(long)]
    ascii: bool,

    /// Write to a file instead of stdout (format is set by --format, not the
    /// extension).
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    output: Option<PathBuf>,

    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for discovery.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    db: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum View {
    /// Focus + its IS-A parents + defining attribute relationships.
    Definition,
    /// Transitive supertypes up the IS-A graph.
    Ancestors,
    /// Subtypes down the IS-A graph.
    Descendants,
    /// One hop each way: parents, children, and defining attributes.
    Neighbourhood,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    Tree,
    Dot,
    Mermaid,
    /// SVG rendered in-process (requires the `diagram-svg` Cargo feature).
    #[cfg(feature = "diagram-svg")]
    Svg,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum LabelStyle {
    Pt,
    Fsn,
    Both,
    Id,
}

/// One out-edge from a node in the diagram.
struct Child {
    id: String,
    /// Edge caption (`is a`, an attribute type name, …). `None` = plain IS-A.
    edge: Option<String>,
    /// Relationship group (`Some(g>0)` clusters under "role group g" in trees).
    group: Option<i64>,
}

/// The built diagram: a focus root plus an adjacency map of out-edges.
struct Diagram {
    root: String,
    adj: BTreeMap<String, Vec<Child>>,
}

pub fn run(args: Args) -> Result<()> {
    let concept = if args.concept == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("reading concept id from stdin")?;
        s.split_whitespace()
            .next()
            .map(str::to_string)
            .context("no concept id on stdin")?
    } else {
        args.concept.clone()
    };

    validate_graph_id(&concept)?;
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db, None)?;

    let labeler = Labeler::new(&conn, args.labels);
    anyhow::ensure!(
        labeler.exists(&concept),
        "concept {concept} not found in {}",
        db.display()
    );

    let wants_attrs = matches!(args.view, View::Definition | View::Neighbourhood);
    if wants_attrs && !eval::has_relationships_table(&conn) {
        eprintln!(
            "note: this database has no 'concept_relationships' table, so attribute \
             relationships are omitted. Rebuild with a current sct (`sct ndjson` then \
             `sct sqlite`) to include them."
        );
    }
    if args.format == Format::Dot && !labeler.has_definition_status {
        eprintln!(
            "note: this database has no 'definition_status' column, so primitive and fully-defined concepts use the same style. Rebuild with a current sct (`sct ndjson` then `sct sqlite`) to enable definition-status styling."
        );
    }

    let diagram = build(&conn, &concept, args.view, args.depth)?;
    for id in all_nodes(&diagram) {
        validate_graph_id(&id)?;
    }

    let (nodes, edges) = diagram.counts();
    let rendered = match args.format {
        Format::Tree => render_tree(&diagram, &labeler, args.ascii),
        Format::Dot => render_dot(&diagram, &labeler),
        Format::Mermaid => render_mermaid(&diagram, &labeler),
        #[cfg(feature = "diagram-svg")]
        Format::Svg => render_svg(&diagram, &labeler)?,
    };

    match &args.output {
        Some(path) => {
            std::fs::write(path, &rendered)
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!(
                "wrote {} ({nodes} node(s), {edges} edge(s)) to {}",
                format_name(args.format),
                path.display()
            );
        }
        None => {
            let mut out = std::io::stdout().lock();
            out.write_all(rendered.as_bytes())?;
            eprintln!("{nodes} node(s), {edges} edge(s)");
        }
    }
    Ok(())
}

fn validate_graph_id(id: &str) -> Result<()> {
    crate::sctid::validate_syntax(id).context("invalid diagram SCTID")
}

impl Diagram {
    fn counts(&self) -> (usize, usize) {
        let mut nodes = BTreeSet::new();
        nodes.insert(self.root.as_str());
        let mut edges = 0;
        for (from, kids) in &self.adj {
            nodes.insert(from.as_str());
            for c in kids {
                nodes.insert(c.id.as_str());
                edges += 1;
            }
        }
        (nodes.len(), edges)
    }
}

fn build(conn: &Connection, root: &str, view: View, depth: Option<usize>) -> Result<Diagram> {
    let mut adj: BTreeMap<String, Vec<Child>> = BTreeMap::new();
    match view {
        View::Definition => {
            let mut kids = Vec::new();
            for p in graph_neighbours(conn, root, Dir::Up)? {
                kids.push(Child {
                    id: p,
                    edge: Some("is a".into()),
                    group: None,
                });
            }
            for (type_id, dest, group) in eval::relationships(conn, root)? {
                validate_graph_id(&type_id)?;
                kids.push(Child {
                    id: dest,
                    edge: Some(type_label(conn, &type_id)),
                    group: Some(group),
                });
            }
            if !kids.is_empty() {
                adj.insert(root.to_string(), kids);
            }
        }
        View::Ancestors => bfs(conn, root, Dir::Up, depth, &mut adj)?,
        View::Descendants => bfs(conn, root, Dir::Down, depth.or(Some(1)), &mut adj)?,
        View::Neighbourhood => {
            let mut kids = Vec::new();
            for p in graph_neighbours(conn, root, Dir::Up)? {
                kids.push(Child {
                    id: p,
                    edge: Some("is a".into()),
                    group: None,
                });
            }
            for c in graph_neighbours(conn, root, Dir::Down)? {
                kids.push(Child {
                    id: c,
                    edge: Some("subtype".into()),
                    group: None,
                });
            }
            for (type_id, dest, group) in eval::relationships(conn, root)? {
                validate_graph_id(&type_id)?;
                kids.push(Child {
                    id: dest,
                    edge: Some(type_label(conn, &type_id)),
                    group: Some(group),
                });
            }
            if !kids.is_empty() {
                adj.insert(root.to_string(), kids);
            }
        }
    }
    Ok(Diagram {
        root: root.to_string(),
        adj,
    })
}

enum Dir {
    Up,
    Down,
}

fn graph_neighbours(conn: &Connection, id: &str, dir: Dir) -> Result<Vec<String>> {
    // Validate raw references before the shared traversal casts them to integers.
    let sql = match dir {
        Dir::Up => "SELECT parent_id FROM concept_isa WHERE child_id = ?1",
        Dir::Down => "SELECT child_id FROM concept_isa WHERE parent_id = ?1",
    };
    let mut stmt = conn.prepare_cached(sql)?;
    for neighbour in stmt.query_map([id], |row| row.get::<_, String>(0))? {
        validate_graph_id(&neighbour?)?;
    }
    match dir {
        Dir::Up => eval::parents(conn, id),
        Dir::Down => eval::children(conn, id),
    }
}

/// Breadth-first walk up (ancestors) or down (descendants) the IS-A graph,
/// building adjacency once per node. `depth = None` means unbounded (to a root).
fn bfs(
    conn: &Connection,
    root: &str,
    dir: Dir,
    depth: Option<usize>,
    adj: &mut BTreeMap<String, Vec<Child>>,
) -> Result<()> {
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    queue.push_back((root.to_string(), 0));
    let mut enqueued: HashSet<String> = HashSet::new();
    enqueued.insert(root.to_string());

    while let Some((node, d)) = queue.pop_front() {
        if depth.is_some_and(|max| d >= max) {
            continue;
        }
        let neighbours = match dir {
            Dir::Up => graph_neighbours(conn, &node, Dir::Up)?,
            Dir::Down => graph_neighbours(conn, &node, Dir::Down)?,
        };
        let mut kids = Vec::new();
        for n in neighbours {
            let edge = match dir {
                Dir::Up => Some("is a".to_string()),
                Dir::Down => None,
            };
            kids.push(Child {
                id: n.clone(),
                edge,
                group: None,
            });
            if enqueued.insert(n.clone()) {
                queue.push_back((n, d + 1));
            }
        }
        if !kids.is_empty() {
            adj.insert(node, kids);
        }
    }
    Ok(())
}

/// Preferred term of an attribute *type* concept, for edge captions.
fn type_label(conn: &Connection, id: &str) -> String {
    conn.query_row(
        "SELECT preferred_term FROM concepts WHERE id = ?1",
        [id],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_else(|_| id.to_string())
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

struct Labeler<'a> {
    conn: &'a Connection,
    style: LabelStyle,
    cache: RefCell<HashMap<String, Option<(String, String)>>>,
    has_definition_status: bool,
    definition_status_cache: RefCell<HashMap<String, DefinitionStatus>>,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum DefinitionStatus {
    Primitive,
    Defined,
    #[default]
    Unknown,
}

impl DefinitionStatus {
    fn from_sctid(id: &str) -> Self {
        match id {
            "900000000000074008" => Self::Primitive,
            "900000000000073002" => Self::Defined,
            _ => Self::Unknown,
        }
    }
}

impl<'a> Labeler<'a> {
    fn new(conn: &'a Connection, style: LabelStyle) -> Self {
        Self {
            conn,
            style,
            cache: RefCell::new(HashMap::new()),
            has_definition_status: conn
                .query_row(
                    "SELECT 1 FROM pragma_table_info('concepts') WHERE name = 'definition_status'",
                    [],
                    |_| Ok(()),
                )
                .is_ok(),
            definition_status_cache: RefCell::new(HashMap::new()),
        }
    }

    /// `(preferred_term, fsn)` for `id`, or `None` if absent. Memoised.
    fn terms(&self, id: &str) -> Option<(String, String)> {
        if let Some(hit) = self.cache.borrow().get(id) {
            return hit.clone();
        }
        let got = self
            .conn
            .query_row(
                "SELECT preferred_term, fsn FROM concepts WHERE id = ?1",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok();
        self.cache.borrow_mut().insert(id.to_string(), got.clone());
        got
    }

    fn exists(&self, id: &str) -> bool {
        self.terms(id).is_some()
    }

    fn caption(&self, id: &str) -> String {
        let (pt, fsn) = self.terms(id).unwrap_or_default();
        match self.style {
            LabelStyle::Id => id.to_string(),
            LabelStyle::Pt => name_with_id(&pt, id),
            LabelStyle::Fsn => name_with_id(&fsn, id),
            LabelStyle::Both => {
                if pt.is_empty() && fsn.is_empty() {
                    id.to_string()
                } else {
                    format!("{pt} — {fsn} ({id})")
                }
            }
        }
    }

    fn definition_status(&self, id: &str) -> DefinitionStatus {
        if !self.has_definition_status {
            return DefinitionStatus::Unknown;
        }
        if let Some(status) = self.definition_status_cache.borrow().get(id) {
            return *status;
        }
        let status = self
            .conn
            .query_row(
                "SELECT definition_status FROM concepts WHERE id = ?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .map(|id| DefinitionStatus::from_sctid(&id))
            .unwrap_or_default();
        self.definition_status_cache
            .borrow_mut()
            .insert(id.to_string(), status);
        status
    }
}

fn name_with_id(name: &str, id: &str) -> String {
    if name.is_empty() {
        id.to_string()
    } else {
        format!("{name} ({id})")
    }
}

// ---------------------------------------------------------------------------
// Tree rendering
// ---------------------------------------------------------------------------

/// A tree node to display: a concept (which may expand) or a role-group box.
enum Disp {
    Concept { id: String, edge: Option<String> },
    Group { num: i64, kids: Vec<Disp> },
}

/// Build the display children of `node`: plain IS-A / ungrouped edges inline,
/// grouped attributes (`group > 0`) collected under "role group N" boxes.
fn disp_children(diagram: &Diagram, node: &str) -> Vec<Disp> {
    let Some(kids) = diagram.adj.get(node) else {
        return Vec::new();
    };
    let mut direct = Vec::new();
    let mut groups: BTreeMap<i64, Vec<Disp>> = BTreeMap::new();
    for c in kids {
        let concept = Disp::Concept {
            id: c.id.clone(),
            edge: c.edge.clone(),
        };
        match c.group {
            Some(g) if g > 0 => groups.entry(g).or_default().push(concept),
            _ => direct.push(concept),
        }
    }
    for (num, kids) in groups {
        direct.push(Disp::Group { num, kids });
    }
    direct
}

fn render_tree(diagram: &Diagram, labeler: &Labeler, ascii: bool) -> String {
    let mut out = String::new();
    out.push_str(&crate::format::single_line(&labeler.caption(&diagram.root)));
    out.push('\n');
    let mut printed: HashSet<String> = HashSet::new();
    printed.insert(diagram.root.clone());
    let items = disp_children(diagram, &diagram.root);
    render_list(&items, "", diagram, labeler, ascii, &mut printed, &mut out);
    out
}

fn render_list(
    items: &[Disp],
    prefix: &str,
    diagram: &Diagram,
    labeler: &Labeler,
    ascii: bool,
    printed: &mut HashSet<String>,
    out: &mut String,
) {
    let n = items.len();
    for (i, item) in items.iter().enumerate() {
        let last = i + 1 == n;
        let (branch, cont) = glyphs(ascii, last);
        match item {
            Disp::Concept { id, edge } => {
                let lbl = edge
                    .as_ref()
                    .map(|e| format!("{}: ", crate::format::single_line(e)))
                    .unwrap_or_default();
                let caption = labeler.caption(id);
                let cap = crate::format::single_line(&caption);
                let has_kids = diagram.adj.get(id).is_some_and(|k| !k.is_empty());
                if has_kids && printed.contains(id) {
                    let mark = if ascii { " ^" } else { " ↑" };
                    out.push_str(&format!("{prefix}{branch}{lbl}{cap}{mark}\n"));
                } else {
                    out.push_str(&format!("{prefix}{branch}{lbl}{cap}\n"));
                    if has_kids {
                        printed.insert(id.clone());
                        let kids = disp_children(diagram, id);
                        render_list(
                            &kids,
                            &format!("{prefix}{cont}"),
                            diagram,
                            labeler,
                            ascii,
                            printed,
                            out,
                        );
                    }
                }
            }
            Disp::Group { num, kids } => {
                out.push_str(&format!("{prefix}{branch}role group {num}\n"));
                render_list(
                    kids,
                    &format!("{prefix}{cont}"),
                    diagram,
                    labeler,
                    ascii,
                    printed,
                    out,
                );
            }
        }
    }
}

fn glyphs(ascii: bool, last: bool) -> (&'static str, &'static str) {
    match (ascii, last) {
        (false, false) => ("├── ", "│   "),
        (false, true) => ("└── ", "    "),
        (true, false) => ("|-- ", "|   "),
        (true, true) => ("`-- ", "    "),
    }
}

// ---------------------------------------------------------------------------
// DOT / Mermaid rendering
// ---------------------------------------------------------------------------

fn all_nodes(diagram: &Diagram) -> BTreeSet<String> {
    let mut nodes = BTreeSet::new();
    nodes.insert(diagram.root.clone());
    for (from, kids) in &diagram.adj {
        nodes.insert(from.clone());
        for c in kids {
            nodes.insert(c.id.clone());
        }
    }
    nodes
}

fn render_dot(diagram: &Diagram, labeler: &Labeler) -> String {
    render_dot_with_clusters(diagram, labeler, true)
}

fn render_dot_with_clusters(
    diagram: &Diagram,
    labeler: &Labeler,
    include_clusters: bool,
) -> String {
    let mut out = String::from(
        "digraph sct {\n  rankdir=TB;\n  node [shape=box, fontsize=10, style=rounded];\n",
    );
    for id in all_nodes(diagram) {
        let style = match labeler.definition_status(&id) {
            DefinitionStatus::Primitive => "style=\"rounded,dashed\", color=\"#64748b\"",
            DefinitionStatus::Defined => {
                "style=\"rounded,filled\", color=\"#166534\", fillcolor=\"#dcfce7\""
            }
            DefinitionStatus::Unknown => "",
        };
        let separator = if style.is_empty() { "" } else { ", " };
        out.push_str(&format!(
            "  \"{id}\" [label=\"{}\"{separator}{style}];\n",
            dot_escape(&labeler.caption(&id))
        ));
    }
    if include_clusters {
        for ((source, group), ids) in dot_role_groups(diagram) {
            out.push_str(&format!(
                "  subgraph cluster_{source}_{group} {{\n    label=\"role group {group}\";\n    color=\"#94a3b8\";\n    style=\"rounded,dashed\";\n"
            ));
            for id in ids {
                out.push_str(&format!("    \"{id}\";\n"));
            }
            out.push_str("  }\n");
        }
    }
    for (from, kids) in &diagram.adj {
        for c in kids {
            match &c.edge {
                Some(e) if e == "is a" => out.push_str(&format!(
                    "  \"{from}\" -> \"{}\" [label=\"{}\"];\n",
                    c.id,
                    dot_escape(e)
                )),
                Some(e) => out.push_str(&format!(
                    "  \"{from}\" -> \"{}\" [label=\"{}\", color=\"#0f766e\", fontcolor=\"#0f766e\", penwidth=1.5];\n",
                    c.id,
                    dot_escape(e)
                )),
                None => out.push_str(&format!("  \"{from}\" -> \"{}\";\n", c.id)),
            }
        }
    }
    out.push_str("}\n");
    out
}

#[cfg(feature = "diagram-svg")]
fn render_svg(diagram: &Diagram, labeler: &Labeler) -> Result<String> {
    use layout::backends::svg::SVGWriter;
    use layout::gv::parser::DotParser;
    use layout::gv::GraphBuilder;

    // layout-rs deliberately does not support Graphviz clusters, so preserve
    // role-group information through the already-labelled coloured edges while
    // omitting cluster subgraphs from this renderer's private DOT input.
    let dot = render_dot_with_clusters(diagram, labeler, false);
    let mut parser = DotParser::new(&dot);
    let graph = parser
        .process()
        .map_err(|err| anyhow::anyhow!("parsing generated DOT for SVG: {err}"))?;
    let mut builder = GraphBuilder::new();
    builder.visit_graph(&graph);
    let mut graph = builder.get();
    let mut writer = SVGWriter::new();
    graph.do_it(false, false, false, &mut writer);
    Ok(writer.finalize())
}

/// Attribute relationship destinations grouped by their RF2 role group for DOT
/// cluster boxes. A destination is emitted at most once per cluster.
fn dot_role_groups(diagram: &Diagram) -> BTreeMap<(String, i64), BTreeSet<String>> {
    let mut groups = BTreeMap::new();
    for (source, kids) in &diagram.adj {
        for child in kids {
            if let Some(group) = child.group.filter(|group| *group > 0) {
                groups
                    .entry((source.clone(), group))
                    .or_insert_with(BTreeSet::new)
                    .insert(child.id.clone());
            }
        }
    }
    groups
}

fn render_mermaid(diagram: &Diagram, labeler: &Labeler) -> String {
    let mut out = String::from("graph TD\n");
    for id in all_nodes(diagram) {
        out.push_str(&format!(
            "  c{id}[\"{}\"]\n",
            mermaid_escape(&labeler.caption(&id))
        ));
    }
    for (from, kids) in &diagram.adj {
        for c in kids {
            match &c.edge {
                Some(e) => {
                    out.push_str(&format!("  c{from} -->|{}| c{}\n", mermaid_escape(e), c.id))
                }
                None => out.push_str(&format!("  c{from} --> c{}\n", c.id)),
            }
        }
    }
    out
}

fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn mermaid_escape(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_punctuation() {
            // Mermaid's entity syntax keeps label text out of graph/HTML syntax.
            write!(out, "#{};", ch as u32).unwrap();
        } else if ch.is_whitespace() || ch.is_control() {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn format_name(f: Format) -> &'static str {
    match f {
        Format::Tree => "tree",
        Format::Dot => "DOT",
        Format::Mermaid => "Mermaid",
        #[cfg(feature = "diagram-svg")]
        Format::Svg => "SVG",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hierarchy: 1 ─ 2 ─ {4,5}; plus attribute rels on 2.
    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (id TEXT PRIMARY KEY, preferred_term TEXT NOT NULL,
                 fsn TEXT NOT NULL, active INTEGER NOT NULL, definition_status TEXT);
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             CREATE TABLE concept_relationships (source_id TEXT NOT NULL, type_id TEXT NOT NULL,
                 destination_id TEXT NOT NULL, group_num INTEGER NOT NULL);",
        )
        .unwrap();
        for (id, pt, definition_status) in [
            ("1", "Root", "900000000000074008"),
            ("2", "Focus", "900000000000073002"),
            ("4", "Child four", "900000000000074008"),
            ("5", "Child five", "900000000000073002"),
            ("116680003", "Is a", "900000000000074008"),
            ("363698007", "Finding site", "900000000000074008"),
            ("999", "Some site", "900000000000074008"),
        ] {
            conn.execute(
                "INSERT INTO concepts (id, preferred_term, fsn, active, definition_status)
                 VALUES (?1, ?2, ?2, 1, ?3)",
                [id, pt, definition_status],
            )
            .unwrap();
        }
        for (c, p) in [("2", "1"), ("4", "2"), ("5", "2")] {
            conn.execute(
                "INSERT INTO concept_isa (child_id, parent_id) VALUES (?1, ?2)",
                [c, p],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO concept_relationships VALUES ('2', '363698007', '999', 1)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn descendants_tree_lists_children() {
        let conn = fixture();
        let d = build(&conn, "1", View::Descendants, Some(2)).unwrap();
        let labeler = Labeler::new(&conn, LabelStyle::Pt);
        let tree = render_tree(&d, &labeler, false);
        assert!(tree.starts_with("Root (1)"));
        assert!(tree.contains("Focus (2)"));
        assert!(tree.contains("Child four (4)"));
    }

    #[test]
    fn definition_tree_groups_attributes() {
        let conn = fixture();
        let d = build(&conn, "2", View::Definition, None).unwrap();
        let labeler = Labeler::new(&conn, LabelStyle::Pt);
        let tree = render_tree(&d, &labeler, false);
        assert!(tree.contains("is a: Root (1)"));
        assert!(tree.contains("role group 1"));
        assert!(tree.contains("Finding site: Some site (999)"));
    }

    #[test]
    fn ascii_mode_uses_ascii_glyphs() {
        let conn = fixture();
        let d = build(&conn, "1", View::Descendants, Some(1)).unwrap();
        let labeler = Labeler::new(&conn, LabelStyle::Id);
        let tree = render_tree(&d, &labeler, true);
        assert!(tree.contains("`-- ") || tree.contains("|-- "));
        assert!(!tree.contains('├'));
    }

    #[test]
    fn dot_and_mermaid_are_wellformed() {
        let conn = fixture();
        let d = build(&conn, "2", View::Definition, None).unwrap();
        let labeler = Labeler::new(&conn, LabelStyle::Pt);
        let dot = render_dot(&d, &labeler);
        assert!(dot.starts_with("digraph sct {"));
        assert!(dot.trim_end().ends_with('}'));
        assert!(dot.contains("style=\"rounded,filled\""));
        assert!(dot.contains("style=\"rounded,dashed\""));
        assert!(dot.contains("subgraph cluster_2_1"));
        assert!(dot.contains("color=\"#0f766e\""));
        let mm = render_mermaid(&d, &labeler);
        assert!(mm.starts_with("graph TD"));
        assert!(mm.contains("c2 -->|is a| c1"));
    }

    #[cfg(feature = "diagram-svg")]
    #[test]
    fn svg_is_wellformed_and_contains_focus_id() {
        let conn = fixture();
        let diagram = build(&conn, "2", View::Definition, None).unwrap();
        let labeler = Labeler::new(&conn, LabelStyle::Pt);
        let svg = render_svg(&diagram, &labeler).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("Focus (2)"));
        assert!(svg.contains("</svg>"));
    }
}
