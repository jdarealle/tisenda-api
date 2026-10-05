# Convenciones del proyecto

- Mantén las declaraciones de módulos y reexportaciones públicas en `src/lib.rs`;
- Usa el estilo `nombre.rs` y `nombre/submodulo.rs` de forma consistente.
- Usa módulos padre como puntos de entrada con submódulos privados y reexportaciones selectivas cuando agrupen varias responsabilidades. Separa por responsabilidad; evita archivos o capas sin una función propia.
- Cuando `src/nombre.rs` tenga submódulos en `src/nombre/`, mantén el archivo padre breve, con documentación del módulo, declaraciones `mod` y reexportaciones selectivas mediante `pub use` o visibilidad restringida, como `pub(crate) use`. Coloca tipos, implementaciones y lógica en los submódulos correspondientes a su responsabilidad, siguiendo la organización de `ingest`, `docling` e `ingestions`.
- Limita la visibilidad al ámbito necesario: privado para implementación local, `pub(super)` dentro del módulo padre, `pub(crate)` entre áreas y `pub` para la API pública. Evita reexportaciones con `*`.
- Mantén privados los campos que protejan invariantes; los tipos que transportan datos pueden tener campos públicos.
- Mantén el README como documentación técnica de lo existente.
