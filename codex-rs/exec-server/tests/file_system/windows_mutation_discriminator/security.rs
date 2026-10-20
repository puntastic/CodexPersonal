//! Test-owned Windows security observations and an empty-root baseline grant.
//! Production implementation is unchanged; native fixture effects are VM-only.

use anyhow::Context;
use anyhow::Result;
use anyhow::ensure;
use serde::Serialize;
use std::ffi::c_void;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::mem::size_of;
use std::os::windows::fs::MetadataExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::ptr;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::ACCESS_ALLOWED_ACE;
use windows_sys::Win32::Security::ACE_HEADER;
use windows_sys::Win32::Security::ACL;
use windows_sys::Win32::Security::ACL_SIZE_INFORMATION;
use windows_sys::Win32::Security::AclSizeInformation;
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::Authorization::EXPLICIT_ACCESS_W;
use windows_sys::Win32::Security::Authorization::GRANT_ACCESS;
use windows_sys::Win32::Security::Authorization::GetSecurityInfo;
use windows_sys::Win32::Security::Authorization::SE_FILE_OBJECT;
use windows_sys::Win32::Security::Authorization::SetEntriesInAclW;
use windows_sys::Win32::Security::Authorization::SetSecurityInfo;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_SID;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_USER;
use windows_sys::Win32::Security::Authorization::TRUSTEE_W;
use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
use windows_sys::Win32::Security::GENERIC_MAPPING;
use windows_sys::Win32::Security::GetAce;
use windows_sys::Win32::Security::GetAclInformation;
use windows_sys::Win32::Security::GetSecurityDescriptorControl;
use windows_sys::Win32::Security::GetTokenInformation;
use windows_sys::Win32::Security::IsValidAcl;
use windows_sys::Win32::Security::IsValidSid;
use windows_sys::Win32::Security::MapGenericMask;
use windows_sys::Win32::Security::OWNER_SECURITY_INFORMATION;
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::Security::TOKEN_USER;
use windows_sys::Win32::Security::TokenUser;
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_EXECUTE;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE;
use windows_sys::Win32::Storage::FileSystem::READ_CONTROL;
use windows_sys::Win32::Storage::FileSystem::WRITE_DAC;
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::System::Threading::OpenProcessToken;

pub(super) const CHILDREN: u8 = 0x03; // OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
pub(super) const INHERITED: u8 = 0x10;

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: each constructor below receives ownership from a LocalAlloc API.
        unsafe { LocalFree(self.0) };
    }
}

fn sid_string(sid: *mut c_void) -> Result<String> {
    // SAFETY: callers pass a SID in a live OS-returned token/descriptor or bounded ACE.
    ensure!(
        !sid.is_null() && unsafe { IsValidSid(sid) } != 0,
        "invalid SID"
    );
    let mut text = ptr::null_mut();
    ensure!(
        unsafe { ConvertSidToStringSidW(sid, &mut text) } != 0,
        "convert SID: {}",
        io::Error::last_os_error()
    );
    let _text = LocalAllocation(text.cast());
    // The API returns a NUL-terminated SID string; valid Windows SIDs fit this bound.
    let length = (0..184)
        .find(|index| unsafe { *text.add(*index) } == 0)
        .context("oversized SID string")?;
    Ok(String::from_utf16(unsafe {
        std::slice::from_raw_parts(text, length)
    })?)
}

pub(super) struct CallerSid(Vec<usize>);

impl CallerSid {
    pub(super) fn read() -> Result<Self> {
        let mut token = ptr::null_mut();
        // SAFETY: query-only current-process token; successful ownership is immediate.
        ensure!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } != 0,
            "query caller token: {}",
            io::Error::last_os_error()
        );
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut length = 0;
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut length,
            )
        };
        ensure!(
            (size_of::<TOKEN_USER>() as u32..=4096).contains(&length),
            "unexpected TokenUser size"
        );
        let mut data = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        // SAFETY: aligned buffer has at least length bytes; retain it while using its SID.
        ensure!(
            unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    data.as_mut_ptr().cast(),
                    length,
                    &mut length,
                )
            } != 0,
            "read caller TokenUser: {}",
            io::Error::last_os_error()
        );
        let caller = Self(data);
        sid_string(caller.as_ptr())?;
        Ok(caller)
    }

    pub(super) fn string(&self) -> Result<String> {
        sid_string(self.as_ptr())
    }

    fn as_ptr(&self) -> *mut c_void {
        // SAFETY: read() initialized this aligned TOKEN_USER; its allocation stays owned.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct AceFact {
    ace_type: u8,
    flags: u8,
    mask: u32,
    mapped_mask: u32,
    sid: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct AclFacts {
    owner: String,
    control: u16,
    bytes: u32,
    aces: Vec<AceFact>,
}

impl AclFacts {
    pub(super) fn full_user_grant(&self, user: &str, flags: u8) -> bool {
        self.aces.iter().all(|ace| ace.ace_type == 0)
            && self.aces.iter().any(|ace| {
                ace.sid == user
                    && ace.mapped_mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS
                    && ace.flags & flags == flags
                    && ace.flags & 0x0c == 0
            })
    }
}

enum AclAccess {
    Observe,
    GrantBaseline,
}

struct Security {
    file: File,
    descriptor: LocalAllocation,
    dacl: *mut ACL,
    owner: *mut c_void,
}

impl Security {
    fn open(path: &Path, access: AclAccess) -> Result<Self> {
        let mask = match access {
            AclAccess::Observe => READ_CONTROL,
            AclAccess::GrantBaseline => READ_CONTROL | WRITE_DAC,
        };
        let file = OpenOptions::new()
            .access_mode(mask)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let mut descriptor = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut owner = ptr::null_mut();
        // SAFETY: live file handle and writable result pointers; descriptor owns returned views.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        let descriptor = LocalAllocation(descriptor);
        ensure!(
            status == 0,
            "GetSecurityInfo: {}",
            io::Error::from_raw_os_error(status as i32)
        );
        Ok(Self {
            file,
            descriptor,
            dacl,
            owner,
        })
    }

    fn facts(&self) -> Result<AclFacts> {
        // SAFETY: all native views below live in the retained descriptor.
        ensure!(
            !self.dacl.is_null() && unsafe { IsValidAcl(self.dacl) } != 0,
            "null/invalid DACL is not a qualified fixture"
        );
        let mut control = 0;
        let mut revision = 0;
        ensure!(
            unsafe { GetSecurityDescriptorControl(self.descriptor.0, &mut control, &mut revision) }
                != 0,
            "read descriptor control: {}",
            io::Error::last_os_error()
        );
        let mut info = ACL_SIZE_INFORMATION::default();
        ensure!(
            unsafe {
                GetAclInformation(
                    self.dacl,
                    (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            } != 0,
            "read ACL size: {}",
            io::Error::last_os_error()
        );
        ensure!(
            info.AceCount <= 32 && info.AclBytesInUse <= 16 * 1024,
            "ACL exceeds diagnostic bounds"
        );
        let mut aces = Vec::new();
        for index in 0..info.AceCount {
            let mut pointer = ptr::null_mut();
            ensure!(
                unsafe { GetAce(self.dacl, index, &mut pointer) } != 0 && !pointer.is_null(),
                "read ACE: {}",
                io::Error::last_os_error()
            );
            let header = unsafe { &*pointer.cast::<ACE_HEADER>() };
            // Only simple allow/deny layouts are decoded; never normalize unfamiliar ACEs.
            ensure!(
                header.AceType <= 1,
                "unsupported ACE type {}",
                header.AceType
            );
            let offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
            ensure!(usize::from(header.AceSize) >= offset + 8, "truncated ACE");
            let sid = unsafe { pointer.cast::<u8>().add(offset) };
            let sid_length = 8 + usize::from(unsafe { *sid.add(1) }) * 4;
            ensure!(
                sid_length <= 68 && sid_length <= usize::from(header.AceSize) - offset,
                "invalid ACE SID length"
            );
            let ace = unsafe { &*pointer.cast::<ACCESS_ALLOWED_ACE>() };
            let mut mapped_mask = ace.Mask;
            let mapping = GENERIC_MAPPING {
                GenericRead: FILE_GENERIC_READ,
                GenericWrite: FILE_GENERIC_WRITE,
                GenericExecute: FILE_GENERIC_EXECUTE,
                GenericAll: FILE_ALL_ACCESS,
            };
            unsafe { MapGenericMask(&mut mapped_mask, &mapping) };
            aces.push(AceFact {
                ace_type: header.AceType,
                flags: header.AceFlags,
                mask: ace.Mask,
                mapped_mask,
                sid: sid_string(sid.cast())?,
            });
        }
        Ok(AclFacts {
            owner: sid_string(self.owner)?,
            control,
            bytes: info.AclBytesInUse,
            aces,
        })
    }
}

pub(super) fn read_facts(path: &Path) -> Result<AclFacts> {
    Security::open(path, AclAccess::Observe).and_then(|security| security.facts())
}

pub(super) fn grant_baseline(root: &Path, caller: &CallerSid, before: &AclFacts) -> Result<()> {
    ensure!(
        std::fs::read_dir(root)?.next().transpose()?.is_none(),
        "baseline root must still be empty"
    );
    let security = Security::open(root, AclAccess::GrantBaseline)?;
    let metadata = security.file.metadata()?;
    ensure!(
        metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "baseline handle must be an ordinary directory"
    );
    ensure!(
        security.facts()? == *before,
        "baseline changed before grant"
    );
    ensure!(
        before.aces.iter().all(|ace| ace.ace_type == 0),
        "deny ACE is not fixture repair authority"
    );
    if before.full_user_grant(&sid_string(caller.as_ptr())?, CHILDREN) {
        return Ok(());
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: u32::from(CHILDREN),
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: caller.as_ptr().cast(),
        },
    };
    let mut acl = ptr::null_mut();
    // SAFETY: only the verified empty owned root, actual TokenUser SID, and live old ACL.
    let status = unsafe { SetEntriesInAclW(1, &entry, security.dacl, &mut acl) };
    let _acl = LocalAllocation(acl.cast());
    ensure!(status == 0, "build fixture baseline ACL: {status}");
    let status = unsafe {
        SetSecurityInfo(
            security.file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        )
    };
    ensure!(status == 0, "set fixture baseline ACL: {status}");
    Ok(())
}
