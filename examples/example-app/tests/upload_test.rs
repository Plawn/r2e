//! Multipart endpoints: derived form schema, OpenAPI modeling, and an
//! end-to-end upload through the r2e-test multipart builders.

use example_app::controllers::upload_controller::ProfileUpload;
use r2e::multipart::MultipartSchema;
use r2e_test::TestApp;

#[test]
fn derived_multipart_schema_shape() {
    let schema = ProfileUpload::multipart_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["name"], serde_json::json!({ "type": "string" }));
    assert_eq!(
        schema["properties"]["bio"],
        serde_json::json!({ "type": "string" }),
        "Option<String> keeps the inner schema"
    );
    assert_eq!(
        schema["properties"]["avatar"],
        serde_json::json!({ "type": "string", "format": "binary" })
    );
    assert_eq!(
        schema["properties"]["attachments"],
        serde_json::json!({ "type": "array", "items": { "type": "string", "format": "binary" } })
    );

    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // Vec<UploadedFile> is not required: an absent field yields an empty Vec.
    assert_eq!(required, vec!["name", "avatar"]);
}

#[r2e::test(app = example_app::app)]
async fn typed_multipart_upload_roundtrip(app: TestApp) {
    let resp = app
        .post("/uploads/profile")
        .field("name", "Ada")
        .file("avatar", "ada.png", "image/png", vec![1u8, 2, 3])
        .file("attachments", "notes.txt", "text/plain", b"hello".to_vec())
        .send()
        .await;
    resp.assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["name"], "Ada");
    assert_eq!(body["bio"], serde_json::Value::Null);
    assert_eq!(body["avatar_size"], 3);
    assert_eq!(body["avatar_filename"], "ada.png");
    assert_eq!(body["avatar_content_type"], "image/png");
    assert_eq!(body["attachment_count"], 1);
}

#[r2e::test(app = example_app::app)]
async fn openapi_models_typed_multipart(app: TestApp) {
    let resp = app.get("/openapi.json").send().await;
    resp.assert_ok();
    let spec: serde_json::Value = resp.json();

    let req_body = &spec["paths"]["/uploads/profile"]["post"]["requestBody"];
    assert_eq!(req_body["required"], true);
    assert_eq!(
        req_body["content"]["multipart/form-data"]["schema"]["$ref"],
        "#/components/schemas/ProfileUpload"
    );

    let schema = &spec["components"]["schemas"]["ProfileUpload"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["avatar"]["format"], "binary");
    assert_eq!(schema["properties"]["attachments"]["type"], "array");
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("name")));
    assert!(
        !required.contains(&serde_json::json!("bio")),
        "Option<String> field must not be required"
    );
}

#[r2e::test(app = example_app::app)]
async fn openapi_models_raw_multipart_as_free_form(app: TestApp) {
    let resp = app.get("/openapi.json").send().await;
    resp.assert_ok();
    let spec: serde_json::Value = resp.json();

    let req_body = &spec["paths"]["/uploads/raw"]["post"]["requestBody"];
    assert_eq!(
        req_body["content"]["multipart/form-data"]["schema"],
        serde_json::json!({ "type": "object" })
    );
    assert!(spec["paths"]["/uploads/raw"]["post"]["responses"]["400"].is_object());
}
