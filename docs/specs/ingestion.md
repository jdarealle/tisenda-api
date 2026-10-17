# Ingestion

## Purpose

Seleccionar originales del corpus del servidor, indexarlos de forma asíncrona en Qdrant y consultar el estado de cada lote. El cliente envía rutas relativas a `DOCUMENTS_ROOT` mediante JSON. La biblioteca expone además `ingest_documents` para indexar documentos ya convertidos.

Non-goals: recibir archivos adjuntos desde el navegador, eliminar o renombrar documentos, deduplicar contenidos o inferir movimientos mediante hashes.

## Requirements

### Requirement: ing.aceptacion-asincrona — Aceptar lotes sin esperar al procesamiento

The system SHALL registrar catálogo, lote y trabajos en una transacción y responder `202 Accepted` con `batch_id`, `total` y `status_url` en el cuerpo y la cabecera `Location`, sin exigir que Docling, TEI o Qdrant estén disponibles. Un cuerpo JSON inválido o con campos desconocidos se rechaza con `400` y código `invalid_selection_json`.

#### Scenario: selección válida con servicios externos detenidos

- **GIVEN** un worker en ejecución y Docling, TEI y Qdrant detenidos
- **WHEN** el cliente envía `POST /ingestions` con rutas válidas
- **THEN** la API responde `202 Accepted`
- **AND** `Location` y `status_url` apuntan a `/ingestions/{batch_id}`

#### Scenario: campo desconocido

- **WHEN** el cliente envía `{"paths": [], "force": true}`
- **THEN** la API responde `400` con código `invalid_selection_json`

### Requirement: ing.seleccion-raiz — Seleccionar toda la raíz, directorios o archivos

The system SHALL interpretar `{}` o `{"paths": []}` como toda la raíz documental, recorrer de forma recursiva los directorios seleccionados omitiendo sus entradas ocultas, normalizar, ordenar y deduplicar las rutas, y fijar la selección en el momento de aceptar el lote.

#### Scenario: toda la raíz

- **WHEN** el cliente envía `POST /ingestions` con `{}`
- **THEN** el lote incluye todos los archivos visibles de `DOCUMENTS_ROOT` y sus subdirectorios

#### Scenario: selección sin archivos

- **GIVEN** un directorio seleccionado que solo contiene entradas ocultas
- **WHEN** el cliente envía la selección
- **THEN** la API responde `202` con `total: 0`
- **AND** el lote figura como `completed` desde su creación

#### Scenario: archivo añadido después de aceptar

- **GIVEN** un lote aceptado
- **WHEN** se añade un archivo nuevo al corpus
- **THEN** ese archivo no forma parte del lote y requiere otra solicitud

### Requirement: ing.rutas-invalidas — Rechazar selecciones inseguras

The system SHALL rechazar la solicitud completa con `400` y código `invalid_selection`, sin crear un lote, cuando una ruta está vacía, es absoluta, contiene `\` o `:`, sale de la raíz, nombra explícitamente una entrada oculta, no existe, no es UTF-8, contiene caracteres de control, es un symlink o incluye una entrada que no es archivo regular ni directorio.

#### Scenario: recorrido fuera de la raíz

- **WHEN** el cliente envía `{"paths": ["../secreto.pdf"]}`
- **THEN** la API responde `400` con código `invalid_selection`
- **AND** no se crea ningún lote

#### Scenario: una ruta inválida entre varias válidas

- **WHEN** el cliente envía `{"paths": ["manual.pdf", ".oculto/notas.md"]}`
- **THEN** la API responde `400` con código `invalid_selection`
- **AND** tampoco se registra `manual.pdf`

### Requirement: ing.rechazo-documento — Rechazar documentos que no se pueden procesar

The system SHALL finalizar con estado `rejected`, sin reintentos, el trabajo cuyo archivo tiene una extensión no admitida (código `unsupported_extension`) o cuyo original no es válido al copiarlo (código `invalid_source`): vacío, mayor que `DOCLING_SERVE_MAX_FILE_SIZE`, que crece por encima de ese límite o queda vacío durante la copia, o que ya no se puede abrir sin seguir symlinks. Las extensiones admitidas están en [formats.rs](../../src/docling/formats.rs).

#### Scenario: extensión desconocida

- **GIVEN** un archivo `notas.xyz` en el corpus
- **WHEN** se incluye en un lote y el worker procesa su trabajo
- **THEN** el trabajo termina `rejected` con código de error `unsupported_extension`

#### Scenario: original vacío

- **GIVEN** un archivo `vacio.pdf` de 0 bytes
- **WHEN** el worker procesa su trabajo
- **THEN** el trabajo termina `rejected` con código de error `invalid_source`

### Requirement: ing.identidad-estable — Conservar la identidad por ruta relativa

The system SHALL asignar un `document_id` (UUIDv7) por `source_key` al aceptar el lote y reutilizarlo en cada ingesta posterior de la misma ruta relativa, aunque falle el procesamiento.

#### Scenario: reingerir la misma ruta

- **GIVEN** `manuales/impresora.pdf` ya ingerido con un `document_id`
- **WHEN** se vuelve a ingerir, con los mismos bytes o con bytes modificados
- **THEN** el informe conserva el mismo `document_id`

#### Scenario: los mismos bytes en otra ruta

- **GIVEN** `manuales/impresora.pdf` ya ingerido
- **WHEN** se ingiere una copia en `archivo/impresora.pdf`
- **THEN** se registra otro `document_id`
- **AND** el documento anterior permanece en el catálogo y en el índice

### Requirement: ing.sin-cambios — Omitir documentos ya indexados sin cambios

The system SHALL finalizar con `result: "unchanged"`, sin convertir el documento, solo si coinciden en todos sus puntos el hash del original, el perfil de conversión, el pipeline, el modelo y su revisión, la cantidad de fragmentos, sus posiciones y sus IDs deterministas. Cambiar el OCR, su preset, el modo de exportación de imágenes, el presupuesto de tokens o el modelo obliga a reprocesar. Si faltan metadatos, procesa el documento; si la consulta a Qdrant falla, no la interpreta como ausencia.

#### Scenario: repetir la ingesta sin cambios

- **GIVEN** un documento indexado correctamente
- **WHEN** se ingiere de nuevo sin modificar sus bytes ni la configuración de procesamiento
- **THEN** se crea otro lote y otro trabajo
- **AND** el trabajo termina con `result: "unchanged"` y el número de puntos del documento no cambia

#### Scenario: cambio del presupuesto de fragmentación

- **GIVEN** un documento indexado con `CHUNK_TARGET_TOKENS=512`
- **WHEN** se ingiere de nuevo con `CHUNK_TARGET_TOKENS=256`
- **THEN** el documento se convierte de nuevo y termina con `result: "indexed"`

### Requirement: ing.conversion-parcial — Completar con advertencias

The system SHALL finalizar con estado `completed_with_warnings`, y el detalle en `warnings`, cuando Docling informa `partial_success` o cuando no se puede eliminar el directorio temporal del intento.

#### Scenario: conversión parcial

- **GIVEN** un documento que Docling convierte con `partial_success`
- **WHEN** el worker termina de indexarlo
- **THEN** el trabajo termina `completed_with_warnings` con `result: "indexed"`
- **AND** `warnings` contiene el estado devuelto por Docling

### Requirement: ing.documento-sin-contenido — Fallar sin fragmentos utilizables

The system SHALL finalizar con estado `failed` y código `processing_failed` cuando la conversión no produce fragmentos, algún fragmento no contiene texto o Docling informa `failure` o `skipped`.

#### Scenario: imagen sin texto reconocible

- **GIVEN** una imagen de la que Docling no extrae texto
- **WHEN** el worker procesa su trabajo
- **THEN** el trabajo termina `failed` con código de error `processing_failed`

### Requirement: ing.refragmentacion — Ajustar fragmentos al límite de tokens

The system SHALL solicitar hasta tres refragmentaciones, reduciendo cada vez a la mitad el presupuesto de tokens, cuando un fragmento excede el límite que informa TEI. Marca el documento como fallido si tras esas recuperaciones sigue excediéndolo, si Docling repite los mismos fragmentos o si el presupuesto llega a 1.

#### Scenario: fragmento demasiado largo

- **GIVEN** un documento cuyo fragmento supera el límite de tokens de TEI
- **WHEN** el worker valida sus tokens
- **THEN** solicita una refragmentación con la mitad del presupuesto
- **AND** tras tres refragmentaciones sin resolverlo, el trabajo termina `failed`

### Requirement: ing.reintentos — Reintentar errores transitorios

The system SHALL permitir hasta tres intentos por trabajo ante errores transitorios: transporte, HTTP 408, 429 o 5xx, estados gRPC `Unavailable`, `DeadlineExceeded`, `ResourceExhausted`, `Aborted`, `Internal` o `Unknown`, y el vencimiento de `DOCLING_CONVERSION_TIMEOUT_SECS`. Las esperas son de 10 y 60 segundos, y mientras dura cada una el trabajo figura `pending` con `next_attempt_at`.

#### Scenario: servicio no disponible temporalmente

- **GIVEN** Docling responde `503` en el primer intento
- **WHEN** el worker procesa el trabajo
- **THEN** el trabajo vuelve a `pending` con `next_attempt_at` 10 segundos después
- **AND** si el tercer intento falla, termina `failed`

#### Scenario: fallo no transitorio

- **GIVEN** Docling informa `failure` para la tarea
- **WHEN** el worker procesa el trabajo
- **THEN** el trabajo termina `failed` sin reintentos

### Requirement: ing.orden-procesamiento — Procesar un trabajo a la vez

The system SHALL procesar un único trabajo a la vez, tomando el disponible más antiguo entre todos los lotes. Un trabajo en espera de reintento no bloquea a los demás. Se pueden aceptar lotes nuevos mientras se procesa otro.

#### Scenario: dos lotes consecutivos

- **GIVEN** un lote A aceptado antes que un lote B
- **WHEN** el worker procesa la cola
- **THEN** los trabajos de A empiezan antes que los de B

### Requirement: ing.recuperacion-reinicio — Recuperar trabajos interrumpidos

The system SHALL devolver a `pending` los trabajos interrumpidos al reiniciar, registrar su intento abierto como `interrupted` y reconciliar la última versión observada en Qdrant antes de volver a leer el original o declarar agotados los intentos.

#### Scenario: reinicio durante el procesamiento

- **GIVEN** un trabajo en `processing`
- **WHEN** la API se detiene y vuelve a iniciar
- **THEN** el trabajo vuelve a `pending` y se procesa de nuevo, o termina `unchanged` si su indexación ya estaba completa

#### Scenario: cierre ordenado

- **GIVEN** un trabajo en `processing`
- **WHEN** la API recibe SIGINT o SIGTERM
- **THEN** el worker deja de tomar trabajos y concede 30 segundos al activo antes de interrumpirlo

### Requirement: ing.detencion-worker — Cerrar la API si el worker falla

The system SHALL detener el worker ante un error de persistencia SQLite o un pánico, registrar el evento `ingestion_worker_stopped` y cerrar la API de forma ordenada. Mientras se cierra, `POST /ingestions` responde `503` con código `worker_stopped`.

#### Scenario: disco lleno

- **GIVEN** SQLite no puede escribir el progreso de un trabajo
- **WHEN** el worker intenta guardarlo
- **THEN** el worker se detiene y la API termina su ejecución
- **AND** el trabajo queda recuperable en el siguiente arranque

### Requirement: ing.estado-lote — Informar el progreso del lote

The system SHALL informar en `GET /ingestions/{batch_id}` el estado del lote (`pending` si ningún trabajo ha empezado, `processing` o `completed` cuando todos sus trabajos son terminales, incluso con fallos), los contadores de todo el lote y una página de documentos en orden de selección. La página tiene 100 elementos por defecto, `limit` admite 1–500 y `offset` debe ser ≥ 0. Cada documento informa `job_id`, `attempts`, fechas, `next_attempt_at`, `status`, `stage`, `result`, `chunks`, `warnings`, `error`, `task_ids` y `original_sha256`.

#### Scenario: lote con un fallo

- **GIVEN** un lote de tres documentos donde uno falla de forma definitiva
- **WHEN** el cliente consulta `GET /ingestions/{batch_id}` al terminar
- **THEN** el lote figura como `completed`
- **AND** los contadores muestran dos documentos completados y uno `failed`, con la etapa y el error del fallido

#### Scenario: paginación fuera de rango

- **WHEN** el cliente solicita una página con `limit=501`
- **THEN** la API responde `400` con código `invalid_pagination`

#### Scenario: lote inexistente

- **WHEN** el cliente consulta un `batch_id` que no existe
- **THEN** la API responde `404` con código `batch_not_found`

### Requirement: ing.api-biblioteca — Indexar documentos convertidos desde la biblioteca

The system SHALL indexar de forma independiente cada `IngestDocument` recibido por `ingest_documents` y devolver un resultado por documento. Rechaza los documentos con `document_id` nulo o máximo, `source_key` o `document_id` repetidos en la misma llamada, nombre o `source_key` inválidos, o sin fragmentos. No elimina del índice los documentos ausentes de la llamada.

#### Scenario: identificador repetido en la misma llamada

- **GIVEN** dos `IngestDocument` con el mismo `document_id`
- **WHEN** se llama a `ingest_documents`
- **THEN** ambos resultados contienen un error de `document_id` duplicado
- **AND** el índice no cambia para esos documentos

## Edge Cases

- **Archivo modificado durante la copia** (`ing.sin-cambios`): no hay lectura atómica; el original debe permanecer estable mientras se copia.
- **Interrupción entre SQLite y Qdrant** (`ing.recuperacion-reinicio`): pueden quedar versiones mezcladas o puntos sobrantes visibles hasta completar una nueva ejecución.
- **Timeout local de Docling** (`ing.reintentos`): la tarea remota puede seguir ejecutándose; el informe conserva sus `task_ids`.
- **Fallo transitorio al reconciliar** (`ing.recuperacion-reinicio`): se repite diez segundos después, incluso tras el tercer intento, sin iniciar otra conversión.
- **Ruta ya asociada a otro documento en el índice** (`ing.identidad-estable`): la indexación falla con un conflicto de identidad entre el catálogo y Qdrant.
- **Archivo omitido o borrado del corpus** (`ing.identidad-estable`): no se eliminan su registro ni sus puntos.
- **Otra ingesta en paralelo sobre el mismo endpoint y alias de Qdrant** (`ing.orden-procesamiento`): falla por el bloqueo local de escritura.
- **El alias activo cambia durante la indexación** (`ing.sin-cambios`): se cancela la operación sin retirar puntos.
- **TEI cambia de revisión durante la preparación** (`ing.refragmentacion`): el documento falla sin modificar el índice.

Estados del trabajo: `pending`, `processing`, `completed`, `completed_with_warnings`, `rejected` y `failed`. Etapas persistidas: `validation`, `snapshot`, `check_index`, `conversion`, `download`, `tokens`, `rechunk`, `index` y `done`; un error conserva la etapa alcanzada. Códigos de error del informe: `unsupported_extension`, `invalid_source` y `processing_failed`.

## References

- Contrato HTTP: `POST /ingestions` y `GET /ingestions/{batch_id}` en [OpenAPI](http://127.0.0.1:3000/openapi.json).
- Arquitectura: [flujo de ingesta](../ARCHITECTURE.md#ingesta), [arranque y apagado](../ARCHITECTURE.md#arranque-y-apagado) y [catálogo SQLite](../ARCHITECTURE.md#catálogo-sqlite).
- Pruebas:
  - `ing.aceptacion-asincrona` → sin prueba automatizada.
  - `ing.seleccion-raiz` → sin prueba automatizada.
  - `ing.rutas-invalidas` → sin prueba automatizada.
  - `ing.rechazo-documento` → sin prueba automatizada.
  - `ing.identidad-estable` → sin prueba automatizada.
  - `ing.sin-cambios` → sin prueba automatizada.
  - `ing.conversion-parcial` → sin prueba automatizada.
  - `ing.documento-sin-contenido` → sin prueba automatizada.
  - `ing.refragmentacion` → sin prueba automatizada.
  - `ing.reintentos` → sin prueba automatizada.
  - `ing.orden-procesamiento` → sin prueba automatizada.
  - `ing.recuperacion-reinicio` → sin prueba automatizada.
  - `ing.detencion-worker` → sin prueba automatizada.
  - `ing.estado-lote` → sin prueba automatizada.
  - `ing.api-biblioteca` → sin prueba automatizada.
