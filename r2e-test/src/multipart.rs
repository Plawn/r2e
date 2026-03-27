use std::sync::atomic::{AtomicU64, Ordering};

static BOUNDARY_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A multipart/form-data body builder for test requests.
pub(crate) struct MultipartForm {
    boundary: String,
    parts: Vec<Part>,
}

enum Part {
    Text {
        name: String,
        value: String,
    },
    File {
        field_name: String,
        file_name: String,
        content_type: String,
        data: Vec<u8>,
    },
}

impl MultipartForm {
    pub(crate) fn new() -> Self {
        let n = BOUNDARY_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            boundary: format!("r2e-test-boundary-{n}"),
            parts: Vec::new(),
        }
    }

    pub(crate) fn add_text(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.parts.push(Part::Text {
            name: name.into(),
            value: value.into(),
        });
    }

    pub(crate) fn add_file(
        &mut self,
        field_name: impl Into<String>,
        file_name: impl Into<String>,
        content_type: impl Into<String>,
        data: impl Into<Vec<u8>>,
    ) {
        self.parts.push(Part::File {
            field_name: field_name.into(),
            file_name: file_name.into(),
            content_type: content_type.into(),
            data: data.into(),
        });
    }

    pub(crate) fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut body = Vec::new();
        for part in &self.parts {
            body.extend_from_slice(b"--");
            body.extend_from_slice(self.boundary.as_bytes());
            body.extend_from_slice(b"\r\n");
            match part {
                Part::Text { name, value } => {
                    body.extend_from_slice(
                        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n")
                            .as_bytes(),
                    );
                    body.extend_from_slice(value.as_bytes());
                }
                Part::File {
                    field_name,
                    file_name,
                    content_type,
                    data,
                } => {
                    body.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{field_name}\"; filename=\"{file_name}\"\r\n\
                             Content-Type: {content_type}\r\n\r\n"
                        )
                        .as_bytes(),
                    );
                    body.extend_from_slice(data);
                }
            }
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(b"--");
        body.extend_from_slice(self.boundary.as_bytes());
        body.extend_from_slice(b"--\r\n");
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_single_text_field() {
        let mut form = MultipartForm::new();
        form.add_text("name", "Alice");
        let body = String::from_utf8(form.encode()).unwrap();
        assert!(body.contains("Content-Disposition: form-data; name=\"name\""));
        assert!(body.contains("Alice"));
    }

    #[test]
    fn encode_single_file() {
        let mut form = MultipartForm::new();
        form.add_file("avatar", "photo.png", "image/png", b"PNG_DATA".to_vec());
        let body = String::from_utf8(form.encode()).unwrap();
        assert!(body.contains("name=\"avatar\"; filename=\"photo.png\""));
        assert!(body.contains("Content-Type: image/png"));
        assert!(body.contains("PNG_DATA"));
    }

    #[test]
    fn encode_mixed_fields_and_files() {
        let mut form = MultipartForm::new();
        form.add_text("description", "Profile photo");
        form.add_file("avatar", "photo.png", "image/png", b"PNG".to_vec());
        let body = String::from_utf8(form.encode()).unwrap();
        assert!(body.contains("Profile photo"));
        assert!(body.contains("filename=\"photo.png\""));
    }

    #[test]
    fn content_type_contains_boundary() {
        let form = MultipartForm::new();
        let ct = form.content_type();
        assert!(ct.starts_with("multipart/form-data; boundary="));
        let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
        assert!(boundary.starts_with("r2e-test-boundary-"));
    }

    #[test]
    fn encode_empty_form() {
        let form = MultipartForm::new();
        let body = String::from_utf8(form.encode()).unwrap();
        // Should contain only the closing boundary
        assert!(body.contains("--r2e-test-boundary-"));
        assert!(body.ends_with("--\r\n"));
    }

    #[test]
    fn encode_binary_data_preserved() {
        let binary = vec![0x00, 0xFF, 0x80, 0x01];
        let mut form = MultipartForm::new();
        form.add_file("data", "bin.dat", "application/octet-stream", binary.clone());
        let encoded = form.encode();
        // The binary bytes must appear in the encoded output
        assert!(encoded
            .windows(binary.len())
            .any(|w| w == binary.as_slice()));
    }

    #[test]
    fn unique_boundaries() {
        let a = MultipartForm::new();
        let b = MultipartForm::new();
        assert_ne!(a.boundary, b.boundary);
    }
}
