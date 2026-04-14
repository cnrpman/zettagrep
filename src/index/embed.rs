use std::fs::OpenOptions;
use std::io::Write;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};
#[cfg(test)]
use std::sync::{Mutex, OnceLock as TestOnceLock};
#[cfg(test)]
use std::thread::ThreadId;

#[cfg(not(test))]
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use super::types::VECTOR_DIMENSIONS;
use crate::{ZgResult, other};

#[cfg(not(test))]
static TEXT_EMBEDDER: OnceLock<Mutex<TextEmbedding>> = OnceLock::new();
const DEFAULT_FASTEMBED_BATCH_SIZE: usize = 64;
#[cfg(test)]
static EMBED_CALLS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static EMBED_TEXTS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static EMBED_CAPTURE_THREAD: TestOnceLock<Mutex<Option<ThreadId>>> = TestOnceLock::new();

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

    #[cfg(test)]
    {
        let current = std::thread::current().id();
        if embed_capture_thread()
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().cloned())
            .is_some_and(|thread_id| thread_id == current)
        {
            EMBED_CALLS.fetch_add(1, Ordering::Relaxed);
            EMBED_TEXTS.fetch_add(texts.len(), Ordering::Relaxed);
        }
    }

    record_test_embed_invocation(prefix, texts.len())?;
    maybe_delay_test_passage_embed(prefix)?;

    let prefixed = texts
        .iter()
        .map(|text| format!("{prefix}: {text}"))
        .collect::<Vec<_>>();

    if force_deterministic_embeddings() {
        Ok(prefixed
            .iter()
            .map(|text| deterministic_embedding(text))
            .collect())
    } else {
        #[cfg(not(test))]
        let model = embedder()?;
        #[cfg(not(test))]
        let mut guard = model
            .lock()
            .map_err(|_| other("fastembed model lock poisoned"))?;
        #[cfg(not(test))]
        let embeddings = guard.embed(prefixed, Some(configured_fastembed_batch_size()))?;

        #[cfg(not(test))]
        {
            Ok(embeddings)
        }
        #[cfg(test)]
        {
            unreachable!("test builds always use deterministic embeddings")
        }
    }
}

fn force_deterministic_embeddings() -> bool {
    // Keeps CLI integration tests hermetic without touching real model downloads.
    cfg!(test) || std::env::var_os("ZG_TEST_FAKE_EMBEDDINGS").is_some()
}

fn record_test_embed_invocation(prefix: &str, text_count: usize) -> ZgResult<()> {
    let Some(path) = std::env::var_os("ZG_TEST_EMBED_LOG_PATH") else {
        return Ok(());
    };

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            other(format!(
                "failed to open ZG_TEST_EMBED_LOG_PATH {}: {error}",
                std::path::Path::new(&path).display()
            ))
        })?;
    writeln!(file, "{prefix}\t{text_count}").map_err(|error| {
        other(format!(
            "failed to append ZG_TEST_EMBED_LOG_PATH {}: {error}",
            std::path::Path::new(&path).display()
        ))
    })?;
    Ok(())
}

fn maybe_delay_test_passage_embed(prefix: &str) -> ZgResult<()> {
    if prefix != "passage" {
        return Ok(());
    }

    let Some(raw_delay_ms) = std::env::var_os("ZG_TEST_PASSAGE_EMBED_DELAY_MS") else {
        return Ok(());
    };
    let raw_delay_ms = raw_delay_ms
        .into_string()
        .map_err(|_| other("ZG_TEST_PASSAGE_EMBED_DELAY_MS must be valid utf-8"))?;
    let delay_ms = raw_delay_ms
        .parse::<u64>()
        .map_err(|_| other("ZG_TEST_PASSAGE_EMBED_DELAY_MS must be a non-negative integer"))?;
    if delay_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
    }
    Ok(())
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
    // fastembed's public TextInitOptions exposes model/cache/provider knobs, but not
    // ONNX Runtime intra-thread control; try_new currently chooses available_parallelism().
    TextEmbedding::try_new(TextInitOptions::new(
        EmbeddingModel::ParaphraseMLMiniLML12V2Q,
    ))
}

#[cfg(not(test))]
fn fastembed_env_summary() -> String {
    let keys = [
        "ZG_FASTEMBED_BATCH_SIZE",
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

fn deterministic_embedding(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0; VECTOR_DIMENSIONS];
    for (index, byte) in text.bytes().enumerate() {
        let slot = index % VECTOR_DIMENSIONS;
        vector[slot] += f32::from(byte % 31) + 1.0;
    }
    vector
}

fn configured_fastembed_batch_size() -> usize {
    std::env::var("ZG_FASTEMBED_BATCH_SIZE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_FASTEMBED_BATCH_SIZE)
}

#[cfg(test)]
pub(crate) fn test_reset_embed_counters() {
    EMBED_CALLS.store(0, Ordering::Relaxed);
    EMBED_TEXTS.store(0, Ordering::Relaxed);
    if let Ok(mut guard) = embed_capture_thread().lock() {
        *guard = None;
    }
}

#[cfg(test)]
pub(crate) fn test_begin_embed_capture_for_current_thread() {
    test_reset_embed_counters();
    if let Ok(mut guard) = embed_capture_thread().lock() {
        *guard = Some(std::thread::current().id());
    }
}

#[cfg(test)]
pub(crate) fn test_embed_counters() -> (usize, usize) {
    (
        EMBED_CALLS.load(Ordering::Relaxed),
        EMBED_TEXTS.load(Ordering::Relaxed),
    )
}

#[cfg(test)]
fn embed_capture_thread() -> &'static Mutex<Option<ThreadId>> {
    EMBED_CAPTURE_THREAD.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_FASTEMBED_BATCH_SIZE, configured_fastembed_batch_size,
        test_begin_embed_capture_for_current_thread, test_embed_counters,
        test_reset_embed_counters,
    };

    #[test]
    fn batch_size_defaults_to_fastembed_default() {
        unsafe {
            std::env::remove_var("ZG_FASTEMBED_BATCH_SIZE");
        }
        assert_eq!(
            configured_fastembed_batch_size(),
            DEFAULT_FASTEMBED_BATCH_SIZE
        );
    }

    #[test]
    fn batch_size_can_be_overridden_by_env() {
        unsafe {
            std::env::set_var("ZG_FASTEMBED_BATCH_SIZE", "64");
        }
        assert_eq!(configured_fastembed_batch_size(), 64);
        unsafe {
            std::env::remove_var("ZG_FASTEMBED_BATCH_SIZE");
        }
    }

    #[test]
    fn test_counters_reset_cleanly() {
        test_reset_embed_counters();
        assert_eq!(test_embed_counters(), (0, 0));
    }

    #[test]
    fn capture_is_thread_scoped() {
        test_begin_embed_capture_for_current_thread();
        assert_eq!(test_embed_counters(), (0, 0));
        test_reset_embed_counters();
    }
}
