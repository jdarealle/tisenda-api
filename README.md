# Tisenda API

API HTTP y biblioteca Rust de generación aumentada por recuperación (RAG) para responder consultas dirigidas al área de IT a partir de documentos locales, con referencias a los originales.

La API y su worker se ejecutan en el host. [Compose](compose.yaml) levanta Docling Serve para convertir documentos, TEI para generar embeddings y Qdrant para indexarlos. SQLite conserva el catálogo y la cola; OpenAI genera respuestas con los fragmentos recuperados.

## Funcionalidad

- Ingesta asíncrona de originales seleccionados por ruta dentro de la raíz documental del servidor, con seguimiento del lote, reintentos y detección de documentos sin cambios.
- Formatos admitidos: PDF, Office OOXML, HTML, Markdown, texto, CSV, AsciiDoc, imágenes y VTT. La lista de extensiones está en [formats.rs](src/docling/formats.rs).
- Consultas en español con citas a los documentos originales.
- Contrato HTTP generado desde Rust y publicado con OpenAPI y Scalar.

Limitaciones:

- No recibe archivos desde el navegador: los originales deben estar en la raíz documental del servidor.
- No tiene autenticación ni TLS. Quien alcance la API puede ingerir documentos y consultar su contenido.
- Funciona como una sola instancia con almacenamiento local.

## Requisitos

- Rust 1.95 o superior (`rust-version` en [Cargo.toml](Cargo.toml)).
- Podman con Compose para Docling Serve, TEI y Qdrant. Las imágenes y versiones están en [compose.yaml](compose.yaml); las variables que exige figuran como `[obligatoria: Compose]` en [.env.example](.env.example).
- CDI NVIDIA en el host: el contenedor Docling lo solicita incluso con `DOCLING_DEVICE=cpu`.
- Una clave de API de OpenAI para generar respuestas con evidencia; la ingesta y las consultas sin evidencia no la usan.

Las versiones exactas de las dependencias Rust están en [Cargo.toml](Cargo.toml) y [Cargo.lock](Cargo.lock).

## Inicio rápido

Ejecuta los comandos desde la raíz del repositorio.

1. Copia la configuración de ejemplo si aún no tienes una local, configura `OPENAI_API_KEY` y crea la carpeta de originales. [.env.example](.env.example) usa `corpus`:

   ```bash
   cp .env.example .env
   mkdir -p corpus
   ```

2. Levanta los servicios y espera a que carguen sus modelos:

   ```bash
   podman compose up -d
   podman compose logs -f docling tei
   ```

3. Inicia la API. Crea la base SQLite y sus directorios al arrancar:

   ```bash
   cargo run --locked
   ```

4. Comprueba que responden la API y los servicios:

   ```bash
   curl --fail-with-body http://127.0.0.1:3000/health
   curl --fail-with-body http://127.0.0.1:6333/healthz
   curl --fail-with-body http://127.0.0.1:5001/ready
   ```

## Configuración

Las variables, su obligatoriedad y sus valores por defecto están documentados en [.env.example](.env.example). Los cambios requieren reiniciar el componente afectado.

- Cambiar el modelo, su revisión, el presupuesto de fragmentación o las opciones de OCR e imágenes obliga a reprocesar los documentos al volver a ingerirlos.
- `EMBEDDING_MODEL`, `EMBEDDING_REVISION` y `EMBEDDING_DIMENSION` deben coincidir entre la API, TEI y el índice.
- `CHUNK_MAX_TOKENS` está obsoleto; usa `CHUNK_TARGET_TOKENS`.
- El cuerpo JSON de las solicitudes HTTP admite hasta 256 KiB y no es configurable.

## Uso básico

1. Abre [Scalar](http://127.0.0.1:3000/docs) y ejecuta `POST /ingestions` con `{}` para seleccionar toda la raíz documental.
2. Consulta `GET /ingestions/{batch_id}` hasta que el lote figure como `completed` y revisa los contadores de fallos.
3. Usa `POST /query` con `{"question": "..."}` para preguntar sobre los documentos indexados.

| Recurso | Dirección |
| --- | --- |
| Endpoints, esquemas y ejemplos interactivos | [Scalar](http://127.0.0.1:3000/docs) |
| Especificación generada desde Rust | [OpenAPI](http://127.0.0.1:3000/openapi.json) |
| Interfaz de conversión | [Docling](http://127.0.0.1:5001/ui) |
| Explorador del índice | [Qdrant](http://127.0.0.1:6333/dashboard) |

### Diagnóstico

La API escribe logs a stdout: texto con `rag=debug` en compilaciones de depuración y JSON con `rag=info` en release. `RUST_LOG` cambia el filtro, no el formato. Los eventos se correlacionan con `x-request-id`, `query_id`, `job_id` y `document_id`. TEI usa gRPC en el puerto 8080: revisa sus logs y los errores de `Info`, `Tokenize` y `Embed`.

| Síntoma | Comprobación |
| --- | --- |
| HTTP 503 `index_unavailable` | Completar una primera ingesta y comprobar el alias |
| Lote `completed` con documentos fallidos | Revisar los contadores y la etapa y el error de cada documento |
| La API se cierra con el evento `ingestion_worker_stopped` (o responde `503 worker_stopped`) | Error de persistencia SQLite o pánico del worker: comprobar espacio, permisos y propiedad exclusiva de SQLite y reiniciar; los trabajos interrumpidos se recuperan |
| La API no arranca: «Otra instancia posee esta cola de ingesta» | Detener la instancia propietaria; no eliminar el archivo `.lock` |
| La API no arranca: esquema SQLite anterior | Seguir la [reinicialización](#reinicialización-y-reconstrucción-del-índice) |
| HTTP 502 `answer_failed` o documentos `failed` por modelo, revisión o esquema incompatible | Revisar la configuración de embeddings y [reconstruir el índice](#reinicialización-y-reconstrucción-del-índice) |
| HTTP 502 `answer_failed` con evidencia | Comprobar `OPENAI_API_KEY` y la disponibilidad de TEI y OpenAI |
| HTTP 502 `invalid_citations` | Revisar el evento de validación de citas; el índice existe |

### Reinicialización y reconstrucción del índice

Originales y SQLite son los datos esenciales: Qdrant se puede reconstruir desde ellos conservando los identificadores documentales. No borres SQLite para reconstruir el índice.

**SQLite con un esquema anterior al catálogo actual.** La API no arranca y no migra ni borra datos automáticamente:

1. Detén la API y respalda los originales y el estado anterior.
2. Conserva `DOCUMENTS_ROOT`. Configura un `INGESTIONS_DB_PATH` nuevo y un `QDRANT_ALIAS` nuevo y vacío, por ejemplo `data/catalog-v1.sqlite` y `rag_catalog_v1`.
3. Inicia la API, envía `POST /ingestions` con `{}` y sigue el lote hasta que termine, revisando sus fallos.

Esta transición crea identificadores e historial nuevos; el estado anterior queda intacto en su ubicación.

**Índice Qdrant incompatible o perdido** (esquema distinto de **4**, otro modelo o revisión, colección eliminada):

1. Detén la API y conserva la misma SQLite y los originales.
2. Configura un `QDRANT_ALIAS` nuevo y vacío, y la configuración de modelos adecuada en la API y en TEI.
3. Inicia la API, envía `POST /ingestions` con `{}` y revisa los fallos del lote. Los documentos conservan su `document_id`.
4. Comprueba una consulta representativa y que sus fuentes correspondan a los originales.

Las consultas vuelven a estar disponibles tras el primer documento indexado, aunque falte procesar el resto del corpus.

## Desarrollo

```bash
cargo fmt --all -- --check
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
```

El proyecto aún no tiene pruebas automatizadas. La API pública de la biblioteca se expone en [src/lib.rs](src/lib.rs).

## Documentación técnica

| Documento | Contenido |
| --- | --- |
| [Arquitectura](docs/ARCHITECTURE.md) | Componentes, tecnología, flujos, datos, invariantes, decisiones y límites conocidos |
| [Especificaciones](docs/specs/README.md) | Comportamiento esperado de la [ingesta](docs/specs/ingestion.md) y de las [consultas](docs/specs/query.md) |
| [Migraciones](migrations/) | Esquema SQLite del catálogo y la cola |
