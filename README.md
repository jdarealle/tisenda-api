# RAG local

CLI y biblioteca en Rust para indexar documentos UTF-8 `.txt` y `.md`, consultar vectores en Qdrant y generar respuestas con fuentes mediante OpenAI. Rig gestiona embeddings y búsquedas; TEI ejecuta el modelo de embeddings y `qdrant-client` realiza las escrituras.

## Requisitos

- Rust y Cargo compatibles con la edición 2024 y las dependencias de `Cargo.lock`. El bloqueo de archivos utiliza `File::try_lock`.
- `protoc` en el `PATH`, requerido para generar el cliente gRPC de `proto/tei.proto` durante la compilación.
- Podman y un proveedor de Compose para ejecutar los servicios locales.

## Configuración

Crear `.env` desde la [plantilla](.env.example):

```bash
cp .env.example .env
```

El modelo desplegado se configura en [compose.yaml](compose.yaml). Debe coincidir con `EMBEDDING_MODEL` y `EMBEDDING_DIMENSION`. TEI informa su revisión mediante `model_sha`; si se define `EMBEDDING_REVISION`, debe coincidir con ese SHA. Para modelos locales sin `model_sha`, la variable identifica la revisión fijada en el despliegue y es obligatoria. El modelo debe permanecer fijo durante la ingesta.

## Arranque

```bash
podman compose up -d
podman compose logs -f tei
```

TEI descarga el modelo en el primer arranque. 

Espera unos minutos la descarga. Cuando TEI termine de arrancar, puedes comprobar la conexión con `grpcurl`:

```bash
grpcurl -plaintext -d '{}' 127.0.0.1:8080 tei.v1.Info/Info
```

## Uso

```bash
cargo run -p rag -- ingest
cargo run -p rag -- ingest --source ./manuales
cargo run -p rag -- ingest --prune
cargo run -p rag -- ask "¿Cómo solicito soporte?"
cargo run -p rag -- ask --top-k 8 "¿Cómo solicito soporte?"
```

`ingest` requiere `SOURCE_DIR` o `--source`. El informe muestra documentos nuevos, actualizados, sin cambios, omitidos, conservados y eliminados, junto con los fragmentos escritos y el total del índice.

`ask` consulta el índice activo y genera una respuesta con referencias a los fragmentos recuperados. Si no encuentra evidencia suficiente, devuelve un mensaje sin llamar a OpenAI.

## Ingesta incremental

La primera carga crea una colección estable y publica `QDRANT_ALIAS` después de confirmar los puntos. Las siguientes reutilizan la colección activa. El estado se almacena en metadatos de la colección y de sus puntos en Qdrant.

- Los documentos nuevos generan embeddings.
- Los modificados se reprocesan completos y sus puntos se sobrescriben mediante UUIDs deterministas. Los fragmentos sobrantes se retiran después de confirmar las escrituras nuevas.
- Los documentos intactos y completos se omiten. Una ejecución sin cambios consulta `Info` de TEI y Qdrant, sin generar embeddings.
- Los documentos ausentes se conservan hasta ejecutar `--prune`, que muestra la ruta y la colección antes de eliminar sus puntos.

La identidad depende del corpus y de la ruta relativa; renombrar un documento equivale a agregar una ruta y retirar otra. Los hashes SHA-256 detectan cambios de contenido y procesamiento. Cambiar los parámetros de fragmentación reprocesa los documentos; cambiar el modelo, revisión, tipo numérico o dimensiones del índice se rechaza.

Cada colección queda vinculada a la carpeta absoluta y al alias de su corpus. 

## Condiciones operativas

- Los errores de lectura o recorrido cancelan la ingesta antes de escribir en Qdrant. Se omiten archivos vacíos, formatos no admitidos y enlaces simbólicos internos, conservando cualquier contenido previamente indexado.
- Una carpeta vacía o sin texto fragmentable no provoca eliminaciones, incluso con `--prune`.
- Tras un fallo, repetir `ingest` permite completar los documentos pendientes o incompletos. Una carga inicial interrumpida reutiliza la colección creada.
- Las actualizaciones son progresivas: las consultas concurrentes pueden recuperar versiones mezcladas.
- El bloqueo de escritores es local y requiere la misma URL de Qdrant, alias y directorio temporal compartido.
- La CLI modifica puntos de la colección activa; no elimina colecciones inactivas.
