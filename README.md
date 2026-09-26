# RAG local

CLI en Rust para indexar archivos UTF-8 `.txt` y `.md` de una carpeta local, buscar fragmentos en Qdrant y responder preguntas con OpenAI. TEI genera embeddings densos con `BAAI/bge-m3`.

## Configuración

Si aún no tienes `.env`, créalo desde la plantilla:

```bash
cp .env.example .env
```

Configura `OPENAI_API_KEY` antes de usar `ask`. `SOURCE_DIR` indica la carpeta de ingesta; la plantilla usa `./manuales`. Verifica que `EMBEDDING_MODEL=BAAI/bge-m3` y `EMBEDDING_DIMENSION=1024` coincidan con el modelo de TEI definido en `compose.yaml`. Las variables del entorno tienen prioridad sobre `.env`.

## Arranque

```bash
podman compose up -d
```

TEI descarga el modelo en el primer arranque. Espera unos minutos la descarga y despues a que el endpoint responda:

```bash
curl -fsS http://127.0.0.1:8080/health
```

## Uso

```bash
cargo run -p rag -- ingest
cargo run -p rag -- ask "¿Cómo solicito soporte?"
```

`ingest` lee `SOURCE_DIR`; `ingest --source otra/carpeta` usa otra ruta en esa ejecución. `ask --top-k 8 "pregunta"` cambia el número máximo de fragmentos recuperados.

Cada ingesta crea una colección nueva y activa el alias `rag_active` al terminar. Las colecciones anteriores permanecen en Qdrant.

## Servicios locales

| Servicio | Dirección |
| --- | --- |
| Qdrant Dashboard | [http://127.0.0.1:6333/dashboard](http://127.0.0.1:6333/dashboard) |
| Qdrant HTTP | `http://127.0.0.1:6333` |
| Qdrant gRPC | `http://127.0.0.1:6334` |
| TEI | `http://127.0.0.1:8080` |

La configuración completa está en `.env.example`.
