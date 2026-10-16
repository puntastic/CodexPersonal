use crate::PeerMessageHost;
use crate::PeerPolicy;
use crate::PeerRuntimeBinding;
use crate::tool::PeerTool;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolContributor;
use codex_protocol::ThreadId;
use codex_tools::ToolCall;
use codex_tools::ToolExecutor;
use std::sync::Arc;

struct PeerExtension {
    policy: Arc<PeerPolicy>,
    host: Arc<dyn PeerMessageHost>,
}

struct CallerBinding {
    caller: ThreadId,
    runtime: PeerRuntimeBinding,
}

impl<C: Sync> ThreadLifecycleContributor<C> for PeerExtension {
    fn on_thread_start<'a>(&'a self, input: ThreadStartInput<'a, C>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if input.session_source.is_non_root_agent() {
                return;
            }
            let Ok(caller) = ThreadId::from_string(input.thread_store.level_id()) else {
                return;
            };
            if self.policy.participates(caller) {
                let runtime = PeerRuntimeBinding::new(&self.policy);
                input.thread_store.insert(runtime.clone());
                input.thread_store.insert(CallerBinding { caller, runtime });
            }
        })
    }
}

impl ToolContributor for PeerExtension {
    fn tools(
        &self,
        _session: &ExtensionData,
        thread: &ExtensionData,
    ) -> Vec<Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>>> {
        let Some(binding) = thread.get::<CallerBinding>() else {
            return Vec::new();
        };
        if self.policy.recipients(binding.caller).is_empty() {
            return Vec::new();
        }
        vec![Arc::new(PeerTool {
            caller: binding.caller,
            runtime: binding.runtime.clone(),
            policy: Arc::clone(&self.policy),
            host: Arc::clone(&self.host),
        })]
    }
}

/// Install native tools using a previously loaded immutable host policy.
/// Hosts omit this call when policy is absent or invalid; installation never writes policy.
pub fn install<C: Sync + 'static>(
    registry: &mut ExtensionRegistryBuilder<C>,
    policy: Arc<PeerPolicy>,
    host: Arc<dyn PeerMessageHost>,
) {
    let extension = Arc::new(PeerExtension { policy, host });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
#[path = "extension_tests.rs"]
mod tests;
