//! Tests for globally-unique event ids (P2.1): cross-process uniqueness and
//! metadata codec round-trip of the `u128` id.

use r2e_events::backend::{decode_metadata, encode_metadata};
use r2e_events::{compose_event_id, EventMetadata};

#[test]
fn ids_from_distinct_process_identities_do_not_collide() {
    // Two different "process identities" with the SAME counter value must
    // produce different event ids — this is the collision the dedup set used
    // to suffer from when both instances counted from 1.
    let process_a: u64 = 0x1111_1111_1111_1111;
    let process_b: u64 = 0x2222_2222_2222_2222;

    for counter in 1..=1000u64 {
        let id_a = compose_event_id(process_a, counter);
        let id_b = compose_event_id(process_b, counter);
        assert_ne!(
            id_a, id_b,
            "ids from distinct processes collided at counter {counter}"
        );
    }
}

#[test]
fn same_process_ids_are_unique_per_counter() {
    let process = 0xDEAD_BEEF_CAFE_F00Du64;
    let mut seen = std::collections::HashSet::new();
    for counter in 1..=1000u64 {
        let id = compose_event_id(process, counter);
        assert!(
            seen.insert(id),
            "duplicate id within a process at {counter}"
        );
    }
}

#[test]
fn compose_packs_process_high_counter_low() {
    let id = compose_event_id(0xAAAA_BBBB_CCCC_DDDD, 0x0102_0304_0506_0708);
    assert_eq!(id >> 64, 0xAAAA_BBBB_CCCC_DDDD);
    assert_eq!(id as u64, 0x0102_0304_0506_0708);
}

#[test]
fn generated_metadata_has_nonzero_unique_ids() {
    let a = EventMetadata::new();
    let b = EventMetadata::new();
    assert_ne!(a.event_id, 0);
    assert_ne!(a.event_id, b.event_id, "consecutive emits must differ");
    // Same process → same high bits.
    assert_eq!(a.event_id >> 64, b.event_id >> 64);
}

#[test]
fn codec_round_trip_preserves_large_u128_id() {
    // An id whose high bits (process identity) are set — the case that would
    // truncate if the codec still parsed as u64.
    let mut meta = EventMetadata::new();
    meta.event_id = compose_event_id(0xFFFF_FFFF_FFFF_FFFF, 0x1234_5678_9ABC_DEF0);
    meta.correlation_id = Some("corr-1".to_string());
    meta.partition_key = Some("pk-1".to_string());
    meta = meta.with_header("k", "v");

    let encoded: Vec<_> = encode_metadata(&meta).collect();
    let decoded = decode_metadata(encoded.iter().map(|(k, v)| (k.as_ref(), v.as_str())));

    assert_eq!(decoded.event_id, meta.event_id);
    assert_eq!(decoded.correlation_id, meta.correlation_id);
    assert_eq!(decoded.partition_key, meta.partition_key);
    assert_eq!(decoded.headers.get("k").map(String::as_str), Some("v"));
    assert_eq!(decoded.timestamp, meta.timestamp);
}
