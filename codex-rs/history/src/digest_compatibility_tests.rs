//! Source-derived historical digest goldens for the shipped c196cb9 payload contract.
//!
//! Provenance: c196cb9040fbfba882e1ff5ae539736b7a8ab81b, history/src/lib.rs and
//! protocol/src/models.rs. These are synthetic source-derived fixtures, NOT old-binary captures.
//! The old durable projection drops cell_id, executed_tool_calls, and tool_calls_complete.
//! Fixed hashes were computed independently with Node's crypto SHA-256 over the explicit old
//! durable JSON below, recursively sorting object keys while preserving array order:
//! ```text
//! canonical(v) = v.map(canonical) for arrays; for objects:
//!   Object.fromEntries(Object.keys(v).sort().map(k => [k, canonical(v[k])]))
//!   otherwise v
//! bytes = JSON.stringify(canonical(old_durable_wire ?? source_wire))
//! digest = "sha256-response-item-envelope-v1:" + SHA256(bytes as UTF-8).hex()
//! ```
//! Do not regenerate these expectations from the current Rust serializer: that would hide drift.

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::CompactedHistoryDigest;
use crate::CompactedHistoryResolver;
use crate::CompactedItem;
use crate::ResponseItemEnvelope;
use crate::RolloutItem;

struct HistoricalFixture {
    name: &'static str,
    source_wire: &'static str,
    old_durable_wire: Option<&'static str>,
    source_digest: &'static str,
}

const FIXTURES: &[HistoricalFixture] = &[
    HistoricalFixture {
        name: "message_inline_image_without_detail",
        source_wire: r#"{
  "type": "response_item",
  "payload": {
    "type": "message",
    "id": "msg-old-inline",
    "role": "user",
    "content": [
      {
        "type": "input_image",
        "image_url": "data:image/png;base64,AA=="
      }
    ]
  }
}"#,
        old_durable_wire: None,
        source_digest: "sha256-response-item-envelope-v1:b79c18df57ab648d24d4b429016728e7e548b3e22a6d7276970c5933f160afed",
    },
    HistoricalFixture {
        name: "function_output_inline_image_and_old_budget",
        source_wire: r#"{
  "type": "response_item",
  "payload": {
    "type": "function_call_output",
    "id": "fco-old-inline",
    "call_id": "call-old-1",
    "name": "read_fixture",
    "namespace": "synthetic",
    "output": [
      {
        "type": "input_text",
        "text": "synthetic output"
      },
      {
        "type": "input_image",
        "image_url": "data:image/png;base64,AQ==",
        "detail": "auto"
      }
    ]
  },
  "metadata": {
    "client_authored": false,
    "fallback_token_limit_override": 1536
  }
}"#,
        old_durable_wire: None,
        source_digest: "sha256-response-item-envelope-v1:5d1e3f7def4eb3528f182bd07d96fc053e837873bcc4e8b87591627cba273351",
    },
    HistoricalFixture {
        name: "custom_output_inline_image_and_host_only_passthrough",
        source_wire: r#"{
  "type": "response_item",
  "payload": {
    "type": "custom_tool_call_output",
    "id": "ctco-old-inline",
    "call_id": "call-old-2",
    "name": "fixture_custom",
    "output": [
      {
        "type": "input_image",
        "image_url": "data:image/png;base64,Ag==",
        "detail": "high"
      },
      {
        "type": "input_text",
        "text": "synthetic custom output"
      }
    ],
    "internal_chat_message_metadata_passthrough": {
      "turn_id": "turn-old",
      "cell_id": "cell-old",
      "executed_tool_calls": [
        {
          "name": "synthetic.read",
          "arguments": {
            "key": "fixture"
          }
        }
      ],
      "tool_calls_complete": true
    }
  },
  "metadata": {
    "client_authored": true,
    "fallback_token_limit_override": 768,
    "user_input_order": 7
  }
}"#,
        old_durable_wire: Some(
            r#"{
  "type": "response_item",
  "payload": {
    "type": "custom_tool_call_output",
    "id": "ctco-old-inline",
    "call_id": "call-old-2",
    "name": "fixture_custom",
    "output": [
      {
        "type": "input_image",
        "image_url": "data:image/png;base64,Ag==",
        "detail": "high"
      },
      {
        "type": "input_text",
        "text": "synthetic custom output"
      }
    ],
    "internal_chat_message_metadata_passthrough": {
      "turn_id": "turn-old"
    }
  },
  "metadata": {
    "client_authored": true,
    "fallback_token_limit_override": 768,
    "user_input_order": 7
  }
}"#,
        ),
        source_digest: "sha256-response-item-envelope-v1:db7468fbcfa5fad05fdb83507935ea05b1f95aa54e5ca7fd8964a151a8c5ca21",
    },
    HistoricalFixture {
        name: "function_output_text_control",
        source_wire: r#"{
  "type": "response_item",
  "payload": {
    "type": "function_call_output",
    "id": "fco-old-text",
    "call_id": "call-old-3",
    "output": "synthetic text output"
  }
}"#,
        old_durable_wire: None,
        source_digest: "sha256-response-item-envelope-v1:805fd710ba94d9fcdf336f05b537ca5fcfa6a25d7321c4f9bad5d72cfe70706f",
    },
];

fn source_from_wire(wire: &str) -> ResponseItemEnvelope {
    let RolloutItem::ResponseItem(source) = serde_json::from_str::<RolloutItem>(wire)
        .expect("historical synthetic source should decode")
    else {
        panic!("historical fixture must be a response item");
    };
    source
}

fn checkpoint_with_historical_digest(
    source: &ResponseItemEnvelope,
    expected_digest: &str,
) -> CompactedItem {
    serde_json::from_value(json!({
        "message": "source-derived historical digest",
        "replacement_history_entries": [{
            "type": "reference_v2",
            "item_id": source.item.id().expect("historical source ID").as_str(),
            "source_digest": expected_digest,
        }],
    }))
    .expect("historical V2 reference should decode")
}

#[test]
fn source_derived_c196cb9_goldens_preserve_projection_digest_and_v2_resolution() {
    for fixture in FIXTURES {
        let source = source_from_wire(fixture.source_wire);
        let expected_projection: serde_json::Value =
            serde_json::from_str(fixture.old_durable_wire.unwrap_or(fixture.source_wire))
                .expect("historical durable projection should decode");
        assert_eq!(
            serde_json::to_value(RolloutItem::ResponseItem(source.clone()))
                .expect("current source should serialize"),
            expected_projection,
            "historical durable projection changed for {}",
            fixture.name
        );
        assert_eq!(
            CompactedHistoryDigest::from_envelope(&source)
                .expect("historical source should encode")
                .as_str(),
            fixture.source_digest,
            "historical digest changed for {}",
            fixture.name
        );

        // The reference uses the fixed historical hash, never reference_v2() from current code.
        let checkpoint = checkpoint_with_historical_digest(&source, fixture.source_digest);
        let mut resolver = CompactedHistoryResolver::default();
        resolver.index_explicit_sources(&RolloutItem::ResponseItem(source.clone()));
        assert_eq!(
            resolver
                .resolve_compacted_item_detailed(&checkpoint)
                .expect("historical V2 reference should resolve"),
            Some(vec![source]),
            "historical source did not resolve for {}",
            fixture.name
        );
    }
}

#[test]
fn source_derived_c196cb9_digest_rejects_changed_inline_image_with_the_same_id() {
    let fixture = &FIXTURES[1];
    let mut changed: serde_json::Value =
        serde_json::from_str(fixture.source_wire).expect("historical source should decode");
    changed["payload"]["output"][1]["image_url"] = json!("data:image/png;base64,Aw==");
    let source = source_from_wire(&serde_json::to_string(&changed).expect("changed source JSON"));
    let actual_digest =
        CompactedHistoryDigest::from_envelope(&source).expect("changed source should encode");
    assert_ne!(actual_digest.as_str(), fixture.source_digest);

    let checkpoint = checkpoint_with_historical_digest(&source, fixture.source_digest);
    let mut resolver = CompactedHistoryResolver::default();
    resolver.index_explicit_sources(&RolloutItem::ResponseItem(source.clone()));
    let error = resolver
        .resolve_compacted_item_detailed(&checkpoint)
        .expect_err("changed image must not satisfy the old V2 digest");
    assert!(error.missing_item_ids().is_empty());
    assert!(error.digest_encoding_failure_item_ids().is_empty());
    let [mismatch] = error.digest_mismatches() else {
        panic!("expected one payload digest mismatch: {error}");
    };
    assert_eq!(
        (
            mismatch.item_id(),
            mismatch.expected().as_str(),
            mismatch.actual().as_str(),
        ),
        (
            source.item.id().expect("unchanged source ID").as_str(),
            fixture.source_digest,
            actual_digest.as_str(),
        )
    );
}
