use std::path::Path;

/// Document formats enabled for this integration. Keep in sync with Docling's
/// FormatToExtensions and review capabilities before extending this policy.
pub(crate) fn input_format(filename: &str) -> Option<&'static str> {
    let extension = Path::new(filename)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    Some(match extension.as_str() {
        "pdf" => "pdf",
        "docx" | "dotx" | "docm" | "dotm" => "docx",
        "pptx" | "potx" | "ppsx" | "pptm" | "potm" | "ppsm" => "pptx",
        "xlsx" | "xlsm" | "xltx" | "xltm" => "xlsx",
        "html" | "htm" | "xhtml" => "html",
        "md" | "txt" | "text" | "qmd" | "rmd" => "md",
        "csv" => "csv",
        "adoc" | "asciidoc" | "asc" => "asciidoc",
        "jpg" | "jpeg" | "png" | "tif" | "tiff" | "bmp" | "webp" => "image",
        "vtt" => "vtt",
        _ => return None,
    })
}
