use crate::EventMetadata;

pub const HEADER_EVENT_ID: &str = "r2e-event-id";
pub const HEADER_TIMESTAMP: &str = "r2e-timestamp";
pub const HEADER_CORRELATION_ID: &str = "r2e-correlation-id";
pub const HEADER_PARTITION_KEY: &str = "r2e-partition-key";
pub const HEADER_USER_PREFIX: &str = "r2e-h-";

/// Encode [`EventMetadata`] into string key-value pairs suitable for
/// message headers in any distributed backend.
pub fn encode_metadata(metadata: &EventMetadata) -> Vec<(String, String)> {
    let mut pairs = Vec::new();

    pairs.push((
        HEADER_EVENT_ID.to_string(),
        metadata.event_id.to_string(),
    ));
    pairs.push((
        HEADER_TIMESTAMP.to_string(),
        metadata.timestamp.to_string(),
    ));

    if let Some(ref cid) = metadata.correlation_id {
        pairs.push((HEADER_CORRELATION_ID.to_string(), cid.clone()));
    }

    if let Some(ref pk) = metadata.partition_key {
        pairs.push((HEADER_PARTITION_KEY.to_string(), pk.clone()));
    }

    for (k, v) in &metadata.headers {
        pairs.push((format!("{HEADER_USER_PREFIX}{k}"), v.clone()));
    }

    pairs
}

/// Decode [`EventMetadata`] from string key-value pairs (message headers).
pub fn decode_metadata(
    pairs: impl Iterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
) -> EventMetadata {
    let mut metadata = EventMetadata::new();

    for (key, value) in pairs {
        let k = key.as_ref();
        let v = value.as_ref();
        match k {
            HEADER_EVENT_ID => {
                if let Ok(id) = v.parse::<u64>() {
                    metadata.event_id = id;
                }
            }
            HEADER_TIMESTAMP => {
                if let Ok(ts) = v.parse::<u64>() {
                    metadata.timestamp = ts;
                }
            }
            HEADER_CORRELATION_ID => {
                metadata.correlation_id = Some(v.to_string());
            }
            HEADER_PARTITION_KEY => {
                metadata.partition_key = Some(v.to_string());
            }
            _ if k.starts_with(HEADER_USER_PREFIX) => {
                metadata.headers.insert(
                    k.trim_start_matches(HEADER_USER_PREFIX).to_string(),
                    v.to_string(),
                );
            }
            _ => {}
        }
    }

    metadata
}
