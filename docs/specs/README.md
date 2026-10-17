# Especificaciones de comportamiento

Esta carpeta describe el comportamiento vigente de cada capacidad del sistema. El diseño interno y sus razones están en [ARCHITECTURE.md](../ARCHITECTURE.md).

## Metodología

Las especificaciones siguen **SDD + TDD**: cada capacidad tiene una especificación con requisitos y escenarios, y cada requisito se implementa guiado por pruebas.

1. Descubrir las reglas y las preguntas abiertas.
2. Escribir o actualizar la especificación.
3. Escribir una prueba que falle por requisito.
4. Implementar hasta que pase.
5. Buscar casos límite y añadir sus pruebas.
6. Refactorizar y revisar.

En cambios triviales bastan los pasos 3, 4 y 6.

## Estructura

```text
docs/specs/
├── README.md
├── ingestion.md     # Selección, cola, procesamiento, estado de lotes e ingest_documents
└── query.md         # Recuperación, respuesta y citas
```

- Una especificación por capacidad, organizada por dominio.
- Una capacidad pasa a subcarpeta (`<capacidad>/README.md` y un archivo por parte) cuando tiene partes que se entienden y cambian por separado.

## Formato de una especificación

Cada especificación contiene:

- `## Purpose`: para qué sirve la capacidad, qué cubre y sus `Non-goals`.
- `## Requirements`: un `### Requirement:` por comportamiento, con su clave y una declaración con SHALL.
- `#### Scenario:` bajo cada requisito, con pasos **GIVEN** (opcional), **WHEN**, **THEN** y **AND**.
- `## Edge Cases`: condiciones límite y de error, con la clave del requisito afectado.
- `## References`: contrato HTTP, arquitectura y prueba de cada requisito.

Reglas:

- Las palabras clave de la convención se escriben en inglés; el contenido, en el idioma del proyecto.
- Cada requisito describe comportamiento observable. Si la implementación puede cambiar sin alterar el comportamiento visible, ese detalle no pertenece a la especificación.
- La fuerza del requisito se indica con SHALL o MUST, SHOULD y MAY, según [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119).
- Cada requisito tiene al menos un `Scenario` verificable mediante una prueba.
- Lo que falta definir se marca en el propio requisito con `[NEEDS CLARIFICATION: ...]` y se resuelve antes de implementarlo.
- La especificación se mantiene breve; se amplía solo cuando el riesgo del cambio lo justifica.

## Identificadores

Cada requisito se identifica con una clave descriptiva: `<prefijo>.<clave>`, por ejemplo `ing.reintentos`.

| Capacidad | Prefijo |
| --- | --- |
| [ingestion](ingestion.md) | `ing` |
| [query](query.md) | `qry` |

- **Prefijo:** de 2 a 4 letras minúsculas derivadas de la capacidad. Se registra en esta tabla y no cambia aunque cambie el nombre de la capacidad.
- **Clave:** de 1 a 3 palabras en minúsculas, unidas por guiones, que resumen el requisito. Es única dentro del prefijo.
- **Estabilidad:** la clave se conserva al cambiar el título o el detalle del requisito. Un requisito eliminado no cede su clave; si el comportamiento reaparece con otra lógica, recibe una clave distinta.

## Requisitos y pruebas

- Cada requisito tiene al menos una prueba que verifica sus escenarios.
- El nombre de la prueba empieza con la clave en formato Rust: `ing.reintentos` → `fn ing_reintentos_<detalle>()`.
- Buscar la clave (`rg -i 'ing.reintentos'`) localiza el requisito, sus pruebas y sus referencias.
- Pruebas: `cargo test --locked`. El proyecto aún no tiene pruebas automatizadas; cada requisito sin prueba figura como "sin prueba automatizada" en su `References` (`rg 'sin prueba automatizada' docs/specs/` lista lo pendiente).

## Mantenimiento

- Un cambio de comportamiento actualiza su especificación y sus pruebas en el mismo cambio.
- Las especificaciones describen lo vigente; el historial está en el control de versiones.
- Los contratos HTTP y el esquema de datos se enlazan; no se copian.

## Base de esta convención

- [OpenSpec](https://github.com/Fission-AI/OpenSpec/blob/main/docs/concepts.md): especificación como contrato de comportamiento, organizada por dominio. [Formato](https://github.com/Fission-AI/OpenSpec/blob/main/openspec/specs/openspec-conventions/spec.md) de `Purpose`, `Requirement`, `Scenario` y pasos GIVEN/WHEN/THEN.
- [GitHub Spec Kit](https://github.com/github/spec-kit/blob/main/templates/spec-template.md): sección `Edge Cases` y marca `[NEEDS CLARIFICATION]`.
- [Kent Beck: Canon TDD](https://newsletter.kentbeck.com/p/canon-tdd): ciclo de prueba, implementación y refactorización.
