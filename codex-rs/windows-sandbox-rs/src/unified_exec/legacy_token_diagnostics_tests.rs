//! Fixture-only observations of the primary token selected for legacy launch.
//! No token is installed on a thread, and no security descriptor is modified.

use crate::spawn_prep::RootCapabilitySid;
use crate::token::TokenGroup;
use std::cell::RefCell;
use std::fmt::Write;
use std::mem::size_of;
use std::mem::size_of_val;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::BorrowedHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::AccessCheck;
use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
use windows_sys::Win32::Security::DuplicateToken;
use windows_sys::Win32::Security::GENERIC_MAPPING;
use windows_sys::Win32::Security::GROUP_SECURITY_INFORMATION;
use windows_sys::Win32::Security::GetFileSecurityW;
use windows_sys::Win32::Security::GetTokenInformation;
use windows_sys::Win32::Security::IsValidSid;
use windows_sys::Win32::Security::OWNER_SECURITY_INFORMATION;
use windows_sys::Win32::Security::SID_AND_ATTRIBUTES;
use windows_sys::Win32::Security::SecurityImpersonation;
use windows_sys::Win32::Security::TOKEN_GROUPS;
use windows_sys::Win32::Security::TokenRestrictedSids;
use windows_sys::Win32::Storage::FileSystem::DELETE;
use windows_sys::Win32::Storage::FileSystem::FILE_ADD_FILE;
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::Storage::FileSystem::FILE_DELETE_CHILD;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_EXECUTE;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;
use windows_sys::Win32::Storage::FileSystem::FILE_READ_DATA;
use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES;

const TOKEN_BYTES: u32 = 16 * 1024;
const DESCRIPTOR_BYTES: u32 = 64 * 1024;
const GROUP_ROWS: usize = 64;
const ROOT_ROWS: usize = 16;
const REPORT_BYTES: usize = 32 * 1024;
const FILE_MASKS: &[(&str, u32)] = &[
    ("FILE_READ_DATA", FILE_READ_DATA),
    ("DELETE", DELETE),
    ("FILE_WRITE_ATTRIBUTES", FILE_WRITE_ATTRIBUTES),
];
const PARENT_MASKS: &[(&str, u32)] = &[
    ("FILE_ADD_FILE", FILE_ADD_FILE),
    ("FILE_DELETE_CHILD", FILE_DELETE_CHILD),
];

thread_local! {
    static REQUEST: RefCell<Option<Rc<RefCell<Slot>>>> = const { RefCell::new(None) };
}

struct Slot {
    fixture: PathBuf,
    cwd: PathBuf,
    result: Option<Observation>,
}

pub(super) struct Observer(Rc<RefCell<Slot>>);

pub(super) fn register(fixture: &Path) -> Observer {
    let slot = Rc::new(RefCell::new(Slot {
        fixture: fixture.to_owned(),
        cwd: fixture.join("workspace"),
        result: None,
    }));
    REQUEST.with(|request| *request.borrow_mut() = Some(Rc::clone(&slot)));
    Observer(slot)
}

impl Observer {
    pub(super) fn take(self) -> Observation {
        self.0.borrow_mut().result.take().unwrap_or_default()
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        REQUEST.with(|request| {
            let mut request = request.borrow_mut();
            if request
                .as_ref()
                .is_some_and(|slot| Rc::ptr_eq(slot, &self.0))
            {
                request.take();
            }
        });
    }
}

fn take_request(cwd: &Path) -> Option<Rc<RefCell<Slot>>> {
    REQUEST.with(|request| {
        let mut request = request.borrow_mut();
        if request
            .as_ref()
            .is_some_and(|slot| slot.borrow().cwd == cwd)
        {
            request.take()
        } else {
            None
        }
    })
}

#[derive(Default)]
pub(super) struct Observation {
    token: Option<TokenObservation>,
    checks: Vec<(
        &'static str,
        &'static str,
        u32,
        Result<AccessResult, String>,
    )>,
}

struct TokenObservation {
    user: Result<Vec<u8>, String>,
    groups: Result<Vec<TokenGroup>, String>,
    restricted: Result<Vec<TokenGroup>, String>,
    root_count: usize,
    roots: Vec<(&'static str, Option<String>)>,
}

#[derive(Debug, PartialEq, Eq)]
struct AccessResult {
    allowed: bool,
    granted: u32,
    status_error: Option<u32>,
}

pub(super) fn observe_before_launch(
    cwd: &Path,
    token: BorrowedHandle<'_>,
    roots: &[RootCapabilitySid],
) {
    let Some(slot) = take_request(cwd) else {
        return;
    };
    let fixture = slot.borrow().fixture.clone();
    // Bounded synchronous work, not timing-neutral. All derived handles and
    // buffers are released here before the original launch path continues.
    // SAFETY: the borrowed primary token remains live with TOKEN_QUERY access.
    let user = unsafe { crate::token::get_user_sid_bytes(token.as_raw_handle()) }
        .map_err(|error| error.to_string());
    let groups = unsafe { crate::token::token_groups(token.as_raw_handle(), TOKEN_BYTES) }
        .map_err(|error| error.to_string());
    let restricted = restricted_sids(token);
    let root_rows = ["workspace", "temp", "tmp"]
        .map(|label| {
            let path = crate::path_normalization::canonicalize_path(&fixture.join(label));
            // Only canonicalize the fixed fixture path. Inspect existing root
            // metadata without opening any path outside this fixture.
            let sid = roots.iter().take(ROOT_ROWS).find(|root| root.root == path);
            (label, sid.map(|root| root.sid_str.clone()))
        })
        .to_vec();
    let mut observation = Observation {
        token: Some(TokenObservation {
            user,
            groups,
            restricted,
            root_count: roots.len(),
            roots: root_rows,
        }),
        checks: Vec::new(),
    };
    let mut duplicate = std::ptr::null_mut();
    // SAFETY: DuplicateToken borrows token and creates an independently owned
    // impersonation token solely for AccessCheck; it does not impersonate us.
    let duplicate =
        if unsafe { DuplicateToken(token.as_raw_handle(), SecurityImpersonation, &mut duplicate) }
            == 0
        {
            Err(format!("DuplicateToken os_error={}", unsafe {
                GetLastError()
            }))
        } else {
            // SAFETY: successful DuplicateToken transferred this new handle.
            Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
        };
    for (label, masks) in [
        ("workspace", PARENT_MASKS),
        ("workspace/workspace-delete.txt", FILE_MASKS),
        ("temp", PARENT_MASKS),
        ("temp/temp-delete.txt", FILE_MASKS),
        ("tmp", PARENT_MASKS),
        ("tmp/tmp-delete.txt", FILE_MASKS),
        ("outside", PARENT_MASKS),
        ("outside/outside-delete.txt", FILE_MASKS),
        ("workspace/.git", &[("DELETE", DELETE)][..]),
    ] {
        let descriptor = read_descriptor(&fixture.join(label));
        for &(name, mask) in masks {
            let result = match (&duplicate, &descriptor) {
                (Ok(duplicate), Ok(descriptor)) => access_check(duplicate, descriptor, mask),
                (Err(error), _) | (_, Err(error)) => Err(error.clone()),
            };
            observation.checks.push((label, name, mask, result));
        }
    }
    // Close the only derived handle before publishing owned data.
    drop(duplicate);
    slot.borrow_mut().result = Some(observation);
}

fn restricted_sids(token: BorrowedHandle<'_>) -> Result<Vec<TokenGroup>, String> {
    let mut needed = 0;
    // SAFETY: token is live; the first query supplies only an output length.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenRestrictedSids,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    let error = unsafe { GetLastError() };
    if error != ERROR_INSUFFICIENT_BUFFER || needed == 0 || needed > TOKEN_BYTES {
        return Err(format!(
            "TokenRestrictedSids size_query os_error={error} needed={needed} limit={TOKEN_BYTES}"
        ));
    }
    let capacity = needed;
    let mut storage = vec![0usize; (capacity as usize).div_ceil(size_of::<usize>())];
    // SAFETY: storage is aligned, writable, bounded, and lives through decoding.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenRestrictedSids,
            storage.as_mut_ptr().cast(),
            capacity,
            &mut needed,
        )
    } == 0
    {
        return Err(format!("TokenRestrictedSids query os_error={}", unsafe {
            GetLastError()
        }));
    }
    if needed > capacity {
        return Err("TokenRestrictedSids invalid returned size".to_string());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), needed as usize) };
    let offset = std::mem::offset_of!(TOKEN_GROUPS, Groups);
    if bytes.len() < offset {
        return Err("TokenRestrictedSids truncated header".to_string());
    }
    let count = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<u32>()) } as usize;
    if count > (bytes.len() - offset) / size_of::<SID_AND_ATTRIBUTES>() {
        return Err("TokenRestrictedSids truncated entries".to_string());
    }
    let mut groups = Vec::with_capacity(count);
    for index in 0..count {
        let entry = unsafe {
            std::ptr::read_unaligned(
                bytes
                    .as_ptr()
                    .add(offset + index * size_of::<SID_AND_ATTRIBUTES>())
                    .cast::<SID_AND_ATTRIBUTES>(),
            )
        };
        let start = (entry.Sid as usize).wrapping_sub(bytes.as_ptr() as usize);
        if bytes.len() < 8 || start > bytes.len() - 8 {
            return Err("TokenRestrictedSids invalid SID pointer".to_string());
        }
        let length = 8 + usize::from(bytes[start + 1]) * 4;
        if bytes[start] != 1
            || length > 68
            || length > bytes.len() - start
            || unsafe { IsValidSid(entry.Sid) } == 0
        {
            return Err("TokenRestrictedSids invalid SID".to_string());
        }
        groups.push(TokenGroup {
            sid: bytes[start..start + length].to_vec(),
            attributes: entry.Attributes,
        });
    }
    Ok(groups)
}

fn read_descriptor(path: &Path) -> Result<Vec<usize>, String> {
    let name = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let information =
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut needed = 0;
    // SAFETY: name is NUL terminated; this only queries the required buffer size.
    unsafe {
        GetFileSecurityW(
            name.as_ptr(),
            information,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    let error = unsafe { GetLastError() };
    if error != ERROR_INSUFFICIENT_BUFFER || needed == 0 || needed > DESCRIPTOR_BYTES {
        return Err(format!(
            "GetFileSecurityW size_query os_error={error} needed={needed} limit={DESCRIPTOR_BYTES}"
        ));
    }
    let capacity = needed;
    let mut descriptor = vec![0usize; (capacity as usize).div_ceil(size_of::<usize>())];
    // SAFETY: descriptor is aligned and writable for the declared size. Read only
    // the actual owner, group and DACL; never substitute guessed owner/group SIDs.
    if unsafe {
        GetFileSecurityW(
            name.as_ptr(),
            information,
            descriptor.as_mut_ptr().cast(),
            capacity,
            &mut needed,
        )
    } == 0
    {
        return Err(format!("GetFileSecurityW query os_error={}", unsafe {
            GetLastError()
        }));
    }
    if needed == 0 || needed > capacity {
        return Err("GetFileSecurityW invalid returned size".to_string());
    }
    Ok(descriptor)
}

fn access_check(
    token: &OwnedHandle,
    descriptor: &[usize],
    mask: u32,
) -> Result<AccessResult, String> {
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    let mut privileges = [0u64; 512];
    let mut length = size_of_val(&privileges) as u32;
    let mut granted = 0;
    let mut status = 0;
    // SAFETY: the queried descriptor and duplicate remain live. The privilege
    // buffer is aligned and bounded. Explicit masks contain no generic bits.
    let api_ok = unsafe {
        AccessCheck(
            descriptor.as_ptr().cast_mut().cast(),
            token.as_raw_handle(),
            mask,
            &mapping,
            privileges.as_mut_ptr().cast(),
            &mut length,
            &mut granted,
            &mut status,
        )
    };
    let error = unsafe { GetLastError() };
    if api_ok == 0 {
        return Err(format!(
            "AccessCheck api_ok=false os_error={error} privilege_bytes={length} limit={}",
            size_of_val(&privileges)
        ));
    }
    Ok(AccessResult {
        allowed: status != 0,
        granted,
        status_error: (status == 0).then_some(error),
    })
}

fn sid_text(bytes: &[u8]) -> String {
    // Queried SIDs have already been bounded and validated. Decode only the
    // numeric SID, avoiding account-name lookup or another native allocation.
    if bytes.len() < 8 || bytes.len() != 8 + usize::from(bytes[1]) * 4 || bytes.len() > 68 {
        return "invalid-sid".to_string();
    }
    let authority = bytes[2..8]
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    let authority = if authority >= 1u64 << 32 {
        format!("0x{authority:012x}")
    } else {
        authority.to_string()
    };
    let mut result = format!("S-{}-{authority}", bytes[0]);
    for part in bytes[8..].chunks_exact(4) {
        let _ = write!(
            result,
            "-{}",
            u32::from_le_bytes([part[0], part[1], part[2], part[3]])
        );
    }
    result
}

impl Observation {
    pub(super) fn diagnostics(&self) -> String {
        let mut report = String::from(
            "TOKEN phase=after_acl_before_launch source=selected_primary_token (not a running-child query); AccessCheck uses a temporary impersonation duplicate and owner/group/DACL only; not an operation oracle\n",
        );
        let Some(token) = &self.token else {
            report.push_str("observation=unavailable reason=observer_not_reached\n");
            return report;
        };
        match &token.user {
            Ok(sid) => {
                let _ = writeln!(report, "TokenUser sid={}", sid_text(sid));
            }
            Err(error) => {
                let _ = writeln!(report, "TokenUser=unavailable {error}");
            }
        }
        for (name, groups) in [
            ("TokenGroups", &token.groups),
            ("TokenRestrictedSids", &token.restricted),
        ] {
            match groups {
                Err(error) => {
                    let _ = writeln!(report, "{name}=unavailable {error}");
                }
                Ok(groups) => {
                    let _ = writeln!(
                        report,
                        "{name} count={} retained={} truncated={}",
                        groups.len(),
                        groups.len().min(GROUP_ROWS),
                        groups.len() > GROUP_ROWS
                    );
                    if name == "TokenRestrictedSids" {
                        report.push_str(
                            "restricting SIDs are always enabled; attributes below are raw\n",
                        );
                    }
                    for group in groups.iter().take(GROUP_ROWS) {
                        let _ = writeln!(
                            report,
                            "sid={} attributes=0x{:08x}",
                            sid_text(&group.sid),
                            group.attributes,
                        );
                        if name == "TokenGroups" {
                            let _ = writeln!(
                                report,
                                "enabled={} deny_only={}",
                                group.attributes & 0x4 != 0,
                                group.attributes & 0x10 != 0
                            );
                        }
                    }
                }
            }
        }
        let _ = writeln!(
            report,
            "existing_write_roots count={} inspected={} truncated={}; sid=None means no matching metadata, not absent token membership",
            token.root_count,
            token.root_count.min(ROOT_ROWS),
            token.root_count > ROOT_ROWS
        );
        for (label, sid) in &token.roots {
            let membership = token.restricted.as_ref().map(|groups| {
                sid.as_ref()
                    .map(|sid| groups.iter().any(|group| sid_text(&group.sid) == *sid))
            });
            let _ = writeln!(
                report,
                "existing_write_root={label} sid={sid:?} restricted_membership={membership:?}"
            );
        }
        for (path, name, mask, result) in &self.checks {
            match result {
                Ok(result) => {
                    let _ = writeln!(
                        report,
                        "{path} right={name} desired=0x{mask:08x} api_ok=true access_status={} granted=0x{:08x} status_error={:?}",
                        result.allowed, result.granted, result.status_error
                    );
                }
                Err(error) => {
                    let _ = writeln!(
                        report,
                        "{path} right={name} desired=0x{mask:08x} observation=unavailable {error}"
                    );
                }
            }
        }
        if report.len() > REPORT_BYTES {
            let mut end = REPORT_BYTES - 32;
            while !report.is_char_boundary(end) {
                end -= 1;
            }
            report.truncate(end);
            report.push_str("\nreport_truncated=true\n");
        }
        report
    }
}

#[test]
fn observer_matches_exact_cwd_and_consumes_once() {
    let observer = register(Path::new("fixture"));
    assert!(take_request(Path::new("other/workspace")).is_none());
    assert!(take_request(Path::new("fixture/workspace")).is_some());
    assert!(take_request(Path::new("fixture/workspace")).is_none());
    assert!(observer.take().token.is_none());
}

#[test]
fn dropping_observer_clears_only_its_own_registration() {
    let first = register(Path::new("first"));
    let second = register(Path::new("second"));
    drop(first);
    assert!(take_request(Path::new("second/workspace")).is_some());
    drop(second);
    let third = register(Path::new("third"));
    drop(third);
    assert!(take_request(Path::new("third/workspace")).is_none());
}

#[test]
fn numeric_sid_format_preserves_authority_and_subauthorities() {
    pretty_assertions::assert_eq!(
        [
            sid_text(&[1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 33, 2, 0, 0]),
            sid_text(&[1, 1, 1, 0, 0, 0, 0, 0, 21, 0, 0, 0])
        ],
        ["S-1-5-32-545", "S-1-0x010000000000-21"]
    );
}

#[test]
fn reports_keep_api_failure_distinct_from_access_denial_and_bound_text() {
    let mut observation = Observation {
        token: Some(TokenObservation {
            user: Err("query failed".to_string()),
            groups: Ok(Vec::new()),
            restricted: Ok(vec![TokenGroup {
                sid: vec![1, 1, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0],
                attributes: 0,
            }]),
            root_count: 0,
            roots: Vec::new(),
        }),
        checks: vec![
            (
                "workspace",
                "DELETE",
                DELETE,
                Ok(AccessResult {
                    allowed: false,
                    granted: 0,
                    status_error: Some(5),
                }),
            ),
            (
                "outside",
                "DELETE",
                DELETE,
                Err("AccessCheck api_ok=false os_error=5".to_string()),
            ),
        ],
    };
    let report = observation.diagnostics();
    assert!(report.contains("TokenUser=unavailable query failed"));
    assert!(report.contains("TokenRestrictedSids count=1 retained=1 truncated=false\nrestricting SIDs are always enabled; attributes below are raw\nsid=S-1-5-21 attributes=0x00000000\nexisting_write_roots"));
    assert!(report.contains("workspace right=DELETE desired=0x00010000 api_ok=true access_status=false granted=0x00000000 status_error=Some(5)"));
    assert!(report.contains("outside right=DELETE desired=0x00010000 observation=unavailable AccessCheck api_ok=false os_error=5"));
    let Some(token) = observation.token.as_mut() else {
        panic!("test requires a token observation");
    };
    token.user = Err("\u{e9}".repeat(REPORT_BYTES));
    let report = observation.diagnostics();
    assert!(report.len() <= REPORT_BYTES);
    assert!(report.ends_with("report_truncated=true\n"));
}
