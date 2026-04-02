pub mod codelist;
pub mod completions;
pub mod diff;
pub mod embed;
pub mod info;
pub mod lexical;
pub mod lookup;
pub mod markdown;
pub mod mcp;
pub mod ndjson;
pub mod parquet;
pub mod semantic;
pub mod sqlite;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(feature = "gui")]
pub mod gui;
