// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Best-effort identity metadata for models served by Ollama.

use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct ModelInfo {
    pub version: Option<String>,
    pub resolved_model: Option<String>,
    pub model_digest: Option<String>,
}

pub fn inspect(base_url: &str, requested_model: &str) -> ModelInfo {
    let base_url = base_url.trim_end_matches('/');
    let version = get_json(&format!("{base_url}/api/version"))
        .and_then(|value| value.get("version")?.as_str().map(str::to_string));
    let model = get_json(&format!("{base_url}/api/tags"))
        .and_then(|value| select_model(value.get("models")?.as_array()?, requested_model).cloned());
    ModelInfo {
        version,
        resolved_model: model.as_ref().and_then(|value| {
            value
                .get("name")
                .or_else(|| value.get("model"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }),
        model_digest: model
            .as_ref()
            .and_then(|value| value.get("digest"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    }
}

fn get_json(url: &str) -> Option<serde_json::Value> {
    ureq::get(url).call().ok()?.into_body().read_json().ok()
}

fn model_names_match(available: &str, requested: &str) -> bool {
    crate::commands::embedding_profile::compatible_models(available, requested)
}

fn select_model<'a>(
    models: &'a [serde_json::Value],
    requested: &str,
) -> Option<&'a serde_json::Value> {
    models
        .iter()
        .find(|model| {
            model_names(model)
                .into_iter()
                .flatten()
                .any(|name| name == requested)
        })
        .or_else(|| {
            models.iter().find(|model| {
                model_names(model)
                    .into_iter()
                    .flatten()
                    .any(|name| model_names_match(name, requested))
            })
        })
}

fn model_names(model: &serde_json::Value) -> [Option<&str>; 2] {
    [
        model.get("name").and_then(serde_json::Value::as_str),
        model.get("model").and_then(serde_json::Value::as_str),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_aliases_match_by_embedding_profile() {
        assert!(model_names_match(
            "nomic-embed-text:latest",
            "nomic-embed-text:v1.5"
        ));
        assert!(!model_names_match(
            "nomic-embed-text:v1.5",
            "nomic-embed-text-v2-moe"
        ));
    }

    #[test]
    fn exact_model_tag_wins_over_a_compatible_alias() {
        let models = serde_json::json!([
            {"name": "nomic-embed-text:latest", "digest": "latest"},
            {"name": "nomic-embed-text:v1.5", "digest": "pinned"}
        ]);
        let selected = select_model(models.as_array().unwrap(), "nomic-embed-text:v1.5").unwrap();
        assert_eq!(selected["digest"], "pinned");
    }
}
