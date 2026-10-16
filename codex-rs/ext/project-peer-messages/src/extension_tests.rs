use super::*;
use crate::PeerSendOutcome;
use crate::PeerSendRequest;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use futures::future::BoxFuture;
use serde_json::json;

struct NeverCalled;
impl PeerMessageHost for NeverCalled {
    fn send(&self, _request: PeerSendRequest) -> BoxFuture<'_, PeerSendOutcome> {
        Box::pin(async { panic!("exposure test must not send") })
    }
}

#[tokio::test]
async fn only_bound_independent_senders_get_tools() {
    let a = ThreadId::new();
    let b = ThreadId::new();
    let policy = Arc::new(PeerPolicy::from_bytes(&serde_json::to_vec(&json!({
        "version":1,"grants":[{"sender":a,"recipient":b,"active_send":true,"idle_wake":false}]
    })).unwrap()).unwrap());
    let extension = PeerExtension {
        policy,
        host: Arc::new(NeverCalled),
    };
    let session = ExtensionData::new(a.to_string());
    let unbound = ExtensionData::new(a.to_string());
    assert!(extension.tools(&session, &unbound).is_empty());
    for (caller, source, expected) in [
        (a, SessionSource::Exec, 1),
        (b, SessionSource::Exec, 0),
        (ThreadId::new(), SessionSource::Exec, 0),
        (
            a,
            SessionSource::SubAgent(SubAgentSource::Other("test".into())),
            0,
        ),
    ] {
        let thread = ExtensionData::new(caller.to_string());
        <PeerExtension as ThreadLifecycleContributor<()>>::on_thread_start(
            &extension,
            ThreadStartInput {
                config: &(),
                session_source: &source,
                persistent_thread_state_available: false,
                environments: &[],
                mcp_resource_client: None,
                extension_metrics: None,
                session_store: &session,
                thread_store: &thread,
            },
        )
        .await;
        assert_eq!(extension.tools(&session, &thread).len(), expected);
    }
}
