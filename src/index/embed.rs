#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};

#[cfg(not(test))]
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

#[cfg(test)]
use super::types::VECTOR_DIMENSIONS;
use crate::{ZgResult, other};

#[cfg(not(test))]
static TEXT_EMBEDDER: OnceLock<Mutex<TextEmbedding>> = OnceLock::new();

pub(crate) fn embed_passages(texts: &[String]) -> ZgResult<Vec<Vec<f32>>> {
    embed(texts, "passage")
}

pub(crate) fn embed_query(text: &str) -> ZgResult<Vec<f32>> {
    let mut vectors = embed(&[text.to_string()], "query")?;
    vectors
        .pop()
        .ok_or_else(|| other("fastembed returned no query vector"))
}

fn embed(texts: &[String], prefix: &str) -> ZgResult<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let prefixed = texts
        .iter()
        .map(|text| format!("{prefix}: {text}"))
        .collect::<Vec<_>>();

    #[cfg(test)]
    {
        Ok(prefixed
            .iter()
            .map(|text| deterministic_test_embedding(text))
            .collect())
    }

    #[cfg(not(test))]
    let model = embedder()?;
    #[cfg(not(test))]
    let mut guard = model
        .lock()
        .map_err(|_| other("fastembed model lock poisoned"))?;
    #[cfg(not(test))]
    let embeddings = guard.embed(prefixed, None)?;

    #[cfg(not(test))]
    Ok(embeddings)
}

#[cfg(not(test))]
fn embedder() -> ZgResult<&'static Mutex<TextEmbedding>> {
    if let Some(embedder) = TEXT_EMBEDDER.get() {
        return Ok(embedder);
    }

    let embedder = build_embedder().map(Mutex::new).map_err(|error| {
        other(format!(
            "failed to initialize fastembed backend: {error}; {}",
            fastembed_env_summary()
        ))
    })?;
    let _ = TEXT_EMBEDDER.set(embedder);

    TEXT_EMBEDDER
        .get()
        .ok_or_else(|| other("failed to initialize fastembed backend"))
}

#[cfg(not(test))]
fn build_embedder() -> ZgResult<TextEmbedding> {
    TextEmbedding::try_new(TextInitOptions::new(
        EmbeddingModel::ParaphraseMLMiniLML12V2Q,
    ))
}

#[cfg(not(test))]
fn fastembed_env_summary() -> String {
    let keys = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "HF_HOME",
        "HF_ENDPOINT",
        "FASTEMBED_CACHE_DIR",
    ];
    let values = keys
        .iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| format!("{key}={value}"))
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        "proxy/cache env: <none>".to_string()
    } else {
        format!("proxy/cache env: {}", values.join(", "))
    }
}

#[cfg(test)]
fn deterministic_test_embedding(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0; VECTOR_DIMENSIONS];
    for (index, byte) in text.bytes().enumerate() {
        let slot = index % VECTOR_DIMENSIONS;
        vector[slot] += f32::from(byte % 31) + 1.0;
    }
    vector
}
