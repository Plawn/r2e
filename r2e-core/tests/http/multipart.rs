use http_body_util::BodyExt;
use r2e_core::http::{IntoResponse, StatusCode};
use r2e_core::web::multipart::MultipartError;

async fn status_of(err: MultipartError) -> StatusCode {
    err.into_response().status()
}

#[r2e_core::test]
async fn field_too_large_maps_to_413() {
    let err = MultipartError::FieldTooLarge {
        field: "avatar".into(),
        limit: 1024,
    };
    assert_eq!(status_of(err).await, StatusCode::PAYLOAD_TOO_LARGE);
}

#[r2e_core::test]
async fn payload_too_large_maps_to_413() {
    let err = MultipartError::PayloadTooLarge { limit: 1024 };
    assert_eq!(status_of(err).await, StatusCode::PAYLOAD_TOO_LARGE);
}

#[r2e_core::test]
async fn missing_field_still_maps_to_400() {
    let err = MultipartError::MissingField("name".into());
    assert_eq!(status_of(err).await, StatusCode::BAD_REQUEST);
}

#[r2e_core::test]
async fn field_too_large_body_contains_limit() {
    let err = MultipartError::FieldTooLarge {
        field: "avatar".into(),
        limit: 4096,
    };
    let resp = err.into_response();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let msg = json["error"].as_str().unwrap();
    assert!(msg.contains("avatar"), "error should name the field: {msg}");
    assert!(
        msg.contains("4096"),
        "error should include the limit: {msg}"
    );
}

#[r2e_core::test]
async fn default_limits_are_sensible() {
    let defaults = r2e_core::web::multipart::MultipartLimits::DEFAULT;
    assert_eq!(defaults.per_field, 10 * 1024 * 1024);
    assert_eq!(defaults.total, 100 * 1024 * 1024);
    assert!(defaults.per_field <= defaults.total);
}

// ── collect_from_with_limits / take_* ──────────────────────────────────────

use r2e_core::http::extract::{FromRequest, Request};
use r2e_core::http::Body;
use r2e_core::web::multipart::{Multipart, MultipartFields, MultipartLimits};

const BOUNDARY: &str = "r2e-test-boundary";

/// `(name, file_name, value)` parts → a `multipart/form-data` request.
fn multipart_request(parts: &[(&str, Option<&str>, &str)]) -> Request {
    let mut body = String::new();
    for (name, file_name, value) in parts {
        body.push_str(&format!("--{BOUNDARY}\r\n"));
        match file_name {
            Some(f) => body.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\n\
                 Content-Type: text/plain\r\n\r\n"
            )),
            None => body.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"
            )),
        }
        body.push_str(value);
        body.push_str("\r\n");
    }
    body.push_str(&format!("--{BOUNDARY}--\r\n"));
    Request::builder()
        .method("POST")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap()
}

async fn collect(
    parts: &[(&str, Option<&str>, &str)],
    per_field: usize,
    total: usize,
) -> Result<MultipartFields, MultipartError> {
    let multipart = <Multipart as FromRequest<()>>::from_request(multipart_request(parts), &())
        .await
        .expect("valid multipart request");
    MultipartFields::collect_from_with_limits(multipart, MultipartLimits { per_field, total }).await
}

#[r2e_core::test]
async fn per_field_limit_is_inclusive() {
    let fields = collect(&[("a", None, "abcde")], 5, 100).await.unwrap();
    assert_eq!(fields.text["a"], ["abcde"]);

    let err = collect(&[("a", None, "abcdef")], 5, 100)
        .await
        .err()
        .expect("over the per-field limit");
    assert!(
        matches!(&err, MultipartError::FieldTooLarge { field, limit: 5 } if field == "a"),
        "{err:?}"
    );
}

#[r2e_core::test]
async fn total_limit_is_inclusive_and_summed_across_fields() {
    // 3 + 2 = 5 == total: accepted.
    let fields = collect(&[("a", None, "abc"), ("b", None, "de")], 10, 5)
        .await
        .unwrap();
    assert_eq!(fields.text["b"], ["de"]);

    // 3 + 3 = 6 > total, though each field is under both limits.
    let err = collect(&[("a", None, "abc"), ("b", None, "def")], 10, 5)
        .await
        .err()
        .expect("over the total limit");
    assert!(
        matches!(err, MultipartError::PayloadTooLarge { limit: 5 }),
        "{err:?}"
    );
}

#[r2e_core::test]
async fn take_accessors_return_the_collected_values() {
    let mut fields = collect(
        &[
            ("title", None, "hello"),
            ("note", None, "n"),
            ("raw", None, "text-bytes"),
            ("doc", Some("a.txt"), "first"),
            ("doc", Some("b.txt"), "second"),
            ("one", Some("one.bin"), "payload"),
            ("blob", Some("blob.bin"), "file-bytes"),
            ("empty", Some("empty.txt"), ""),
        ],
        1024,
        4096,
    )
    .await
    .unwrap();

    assert_eq!(fields.take_text("title").unwrap(), "hello");
    assert!(matches!(
        fields.take_text("title"),
        Err(MultipartError::MissingField(name)) if name == "title"
    ));
    assert_eq!(fields.take_text_opt("note").as_deref(), Some("n"));
    assert_eq!(fields.take_text_opt("note"), None);
    assert_eq!(fields.take_text_opt("absent"), None);

    let one = fields.take_file_opt("one").expect("file present");
    assert_eq!(one.file_name.as_deref(), Some("one.bin"));
    assert_eq!(&one.data[..], b"payload");
    assert_eq!(one.len(), 7);
    assert!(!one.is_empty());
    assert!(fields.take_file_opt("one").is_none());

    let docs = fields.take_files("doc");
    let names: Vec<_> = docs
        .iter()
        .map(|f| f.file_name.as_deref().unwrap())
        .collect();
    assert_eq!(names, ["a.txt", "b.txt"]);
    assert!(fields.take_files("doc").is_empty());

    let empty = fields.take_file("empty").unwrap();
    assert_eq!(empty.len(), 0);
    assert!(empty.is_empty());

    assert_eq!(&fields.take_bytes("blob").unwrap()[..], b"file-bytes");
    assert_eq!(&fields.take_bytes("raw").unwrap()[..], b"text-bytes");
    assert!(matches!(
        fields.take_bytes("raw"),
        Err(MultipartError::MissingField(_))
    ));
}
