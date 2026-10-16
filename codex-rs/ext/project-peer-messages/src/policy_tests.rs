use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

const A: &str = "00000000-0000-4000-8000-000000000001";
const B: &str = "00000000-0000-4000-8000-000000000002";

fn bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({"version":1,"grants":[{
        "sender":A,"recipient":B,"active_send":true,"idle_wake":false
    }]}))
    .unwrap()
}

#[test]
fn grants_are_directed_and_wake_is_separate() {
    let policy = PeerPolicy::from_bytes(&bytes()).unwrap();
    let a = ThreadId::from_string(A).unwrap();
    let b = ThreadId::from_string(B).unwrap();
    assert!(policy.allows(a, b, DeliveryMode::ActiveTurn));
    assert!(!policy.allows(a, b, DeliveryMode::IdleTurn));
    assert!(!policy.allows(b, a, DeliveryMode::ActiveTurn));
    assert_eq!(policy.recipients(a), vec![B.to_owned()]);
    assert_eq!(
        policy.generation(),
        PeerPolicy::from_bytes(&bytes()).unwrap().generation()
    );
}

#[test]
fn invalid_overbroad_and_oversized_policies_fail_closed() {
    let valid: serde_json::Value = serde_json::from_slice(&bytes()).unwrap();
    let mut cases = vec![
        json!({"version":2,"grants":[]}),
        json!({"version":1,"grants":[],"enabled":true}),
    ];
    for (field, value) in [
        ("sender", json!("*")),
        ("recipient", json!(A)),
        ("active_send", json!(false)),
        ("extra", json!(true)),
    ] {
        let mut changed = valid.clone();
        changed["grants"][0][field] = value;
        cases.push(changed);
    }
    let mut duplicate = valid.clone();
    duplicate["grants"]
        .as_array_mut()
        .unwrap()
        .push(valid["grants"][0].clone());
    cases.push(duplicate);
    for value in cases {
        assert!(PeerPolicy::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(PeerPolicy::from_bytes(&vec![b' '; MAX_POLICY_BYTES + 1]).is_err());
}

#[test]
fn absent_policy_stays_disabled_until_a_new_owner_cache() {
    let home = tempfile::tempdir().unwrap();
    let cache = ProcessPolicyCache::default();
    assert!(cache.load(home.path()).unwrap().is_none());
    fs::write(home.path().join(HOST_POLICY_FILE), bytes()).unwrap();
    assert!(cache.load(home.path()).unwrap().is_none());
    assert!(
        ProcessPolicyCache::default()
            .load(home.path())
            .unwrap()
            .is_some()
    );
    let other = tempfile::tempdir().unwrap();
    assert!(cache.load(other.path()).is_err());
}

#[test]
fn loaded_and_invalid_policy_snapshots_never_reload() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(HOST_POLICY_FILE);
    fs::write(&path, bytes()).unwrap();
    let cache = ProcessPolicyCache::default();
    let first = cache.load(home.path()).unwrap().unwrap();
    fs::remove_file(&path).unwrap();
    let second = cache.load(home.path()).unwrap().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    fs::write(&path, "invalid").unwrap();
    let invalid = ProcessPolicyCache::default();
    assert!(invalid.load(home.path()).is_err());
    fs::write(&path, bytes()).unwrap();
    assert!(invalid.load(home.path()).is_err());
}

#[test]
fn an_unresolvable_owner_home_also_requires_restart() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("not-created-yet");
    let cache = ProcessPolicyCache::default();
    assert!(cache.load(&home).is_err());
    fs::create_dir(&home).unwrap();
    fs::write(home.join(HOST_POLICY_FILE), bytes()).unwrap();
    assert!(cache.load(&home).is_err());
}
