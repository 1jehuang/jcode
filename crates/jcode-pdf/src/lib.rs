use anyhow::{Result, anyhow};
use std::panic::AssertUnwindSafe;
use std::path::Path;

/// Extract text from a PDF.
///
/// `pdf_extract` panics on some exotic/malformed PDFs (e.g. "unexpected entry
/// in unicode map"). A panic inside a tool task kills the task and surfaces a
/// raw panic to the model, so recover it into a normal error (see #573).
pub fn extract_text(path: &Path) -> Result<String> {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text(path)));
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(err)) => Err(err.into()),
        Err(payload) => Err(anyhow!(
            "PDF text extraction failed (parser panic: {})",
            panic_message(&payload)
        )),
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn malformed_pdf_returns_error_instead_of_panicking() {
        let dir = std::env::temp_dir().join("jcode-pdf-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("malformed.pdf");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"%PDF-1.7\nnot a real pdf body\n%%EOF\n")
            .unwrap();
        drop(f);

        let err = extract_text(&path).err();
        assert!(err.is_some(), "expected an error for a malformed PDF");
        let _ = std::fs::remove_file(&path);
    }
}
