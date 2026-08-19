// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Model-specific document/query formatting for embedding retrieval.

use anyhow::Result;

pub const METADATA_KEY: &str = "sct.embedding_profile";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingProfile {
    pub id: &'static str,
    pub expected_dimensions: usize,
    pub max_input_tokens: usize,
}

const NOMIC_V1: EmbeddingProfile = EmbeddingProfile {
    id: "nomic-embed-text-v1.5/sct-1",
    expected_dimensions: 768,
    max_input_tokens: 2048,
};

const NOMIC_V2_MOE: EmbeddingProfile = EmbeddingProfile {
    id: "nomic-embed-text-v2-moe/sct-1",
    expected_dimensions: 768,
    max_input_tokens: 512,
};

const QWEN3_06B: EmbeddingProfile = EmbeddingProfile {
    id: "qwen3-embedding-0.6b/sct-clinical-retrieval-1",
    expected_dimensions: 1024,
    max_input_tokens: 32_768,
};

const EMBEDDING_GEMMA: EmbeddingProfile = EmbeddingProfile {
    id: "embeddinggemma-300m/sct-retrieval-1",
    expected_dimensions: 768,
    max_input_tokens: 2048,
};

const QWEN_CLINICAL_RETRIEVAL_TASK: &str =
    "Given a clinical terminology search query, retrieve the SNOMED CT concept description that best matches the query";

impl EmbeddingProfile {
    pub fn accepts_legacy_text_scheme(self) -> bool {
        matches!(
            self.id,
            "nomic-embed-text-v1.5/sct-1" | "nomic-embed-text-v2-moe/sct-1"
        )
    }

    pub fn format_document(self, text: &str) -> String {
        match self.id {
            "nomic-embed-text-v1.5/sct-1" | "nomic-embed-text-v2-moe/sct-1" => {
                format!("search_document: {text}")
            }
            "qwen3-embedding-0.6b/sct-clinical-retrieval-1" => text.to_string(),
            "embeddinggemma-300m/sct-retrieval-1" => format!("title: none | text: {text}"),
            _ => unreachable!("all registered profiles define document formatting"),
        }
    }

    pub fn format_query(self, text: &str) -> String {
        match self.id {
            "nomic-embed-text-v1.5/sct-1" | "nomic-embed-text-v2-moe/sct-1" => {
                format!("search_query: {text}")
            }
            "qwen3-embedding-0.6b/sct-clinical-retrieval-1" => {
                format!("Instruct: {QWEN_CLINICAL_RETRIEVAL_TASK}\nQuery:{text}")
            }
            "embeddinggemma-300m/sct-retrieval-1" => {
                format!("task: search result | query: {text}")
            }
            _ => unreachable!("all registered profiles define query formatting"),
        }
    }
}

pub fn resolve(model: &str) -> Result<EmbeddingProfile> {
    match model {
        "nomic-embed-text" | "nomic-embed-text:latest" | "nomic-embed-text:v1.5" => Ok(NOMIC_V1),
        "nomic-embed-text-v2-moe" | "nomic-embed-text-v2-moe:latest" => Ok(NOMIC_V2_MOE),
        "qwen3-embedding:0.6b" => Ok(QWEN3_06B),
        "embeddinggemma" | "embeddinggemma:latest" | "embeddinggemma:300m" => Ok(EMBEDDING_GEMMA),
        _ => anyhow::bail!(
            "embedding model {model:?} is not supported by sct's model-aware adapters; \
             supported models: nomic-embed-text, nomic-embed-text:v1.5, \
             nomic-embed-text-v2-moe, qwen3-embedding:0.6b, embeddinggemma"
        ),
    }
}

pub fn compatible_models(stored: &str, requested: &str) -> bool {
    match (resolve(stored), resolve(requested)) {
        (Ok(stored), Ok(requested)) => stored.id == requested.id,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomic_aliases_share_one_versioned_profile() {
        for model in [
            "nomic-embed-text",
            "nomic-embed-text:latest",
            "nomic-embed-text:v1.5",
        ] {
            assert_eq!(resolve(model).unwrap(), NOMIC_V1);
        }
    }

    #[test]
    fn nomic_profile_formats_both_sides_of_asymmetric_retrieval() {
        for model in ["nomic-embed-text", "nomic-embed-text-v2-moe"] {
            let profile = resolve(model).unwrap();
            assert_eq!(
                profile.format_document("Myocardial infarction"),
                "search_document: Myocardial infarction"
            );
            assert_eq!(
                profile.format_query("heart attack"),
                "search_query: heart attack"
            );
        }
    }

    #[test]
    fn nomic_v2_is_a_distinct_768_dimension_profile() {
        let v1 = resolve("nomic-embed-text").unwrap();
        let v2 = resolve("nomic-embed-text-v2-moe").unwrap();
        assert_ne!(v1.id, v2.id);
        assert_eq!(v2.expected_dimensions, 768);
        assert_eq!(v2.max_input_tokens, 512);
        assert_eq!(resolve("nomic-embed-text-v2-moe:latest").unwrap(), v2);
        assert!(!compatible_models(
            "nomic-embed-text",
            "nomic-embed-text-v2-moe"
        ));
    }

    #[test]
    fn qwen_profile_uses_an_instruction_only_for_queries() {
        let profile = resolve("qwen3-embedding:0.6b").unwrap();
        assert_eq!(profile.expected_dimensions, 1024);
        assert_eq!(profile.max_input_tokens, 32_768);
        assert_eq!(
            profile.format_document("Myocardial infarction"),
            "Myocardial infarction"
        );
        assert_eq!(
            profile.format_query("heart attack"),
            format!("Instruct: {QWEN_CLINICAL_RETRIEVAL_TASK}\nQuery:heart attack")
        );
    }

    #[test]
    fn embeddinggemma_profile_uses_its_retrieval_prompts() {
        let profile = resolve("embeddinggemma").unwrap();
        assert_eq!(profile.expected_dimensions, 768);
        assert_eq!(profile.max_input_tokens, 2048);
        assert_eq!(
            profile.format_document("Myocardial infarction"),
            "title: none | text: Myocardial infarction"
        );
        assert_eq!(
            profile.format_query("heart attack"),
            "task: search result | query: heart attack"
        );
        assert_eq!(resolve("embeddinggemma:300m").unwrap(), profile);
    }

    #[test]
    fn arbitrary_ollama_models_are_rejected() {
        let error = resolve("mxbai-embed-large").unwrap_err().to_string();
        assert!(error.contains("not supported"));
        assert!(error.contains("embeddinggemma"));
    }

    #[test]
    fn only_nomic_profiles_accept_pre_profile_scheme_two_artifacts() {
        assert!(resolve("nomic-embed-text")
            .unwrap()
            .accepts_legacy_text_scheme());
        assert!(resolve("nomic-embed-text-v2-moe")
            .unwrap()
            .accepts_legacy_text_scheme());
        assert!(!resolve("qwen3-embedding:0.6b")
            .unwrap()
            .accepts_legacy_text_scheme());
        assert!(!resolve("embeddinggemma")
            .unwrap()
            .accepts_legacy_text_scheme());
    }
}
