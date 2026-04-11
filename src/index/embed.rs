#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};

#[cfg(not(test))]
use fastembed::{
    EmbeddingModel, InitOptionsUserDefined, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};

#[cfg(test)]
use super::types::VECTOR_DIMENSIONS;
#[cfg(not(test))]
use crate::paths;
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

    let embedder = build_embedder()
        .map(Mutex::new)
        .map_err(|error| other(format!("failed to initialize fastembed backend: {error}")))?;
    let _ = TEXT_EMBEDDER.set(embedder);

    TEXT_EMBEDDER
        .get()
        .ok_or_else(|| other("failed to initialize fastembed backend"))
}

#[cfg(not(test))]
fn build_embedder() -> ZgResult<TextEmbedding> {
    if let Some(model_dir) = paths::bundled_embedding_model_dir() {
        if let Some(embedder) = try_user_defined_embedder(&model_dir)? {
            return Ok(embedder);
        }
    }

    Err(other(format!(
        "no bundled fastembed model found; place the {:?} model files under ZG_MODEL_DIR or <prefix>/share/zg/models",
        EmbeddingModel::BGESmallENV15
    )))
}

#[cfg(not(test))]
fn try_user_defined_embedder(model_dir: &std::path::Path) -> ZgResult<Option<TextEmbedding>> {
    let onnx_path = ["model_optimized.onnx", "model.onnx"]
        .into_iter()
        .map(|name| model_dir.join(name))
        .find(|path| path.exists());

    let Some(onnx_path) = onnx_path else {
        return Ok(None);
    };

    let tokenizer_file = model_dir.join("tokenizer.json");
    let config_file = model_dir.join("config.json");
    let special_tokens_map_file = model_dir.join("special_tokens_map.json");
    let tokenizer_config_file = model_dir.join("tokenizer_config.json");

    let required = [
        &tokenizer_file,
        &config_file,
        &special_tokens_map_file,
        &tokenizer_config_file,
    ];
    if required.iter().any(|path| !path.exists()) {
        return Ok(None);
    }

    let tokenizer_files = TokenizerFiles {
        tokenizer_file: std::fs::read(tokenizer_file)?,
        config_file: std::fs::read(config_file)?,
        special_tokens_map_file: std::fs::read(special_tokens_map_file)?,
        tokenizer_config_file: std::fs::read(tokenizer_config_file)?,
    };
    let pooling = TextEmbedding::get_default_pooling_method(&EmbeddingModel::BGESmallENV15)
        .ok_or_else(|| other("fastembed has no default pooling for BGESmallENV15"))?;
    let model = UserDefinedEmbeddingModel::new(std::fs::read(onnx_path)?, tokenizer_files)
        .with_pooling(pooling);

    TextEmbedding::try_new_from_user_defined(model, InitOptionsUserDefined::default())
        .map(Some)
        .map_err(|error| {
            other(format!(
                "failed to initialize bundled fastembed model: {error}"
            ))
        })
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
