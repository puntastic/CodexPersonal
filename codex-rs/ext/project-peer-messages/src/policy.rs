use crate::DeliveryMode;
use codex_protocol::ThreadId;
use serde::Deserialize;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

pub const HOST_POLICY_FILE: &str = "project-peer-messages.json";
pub const MAX_POLICY_BYTES: usize = 16 * 1024;
const MAX_GRANTS: usize = 32;
const MAX_RECIPIENTS_PER_SENDER: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    version: u8,
    grants: Vec<Grant>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    sender: ThreadId,
    recipient: ThreadId,
    active_send: bool,
    idle_wake: bool,
}

/// An immutable, versioned set of directed host grants. There are no wildcard grants.
pub struct PeerPolicy {
    generation: String,
    grants: Vec<Grant>,
}

impl PeerPolicy {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_POLICY_BYTES {
            return Err("peer policy exceeds 16384 bytes".into());
        }
        let file: PolicyFile = serde_json::from_slice(bytes)
            .map_err(|_| "peer policy must be strict versioned JSON with UUID grants".to_owned())?;
        if file.version != 1 || file.grants.len() > MAX_GRANTS {
            return Err("peer policy requires version 1 and at most 32 grants".into());
        }
        let mut recipients = BTreeMap::<String, Vec<ThreadId>>::new();
        for grant in &file.grants {
            if grant.sender == grant.recipient || !(grant.active_send || grant.idle_wake) {
                return Err("peer policy rejects self grants and grants with no rights".into());
            }
            let targets = recipients.entry(grant.sender.to_string()).or_default();
            if targets.contains(&grant.recipient) || targets.len() >= MAX_RECIPIENTS_PER_SENDER {
                return Err(
                    "peer policy rejects duplicate grants and more than 8 targets per sender"
                        .into(),
                );
            }
            targets.push(grant.recipient);
        }
        Ok(Self {
            generation: format!("{:x}", Sha256::digest(bytes)),
            grants: file.grants,
        })
    }

    pub fn generation(&self) -> &str {
        &self.generation
    }

    pub fn participates(&self, thread: ThreadId) -> bool {
        self.grants
            .iter()
            .any(|grant| grant.sender == thread || grant.recipient == thread)
    }

    pub fn recipients(&self, sender: ThreadId) -> Vec<String> {
        let mut recipients = self
            .grants
            .iter()
            .filter(|grant| grant.sender == sender)
            .map(|grant| grant.recipient.to_string())
            .collect::<Vec<_>>();
        recipients.sort();
        recipients
    }

    pub fn allows(&self, sender: ThreadId, recipient: ThreadId, mode: DeliveryMode) -> bool {
        self.grants.iter().any(|grant| {
            grant.sender == sender
                && grant.recipient == recipient
                && match mode {
                    DeliveryMode::ActiveTurn => grant.active_send,
                    DeliveryMode::IdleTurn => grant.idle_wake,
                }
        })
    }
}

#[derive(Default)]
struct ProcessPolicyCache {
    snapshot: OnceLock<PolicySnapshot>,
}

struct PolicySnapshot {
    requested_home: PathBuf,
    canonical_home: Option<PathBuf>,
    result: Result<Option<Arc<PeerPolicy>>, String>,
}

impl ProcessPolicyCache {
    fn load(&self, host_home: &Path) -> Result<Option<Arc<PeerPolicy>>, String> {
        let snapshot = self.snapshot.get_or_init(|| {
            let (canonical_home, result) = match fs::canonicalize(host_home) {
                Ok(home) => {
                    let result = read_policy(&home);
                    (Some(home), result)
                }
                Err(_) => (
                    None,
                    Err("cannot resolve host CODEX_HOME for peer policy".to_owned()),
                ),
            };
            PolicySnapshot {
                requested_home: host_home.to_path_buf(),
                canonical_home,
                result,
            }
        });
        if host_home != snapshot.requested_home.as_path()
            && !snapshot.canonical_home.as_ref().is_some_and(|expected| {
                fs::canonicalize(host_home).is_ok_and(|home| &home == expected)
            })
        {
            return Err("peer policy is bound to another CODEX_HOME; restart the host".into());
        }
        snapshot.result.clone()
    }
}

/// Load the fixed host file once per process, including absence and parse failures.
/// File changes never revoke or refresh this snapshot: a full host restart is required.
pub fn process_policy(host_home: &Path) -> Result<Option<Arc<PeerPolicy>>, String> {
    static CACHE: ProcessPolicyCache = ProcessPolicyCache {
        snapshot: OnceLock::new(),
    };
    CACHE.load(host_home)
}

fn read_policy(home: &Path) -> Result<Option<Arc<PeerPolicy>>, String> {
    let path = home.join(HOST_POLICY_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot inspect peer policy file".into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("peer policy must be a regular non-symlink file".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "cannot open peer policy file".to_owned())?
        .take((MAX_POLICY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read peer policy file".to_owned())?;
    PeerPolicy::from_bytes(&bytes).map(|policy| Some(Arc::new(policy)))
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
