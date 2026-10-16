use super::*;
use futures::future::BoxFuture;
use pretty_assertions::assert_eq;
use std::sync::Mutex;

const A: &str = "00000000-0000-4000-8000-000000000001";
const B: &str = "00000000-0000-4000-8000-000000000002";

#[derive(Default)]
struct FakeHost(Mutex<Vec<PeerSendRequest>>);

impl PeerMessageHost for FakeHost {
    fn send(&self, request: PeerSendRequest) -> BoxFuture<'_, PeerSendOutcome> {
        Box::pin(async move {
            self.0.lock().unwrap().push(request);
            PeerSendOutcome::OutcomeUnknown {
                message_id: "test-attempt".into(),
            }
        })
    }
}

fn tool(sender: &str) -> (PeerTool, Arc<FakeHost>) {
    let policy = Arc::new(PeerPolicy::from_bytes(&serde_json::to_vec(&json!({
        "version":1,"grants":[{"sender":A,"recipient":B,"active_send":true,"idle_wake":false}]
    })).unwrap()).unwrap());
    let host = Arc::new(FakeHost::default());
    (
        PeerTool {
            caller: ThreadId::from_string(sender).unwrap(),
            runtime: PeerRuntimeBinding::new(&policy),
            policy,
            host: host.clone(),
        },
        host,
    )
}

#[tokio::test]
async fn sender_spoofing_and_wake_or_reverse_grants_do_not_reach_host() {
    let (tool, host) = tool(A);
    let mut args = json!({"recipient_id":B,"message":"hello","delivery":"active_turn"});
    args["sender"] = json!(B);
    assert!(tool.send(&args.to_string(), "turn".into()).await.is_err());
    args.as_object_mut().unwrap().remove("sender");
    args["delivery"] = json!("idle_turn");
    assert_eq!(
        tool.send(&args.to_string(), "turn".into()).await.unwrap(),
        PeerSendOutcome::refused(RefusalReason::Denied)
    );
    assert!(host.0.lock().unwrap().is_empty());
    let (reverse, host) = self::tool(B);
    args["recipient_id"] = json!(A);
    args["delivery"] = json!("active_turn");
    assert_eq!(
        reverse
            .send(&args.to_string(), "turn".into())
            .await
            .unwrap(),
        PeerSendOutcome::refused(RefusalReason::Denied)
    );
    assert!(host.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn utf8_payload_boundary_is_exact_and_unknown_is_not_retried() {
    let (tool, host) = tool(A);
    let payload = "é".repeat(MAX_MESSAGE_BYTES / 2);
    let mut args = json!({"recipient_id":B,"message":payload,"delivery":"active_turn"});
    assert_eq!(
        tool.send(&args.to_string(), "trusted-turn".into())
            .await
            .unwrap(),
        PeerSendOutcome::OutcomeUnknown {
            message_id: "test-attempt".into()
        }
    );
    {
        let calls = host.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].sender, ThreadId::from_string(A).unwrap());
        assert_eq!(calls[0].sender_turn_id, "trusted-turn");
        assert_eq!(calls[0].message.as_bytes(), payload.as_bytes());
    }
    args["message"] = json!(format!("{payload}a"));
    assert_eq!(
        tool.send(&args.to_string(), "trusted-turn".into())
            .await
            .unwrap(),
        PeerSendOutcome::refused(RefusalReason::InvalidMessage)
    );
    assert_eq!(host.0.lock().unwrap().len(), 1);
}
