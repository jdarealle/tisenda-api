# Query

## Purpose

Responder preguntas en español con fragmentos recuperados del índice activo y citas que remiten a los documentos originales.

Non-goals: verificar que cada afirmación se deduzca de su fragmento, medir la calidad de recuperación, filtrar fuentes según permisos del usuario o permitir que el cliente elija `TOP_K`.

## Requirements

### Requirement: qry.pregunta-valida — Validar la solicitud

The system SHALL aceptar en `POST /query` un único campo `question`, recortar sus espacios iniciales y finales y rechazar con `400` un cuerpo JSON inválido, con campos desconocidos o mayor que 256 KiB (código `invalid_query_json`), o una pregunta vacía o compuesta solo de espacios (código `empty_question`).

#### Scenario: pregunta vacía

- **WHEN** el cliente envía `POST /query` con `{"question": "   "}`
- **THEN** la API responde `400` con código `empty_question`

#### Scenario: campo desconocido

- **WHEN** el cliente envía `{"question": "¿Cómo cambio el tóner?", "top_k": 10}`
- **THEN** la API responde `400` con código `invalid_query_json`

### Requirement: qry.indice-requerido — Exigir un índice activo y compatible

The system SHALL responder `503` con código `index_unavailable` cuando no existe un alias Qdrant activo y `502` con código `qdrant_unavailable` cuando no puede consultar los alias. Con un índice activo, responde `502` con código `answer_failed` si TEI no está disponible o si el modelo, su revisión, la configuración de embeddings, el alias o el esquema no coinciden con los metadatos de la colección.

#### Scenario: antes de la primera ingesta

- **GIVEN** ningún documento indexado todavía
- **WHEN** el cliente envía `POST /query` con una pregunta
- **THEN** la API responde `503` con código `index_unavailable`

#### Scenario: TEI sirve otra revisión del modelo

- **GIVEN** un índice creado con una revisión del modelo y TEI sirviendo otra
- **WHEN** el cliente envía una pregunta
- **THEN** la API responde `502` con código `answer_failed`

### Requirement: qry.seleccion-evidencia — Seleccionar la evidencia

The system SHALL solicitar a Qdrant hasta `min(2 × TOP_K, 50)` candidatos y recorrerlos por similitud coseno descendente, deteniéndose en el primero con similitud < 0,25 o al reunir `TOP_K` fragmentos (1–50, 5 por defecto). Admite hasta tres fragmentos por `document_id` y hasta 12 000 bytes UTF-8 de contexto. Un fragmento que no cabe en el presupuesto restante se omite completo y se prueba el siguiente.

#### Scenario: un documento domina los resultados

- **GIVEN** `TOP_K=5` y seis fragmentos relevantes del mismo documento
- **WHEN** el cliente envía una pregunta
- **THEN** el contexto incluye como máximo tres fragmentos de ese documento

#### Scenario: fragmento demasiado largo para el presupuesto restante

- **GIVEN** un contexto con 11 000 bytes ocupados y un candidato de 2 000 bytes seguido de otro de 500
- **WHEN** se construye el contexto
- **THEN** se omite el candidato de 2 000 bytes y se incluye el de 500

### Requirement: qry.sin-evidencia — Responder sin evidencia

The system SHALL devolver `200` con el texto "No hay información suficiente en los documentos indexados para responder esa pregunta." y `sources: []` cuando ningún fragmento cumple los criterios de selección, sin llamar a OpenAI ni requerir `OPENAI_API_KEY`. También devuelve esa respuesta cuando el modelo contesta exactamente esa frase.

#### Scenario: pregunta fuera del corpus

- **GIVEN** un índice sin fragmentos con similitud ≥ 0,25 para la pregunta
- **WHEN** el cliente envía `POST /query`
- **THEN** la respuesta contiene el mensaje de información insuficiente y `sources: []`
- **AND** no se envía ninguna solicitud a OpenAI

#### Scenario: el modelo considera insuficiente el contexto

- **GIVEN** evidencia seleccionada que no responde la pregunta
- **WHEN** OpenAI contesta exactamente la frase de información insuficiente
- **THEN** la respuesta contiene esa frase y `sources: []`

### Requirement: qry.citas — Citar las fuentes

The system SHALL generar una respuesta en español con marcadores `[n]` que se resuelven por `sources[].id`, y devolver solo las fuentes citadas, sin duplicados y en orden de primera aparición. Cada fuente incluye `id`, `document_id`, `filename`, `source_key`, `excerpt` (el texto indexado enviado al modelo) y `location`. `location.kind` depende de la extensión: `page` (PDF), `slide`, `sheet`, `section`, `image` o `document`. `page_numbers` se informa solo en PDF y `headings` solo cuando hay encabezados.

#### Scenario: respuesta con dos fuentes

- **GIVEN** evidencia de dos documentos
- **WHEN** el cliente envía una pregunta que ambos responden
- **THEN** la respuesta contiene marcadores `[1]` y `[2]`
- **AND** `sources` contiene exactamente esas dos fuentes, con su `document_id`

#### Scenario: fuente de una presentación

- **GIVEN** un fragmento citado de `capacitacion.pptx`
- **WHEN** se construye la respuesta
- **THEN** su `location.kind` es `slide` y no incluye `page_numbers`

### Requirement: qry.correccion-citas — Corregir citas inválidas una vez

The system SHALL considerar inválida una respuesta vacía, una respuesta informativa sin ninguna cita, un corchete que no forme un marcador `[n]` (entero positivo sin ceros iniciales ni espacios, o `]` sin `[` previo) o una cita a un identificador inexistente. En ese caso solicita una única corrección con el mismo contexto y responde `502` con código `invalid_citations` si la corrección también es inválida.

#### Scenario: segunda respuesta con citas inválidas

- **GIVEN** OpenAI devuelve dos veces una respuesta que cita `[7]` con solo tres fuentes disponibles
- **WHEN** el cliente envía la pregunta
- **THEN** la API responde `502` con código `invalid_citations`

#### Scenario: corrección válida

- **GIVEN** una primera respuesta con el marcador `[01]` y una corrección con `[1]`
- **WHEN** el cliente envía la pregunta
- **THEN** la API responde `200` con la respuesta corregida

### Requirement: qry.credencial-openai — Exigir la credencial al generar

The system SHALL responder `502` con código `answer_failed` cuando hay evidencia seleccionada y `OPENAI_API_KEY` falta, está vacía o vale `CHANGE_ME`. También responde `502 answer_failed` si falla la llamada a OpenAI.

#### Scenario: clave sin configurar

- **GIVEN** `OPENAI_API_KEY=CHANGE_ME` y evidencia relevante en el índice
- **WHEN** el cliente envía una pregunta
- **THEN** la API responde `502` con código `answer_failed`

## Edge Cases

- **Categorías sin coordenadas** (`qry.citas`): fuentes como `slide` o `sheet` no garantizan página ni posición.
- **Citas válidas sin respaldo** (`qry.correccion-citas`): el validador comprueba la sintaxis y la existencia de las referencias, no que la afirmación se deduzca del fragmento.
- **Corchetes en el contenido** (`qry.correccion-citas`): los corchetes quedan reservados para citas; una respuesta que los use para otro fin es inválida.
- **`TOP_K` fuera de 1–50** (`qry.seleccion-evidencia`): la API no arranca; no es un error de consulta.
- **Fallo de búsqueda en Qdrant tras encontrar el alias** (`qry.indice-requerido`): la API responde `502` con código `answer_failed`.

## References

- Contrato HTTP: `POST /query` en [OpenAPI](http://127.0.0.1:3000/openapi.json).
- Arquitectura: [flujo de consulta](../ARCHITECTURE.md#consulta).
- Pruebas:
  - `qry.pregunta-valida` → sin prueba automatizada.
  - `qry.indice-requerido` → sin prueba automatizada.
  - `qry.seleccion-evidencia` → sin prueba automatizada.
  - `qry.sin-evidencia` → sin prueba automatizada.
  - `qry.citas` → sin prueba automatizada.
  - `qry.correccion-citas` → sin prueba automatizada.
  - `qry.credencial-openai` → sin prueba automatizada.
