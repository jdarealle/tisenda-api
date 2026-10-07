use super::{Answer, AnswerRequest, context, generation};
use crate::{Config, documents::Chunk, qdrant, tei::TeiModel};
use anyhow::{Context, Result, bail};
use rig::vector_store::{VectorStoreIndex, request::VectorSearchRequest};
use rig_qdrant::QdrantFilter;

#[tracing::instrument(skip_all, name = "query", fields(query_id = %uuid::Uuid::new_v4()))]
pub async fn answer(config: &Config, request: AnswerRequest) -> Result<Answer> {
    let started = std::time::Instant::now();
    let result = answer_inner(config, request).await;
    tracing::info!(
        event = "query_completed",
        duration_ms = started.elapsed().as_secs_f64() * 1000.0,
        outcome = if result.is_ok() { "success" } else { "failed" },
        sources = result.as_ref().ok().map(|answer| answer.sources.len()),
        no_evidence = result.as_ref().ok().map(|answer| answer.sources.is_empty()),
    );
    result
}

async fn answer_inner(config: &Config, request: AnswerRequest) -> Result<Answer> {
    let question = request.question.trim();
    if question.is_empty() {
        bail!("La pregunta no puede estar vacía");
    }
    let top_k = config.top_k;
    if top_k == 0 || top_k > 50 {
        bail!("TOP_K debe estar entre 1 y 50");
    }
    let client = qdrant::client(config)?;
    let active = qdrant::active_collection(&client, &config.qdrant_alias)
        .await?
        .context("No hay índice activo. Ingiere documentos con POST /ingestions primero")?;
    let binding = qdrant::collection_binding(&client, config, &active).await?;
    let model = TeiModel::new(config)?;
    let identity = model
        .check_model()
        .await
        .context("TEI no está listo para la consulta")?;
    binding.validate_model(config, &identity)?;
    binding.validate_source(config)?;
    let store = qdrant::store(client, model, &config.qdrant_alias);
    let search = VectorSearchRequest::<QdrantFilter>::builder()
        .query(question)
        .samples((top_k * 2).min(50) as u64)
        .build();
    let results: Vec<(f64, String, Chunk)> = crate::logging::operation("qdrant", "search", async {
        store
            .top_n(search)
            .await
            .context("Falló la búsqueda en Qdrant")
    })
    .await?;
    tracing::debug!(
        event = "retrieval_completed",
        candidates = results.len(),
        top_k
    );
    generation::generate(config, question, context::prepare(results, top_k)).await
}
