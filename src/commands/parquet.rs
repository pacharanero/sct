//! `sct parquet` — Export a SNOMED CT NDJSON artefact to a Parquet file.
//!
//! Array/object columns (synonyms, hierarchy_path, parents, attributes) are
//! stored as JSON strings so DuckDB can query them with `json_extract` /
//! `json_each` without any import step.
//!
//! Example DuckDB query after export:
//!   duckdb -c "SELECT hierarchy, COUNT(*) FROM 'snomed.parquet' GROUP BY hierarchy ORDER BY 2 DESC"

use anyhow::{Context, Result};
use arrow::array::{ArrayRef, BooleanBuilder, Int64Builder, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::schema::ConceptRecord;

const BATCH_SIZE: usize = 50_000;

#[derive(Parser, Debug)]
pub struct Args {
    /// Input NDJSON file produced by `sct ndjson`. Use `-` for stdin.
    #[arg(long, short)]
    pub input: PathBuf,

    /// Output Parquet file.
    #[arg(long, short, default_value = "snomed.parquet")]
    pub output: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let input: Box<dyn std::io::Read> = if args.input.as_os_str() == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(
            std::fs::File::open(&args.input)
                .with_context(|| format!("opening {}", args.input.display()))?,
        )
    };

    let reader = BufReader::new(input);

    let schema = Arc::new(parquet_schema());
    let file = std::fs::File::create(&args.output)
        .with_context(|| format!("creating {}", args.output.display()))?;
    let props = WriterProperties::builder().build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .context("creating Parquet writer")?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} [{elapsed_precise}] {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(120));
    pb.set_message("Writing Parquet...");

    let mut batch_buf: Vec<ConceptRecord> = Vec::with_capacity(BATCH_SIZE);
    let mut total = 0usize;

    for line in reader.lines() {
        let line = line.context("reading input")?;
        if line.trim().is_empty() {
            continue;
        }
        let record: ConceptRecord = serde_json::from_str(&line).context("parsing NDJSON record")?;
        batch_buf.push(record);
        total += 1;

        if batch_buf.len() >= BATCH_SIZE {
            let batch = build_batch(&schema, &batch_buf)?;
            writer.write(&batch).context("writing Parquet batch")?;
            batch_buf.clear();
            pb.set_message(format!("{} concepts written...", total));
        }
    }

    // Final partial batch
    if !batch_buf.is_empty() {
        let batch = build_batch(&schema, &batch_buf)?;
        writer
            .write(&batch)
            .context("writing final Parquet batch")?;
    }

    writer.close().context("finalising Parquet file")?;

    pb.finish_with_message(format!(
        "Done. {} concepts → {}",
        total,
        args.output.display()
    ));
    Ok(())
}

fn parquet_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("fsn", DataType::Utf8, false),
        Field::new("preferred_term", DataType::Utf8, false),
        Field::new("synonyms", DataType::Utf8, true), // JSON array
        Field::new("hierarchy", DataType::Utf8, true),
        Field::new("hierarchy_path", DataType::Utf8, true), // JSON array
        Field::new("parents", DataType::Utf8, true),        // JSON array of {id,fsn}
        Field::new("children_count", DataType::Int64, false),
        Field::new("active", DataType::Boolean, false),
        Field::new("module", DataType::Utf8, true),
        Field::new("effective_time", DataType::Utf8, true),
        Field::new("attributes", DataType::Utf8, true), // JSON object
        Field::new("schema_version", DataType::Int64, false),
    ])
}

fn build_batch(schema: &Arc<Schema>, records: &[ConceptRecord]) -> Result<RecordBatch> {
    let mut ids = StringBuilder::new();
    let mut fsns = StringBuilder::new();
    let mut preferred_terms = StringBuilder::new();
    let mut synonyms = StringBuilder::new();
    let mut hierarchies = StringBuilder::new();
    let mut hierarchy_paths = StringBuilder::new();
    let mut parents_col = StringBuilder::new();
    let mut children_counts = Int64Builder::new();
    let mut actives = BooleanBuilder::new();
    let mut modules = StringBuilder::new();
    let mut effective_times = StringBuilder::new();
    let mut attributes_col = StringBuilder::new();
    let mut schema_versions = Int64Builder::new();

    for r in records {
        ids.append_value(&r.id);
        fsns.append_value(&r.fsn);
        preferred_terms.append_value(&r.preferred_term);
        synonyms.append_value(serde_json::to_string(&r.synonyms)?);
        hierarchies.append_value(&r.hierarchy);
        hierarchy_paths.append_value(serde_json::to_string(&r.hierarchy_path)?);
        parents_col.append_value(serde_json::to_string(&r.parents)?);
        children_counts.append_value(r.children_count as i64);
        actives.append_value(r.active);
        modules.append_value(&r.module);
        effective_times.append_value(&r.effective_time);
        attributes_col.append_value(serde_json::to_string(&r.attributes)?);
        schema_versions.append_value(r.schema_version as i64);
    }

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids.finish()) as ArrayRef,
            Arc::new(fsns.finish()) as ArrayRef,
            Arc::new(preferred_terms.finish()) as ArrayRef,
            Arc::new(synonyms.finish()) as ArrayRef,
            Arc::new(hierarchies.finish()) as ArrayRef,
            Arc::new(hierarchy_paths.finish()) as ArrayRef,
            Arc::new(parents_col.finish()) as ArrayRef,
            Arc::new(children_counts.finish()) as ArrayRef,
            Arc::new(actives.finish()) as ArrayRef,
            Arc::new(modules.finish()) as ArrayRef,
            Arc::new(effective_times.finish()) as ArrayRef,
            Arc::new(attributes_col.finish()) as ArrayRef,
            Arc::new(schema_versions.finish()) as ArrayRef,
        ],
    )
    .context("building Arrow record batch")?;

    Ok(batch)
}
