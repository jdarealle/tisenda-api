# RAG con Docling

API HTTP y biblioteca Rust para ingerir documentos desde una carpeta y responder preguntas con referencias a los archivos originales. Docling convierte y fragmenta; TEI valida tokens y genera embeddings; Qdrant almacena el índice; OpenAI genera las respuestas.

## Arquitectura

```mermaid
flowchart LR
    Cliente -->|POST /ingestions: selección JSON|API[API Rust / Axum]
    Origen[DOCUMENTS_ROOT: originales] -->|Lectura y validación|API
    API -->|Conversión y chunks|Docling[Docling Serve]
    Docling -->|JSON, chunks y recursos|API
    API -->|Tokenize y Embed por gRPC|TEI
    API -->|Contenido, vectores y referencias|Qdrant
    Cliente -->|POST /query|API
    API -->|Generación de respuestas|OpenAI
```

La API se ejecuta en el host. [compose.yaml](compose.yaml) define Docling Serve 1.36.0, TEI 1.9.4 y Qdrant 1.19.1. Qdrant y la caché del modelo TEI utilizan volúmenes de Podman.

## Configuración y ejecución

Requisitos: Linux, Rust/Cargo con soporte para edición 2024, `protoc` y Podman con un proveedor de Compose. El Compose incluye el dispositivo NVIDIA `nvidia.com/gpu=all`; requiere que esté disponible mediante CDI.

Crear la configuración local y ajustar sus valores:

```bash
cp .env.example .env
```

Las variables están documentadas en [.env.example](.env.example). La API carga `.env` al iniciar y respeta las variables ya definidas en el proceso. Los cambios de configuración requieren reiniciar el componente correspondiente.

| Variable | Uso |
|---|---|
| `HTTP_BIND` | Dirección de escucha de la API; predeterminado `0.0.0.0:3000`. |
| `DOCUMENTS_ROOT` | Carpeta de originales; predeterminado `manuales`. Debe existir y ser legible al iniciar. |
| `QDRANT_URL`, `QDRANT_ALIAS` | Endpoint gRPC y alias del índice. |
| `TEI_URL` | Endpoint gRPC de embeddings. |
| `EMBEDDING_MODEL`, `EMBEDDING_REVISION`, `EMBEDDING_DIMENSION` | Identidad del modelo, compatible con el servicio TEI y el índice. |
| `DOCLING_URL` | Endpoint HTTP de Docling Serve. |
| `OPENAI_MODEL`, `OPENAI_API_KEY` | Modelo y credenciales para generar respuestas. |
| `TOP_K` | Máximo de fragmentos enviados al modelo, entre `1` y `50`; predeterminado `5`. |
| `RUST_LOG` | Filtro de logs; predeterminado `rag=info`. |

Las rutas relativas de `DOCUMENTS_ROOT` se resuelven desde el directorio de ejecución.

```bash
podman compose up -d
podman compose logs -f docling tei
```

Iniciar la API cuando los servicios estén disponibles:

```bash
curl --fail-with-body http://127.0.0.1:5001/ready
cargo run --locked
```

| Servicio | Interfaz local |
|---|---|
| API | `http://127.0.0.1:3000/health` |
| Docling | `http://127.0.0.1:5001/ui` |
| Qdrant | `http://127.0.0.1:6333/dashboard` |

## API HTTP

Las solicitudes utilizan JSON y admiten cuerpos de hasta 256 KiB.

| Método | Ruta | Función |
|---|---|---|
| `GET` | `/health` | Devuelve `{"status":"ok"}`; comprueba que la API responde. |
| `POST` | `/ingestions` | Procesa una selección de originales y devuelve el resultado al terminar. |
| `POST` | `/query` | Responde una pregunta con fuentes del índice. |

### Ingesta

Procesar toda la raíz, incluyendo subcarpetas:

```bash
curl --fail-with-body http://127.0.0.1:3000/ingestions \
  -H 'Content-Type: application/json' \
  -d '{}'
```

Seleccionar archivos o carpetas mediante rutas relativas:

```bash
curl --fail-with-body http://127.0.0.1:3000/ingestions \
  -H 'Content-Type: application/json' \
  -d '{"paths":["consumibles-impresoras.pdf","catalogo-software-ti.xlsx"]}'
```

`paths` omitido o vacío selecciona toda la raíz. La selección se ordena y deduplica. Se excluyen entradas ocultas de la exploración; se rechazan rutas explícitas ocultas, absolutas, con `..` o con enlaces simbólicos.

La API procesa un archivo a la vez y admite un único lote activo por proceso. Los clientes y proxies deben permitir conexiones largas. Los rechazos y fallos individuales permiten continuar con los demás archivos; las conversiones parciales con chunks válidos se indexan con advertencias.

La respuesta contiene:

- `counts`: `total`, `completed`, `completed_with_warnings`, `rejected` y `failed`. Los contadores de estado son excluyentes.
- `documents`: resultados individuales con `filename`, `source_key`, `status`, `stage`, `original_sha256`, `task_ids`, `chunks`, `warnings`, `error` y `profile`.

`error` es `null` o un objeto con `code` y `message`. Los códigos por archivo son `unsupported_extension` y `processing_failed`. Las etapas son `validation`, `snapshot`, `conversion`, `download`, `tokens`, `rechunk`, `index` y `done`. `chunks` informa los fragmentos preparados; un estado `completed` o `completed_with_warnings` confirma la indexación.

| Estado HTTP | Significado en `/ingestions` |
|---|---|
| `200` | Lote terminado; revisar los resultados individuales. Una selección vacía devuelve contadores en cero. |
| `400` | JSON o selección inválidos. |
| `409` | El esquema del índice es incompatible con la ingesta. |
| `502` | Falló la comprobación del índice en Qdrant. |
| `503` | Ya existe un lote activo. |

### Formatos admitidos

La API valida la extensión antes de crear temporales o enviar el archivo a Docling. La comparación no distingue mayúsculas. Docling valida posteriormente el contenido.

| Formato | Extensiones |
|---|---|
| PDF | `.pdf` |
| Word OOXML | `.docx`, `.dotx`, `.docm`, `.dotm` |
| PowerPoint OOXML | `.pptx`, `.potx`, `.ppsx`, `.pptm`, `.potm`, `.ppsm` |
| Excel OOXML | `.xlsx`, `.xlsm`, `.xltx`, `.xltm` |
| HTML | `.html`, `.htm`, `.xhtml` |
| Markdown y texto | `.md`, `.txt`, `.text`, `.qmd`, `.rmd` |
| CSV | `.csv` |
| AsciiDoc | `.adoc`, `.asciidoc`, `.asc` |
| Imágenes | `.jpg`, `.jpeg`, `.png`, `.tif`, `.tiff`, `.bmp`, `.webp` |
| Subtítulos | `.vtt` |

Los archivos sin extensión o fuera del catálogo reciben `status="rejected"`, `stage="validation"` y `error.code="unsupported_extension"`.

### Consultas

```bash
curl --fail-with-body http://127.0.0.1:3000/query \
  -H 'Content-Type: application/json' \
  -d '{"question":"¿Qué consumible utiliza la impresora de recepción?"}'
```

La solicitud admite únicamente `question`, que debe contener texto.

La selección utiliza exclusivamente `Config.top_k`, cargado desde `TOP_K` en el servidor: predeterminado `5`, rango `1`–`50`. La búsqueda solicita hasta `min(TOP_K × 2, 50)` candidatos y selecciona hasta `TOP_K` fragmentos mediante los filtros y el límite de contexto. Este valor no garantiza una cantidad de documentos distintos ni de citas; la respuesta puede citar menos fragmentos. La respuesta contiene `text` y `sources`:

```json
{
  "text": "Apaga la impresora antes de sustituir el cartucho. [1]",
  "sources": [
    {
      "id": "1",
      "filename": "manual-impresora.pdf",
      "source_key": "impresoras/manual-impresora.pdf",
      "location": {
        "kind": "page",
        "page_numbers": [12],
        "headings": ["Mantenimiento"]
      },
      "excerpt": "Apague la impresora antes de sustituir el cartucho."
    }
  ]
}
```

Los marcadores `[n]` vinculan afirmaciones del texto con `sources[].id`. Cada fuente representa un fragmento; un documento puede tener varias referencias. Los identificadores son cadenas numéricas locales a la respuesta. `sources` contiene únicamente fragmentos citados, sin duplicados del mismo identificador y en orden de primera aparición. Los identificadores conservan su numeración original: una respuesta que cite `[3]` y `[1]` tendrá fuentes con esos identificadores en ese orden. Los consumidores deben resolver por `id`, no por posición en el arreglo.

`filename` es el nombre visible; `source_key` identifica la ruta relativa del original. `excerpt` es el texto exacto del fragmento indexado enviado al modelo, incluido su contexto estructural; no es una cita textual seleccionada o reformulada por el modelo.

`location.kind` describe la categoría del formato: `page`, `slide`, `sheet`, `section`, `image` o `document`. No garantiza una ubicación precisa. `page_numbers` se incluye únicamente para PDF con páginas conocidas; `headings`, cuando hay encabezados. Ambos campos se omiten si están ausentes o vacíos. No se inventan números de diapositiva, nombres de hoja ni enlaces al original.

La API valida que una respuesta informativa tenga al menos una cita y que todos los marcadores correspondan a identificadores del contexto. Los corchetes se reservan para citas `[n]`, con enteros positivos sin espacios ni ceros iniciales; varias fuentes se escriben `[1][2]`. Ante una respuesta vacía, citas ausentes, marcadores inválidos o identificadores desconocidos, solicita una única corrección con la misma pregunta y contexto. Si falla nuevamente, devuelve HTTP `502` con `{"error":"No se pudo generar una respuesta con referencias válidas"}`. Los fallos del proveedor no se reintentan y devuelven el error genérico `{"error":"No se pudo generar la respuesta"}`.

La validación comprueba sintaxis y existencia de referencias; no evalúa si cada afirmación está respaldada ni detecta todas las afirmaciones sin cita. Los logs registran el motivo de validación y el resultado de la corrección sin incluir el contenido documental.

Cuando no hay fragmentos seleccionados, la API responde sin llamar al generador. Si el modelo determina que el contexto no basta, debe devolver la misma frase de insuficiencia, sin citas. En ambos casos la respuesta es `{"text":"No hay información suficiente en los documentos indexados para responder esa pregunta.","sources":[]}`.

La biblioteca expone `AnswerRequest` para la pregunta y `Answer`, `Source` y `SourceLocation` para la respuesta y sus referencias.

## Conversión y límites

Docling es el único responsable de crear los chunks. La API solicita JSON estructurado y chunks con texto contextualizado y texto original, tablas en modo `accurate` y jerarquía de encabezados PDF.

| Variable | Predeterminado | Comportamiento |
|---|---|---|
| `DOCLING_IMAGE_EXPORT_MODE` | `referenced` | `referenced`: recursos del ZIP; `embedded`: imágenes dentro del JSON; `placeholder`: estructura sin datos de imagen. |
| `DOCLING_DO_OCR` | `true` | Habilita o deshabilita OCR, independientemente del modo de imágenes. Cuando está habilitado, la conversión de archivos de imagen fuerza OCR de página completa. |
| `DOCLING_OCR_PRESET` | `auto` | Preset enviado cuando OCR está habilitado; debe estar disponible en Docling. |
| `CHUNK_TARGET_TOKENS` | `512` | Objetivo solicitado a HybridChunker. |

La conversión mantiene deshabilitadas la descripción de imágenes, clasificación visual y extracción de gráficos mediante modelos.

Para originales JPG, JPEG, PNG, TIFF, BMP y WebP, la API envía `force_ocr=true` cuando `DOCLING_DO_OCR=true`. El perfil de cada documento registra el valor efectivo. El OCR permite extraer texto de una imagen; la fotografía sin texto no se convierte en una descripción visual. Si Docling devuelve cero chunks, el documento falla antes de generar embeddings o escribir en Qdrant.

La API consulta la capacidad real de TEI y cuenta el texto final con su tokenizador, incluyendo tokens especiales. Los embeddings se generan con `truncate=false`. Si un chunk excede la capacidad, Docling vuelve a fragmentar el JSON temporal con la mitad del presupuesto: hasta tres ajustes, deteniéndose si repite el resultado incompatible. Un exceso no resuelto falla el documento.

| Variable | Predeterminado | Límite |
|---|---|---|
| `DOCLING_SERVE_MAX_FILE_SIZE` | 50 MiB | Tamaño del original. |
| `DOCLING_SERVE_MAX_NUM_PAGES` | 200 | Páginas por documento en Docling. |
| `DOCLING_SERVE_MAX_SOURCES_PER_REQUEST` | 20 | Fuentes por solicitud a Docling; la API envía un archivo por tarea. |
| `MAX_FILE_BYTES` | 10 MiB | JSONL de chunks y representación normalizada para indexación. |
| `DOCLING_MAX_DOWNLOAD_BYTES` | 32 MiB | ZIP comprimido por tarea. |
| `DOCLING_MAX_EXPANDED_BYTES` | 128 MiB | Recursos descomprimidos del ZIP. |
| `DOCLING_MAX_ARCHIVE_ENTRIES` | 200 | Entradas del ZIP. |
| `DOCLING_REQUEST_TIMEOUT_SECS` | 120 | Tiempo por solicitud HTTP a Docling. |
| `DOCLING_CONVERSION_TIMEOUT_SECS` | 1800 | Espera de finalización de cada tarea de Docling. |

## Almacenamiento e identidad

`DOCUMENTS_ROOT` conserva los originales. La API utiliza una copia temporal por documento y elimina los temporales al finalizar o ante errores controlados. Qdrant conserva contenido, embeddings, hash del original, perfil y procedencia. Los ZIP y recursos exportados se procesan en memoria dentro de los límites configurados.

`filename` es el nombre visible (`manual.pdf`); `source_key` es la ruta relativa que identifica el documento (`impresoras/manual.pdf`). Archivos con igual nombre en distintas subcarpetas tienen identidades diferentes. Una ingesta actualiza únicamente los documentos seleccionados; los documentos ausentes permanecen en el índice. Renombrar la ruta crea otra identidad.

Los resultados del lote existen durante la solicitud. Los errores de ejecución se informan sin reintentos automáticos. Un fallo de escritura en Qdrant puede dejar puntos parciales; la actualización documental no es transaccional. Un cierre abrupto puede dejar residuos temporales. El bloqueo de escritura coordina procesos del mismo host.
