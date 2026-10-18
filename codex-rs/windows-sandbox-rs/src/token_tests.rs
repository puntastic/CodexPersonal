use super::*;
use pretty_assertions::assert_eq;
use windows_sys::Win32::Security::EqualSid;
use windows_sys::Win32::Security::TokenRestrictedSids;

unsafe fn token_has_restricting_sid(token: HANDLE, expected_sid: *mut c_void) -> Result<bool> {
    let mut needed = 0;
    GetTokenInformation(
        token,
        TokenRestrictedSids,
        std::ptr::null_mut(),
        0,
        &mut needed,
    );
    if needed == 0 {
        return Err(anyhow!(
            "GetTokenInformation(TokenRestrictedSids) size query failed: {}",
            GetLastError()
        ));
    }

    let mut buffer = vec![0_u8; needed as usize];
    if GetTokenInformation(
        token,
        TokenRestrictedSids,
        buffer.as_mut_ptr().cast(),
        needed,
        &mut needed,
    ) == 0
    {
        return Err(anyhow!(
            "GetTokenInformation(TokenRestrictedSids) failed: {}",
            GetLastError()
        ));
    }

    let group_count = std::ptr::read_unaligned(buffer.as_ptr().cast::<u32>()) as usize;
    let after_count = buffer.as_ptr().add(std::mem::size_of::<u32>()) as usize;
    let align = std::mem::align_of::<SID_AND_ATTRIBUTES>();
    let entries_addr = (after_count + (align - 1)) & !(align - 1);
    let restricting_sids =
        std::slice::from_raw_parts(entries_addr as *const SID_AND_ATTRIBUTES, group_count);
    Ok(restricting_sids
        .iter()
        .any(|entry| EqualSid(entry.Sid, expected_sid) != 0))
}

#[test]
fn elevated_token_includes_network_proxy_restricting_sid() -> Result<()> {
    let capability_sid = LocalSid::from_string("S-1-5-21-10-20-30-40")?;
    let network_proxy_sid = LocalSid::from_string("S-1-5-21-50-60-70-80")?;
    let base_token = unsafe { get_current_token_for_restriction()? };
    let restricted_token = unsafe {
        create_readonly_token_with_caps_and_user_from(
            base_token,
            &[capability_sid.as_ptr()],
            &[network_proxy_sid.as_ptr()],
        )?
    };

    let has_network_proxy_sid =
        unsafe { token_has_restricting_sid(restricted_token, network_proxy_sid.as_ptr()) };
    unsafe {
        CloseHandle(restricted_token);
        CloseHandle(base_token);
    }

    assert!(has_network_proxy_sid?);
    Ok(())
}

#[test]
fn default_objects_deny_other_logons_even_with_the_same_owner() -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::io::FromRawHandle;
    use std::os::windows::io::OwnedHandle;
    use windows_sys::Win32::Security::ACCESS_ALLOWED_ACE;
    use windows_sys::Win32::Security::AccessCheck;
    use windows_sys::Win32::Security::DuplicateToken;
    use windows_sys::Win32::Security::GENERIC_MAPPING;
    use windows_sys::Win32::Security::GetAce;
    use windows_sys::Win32::Security::InitializeSecurityDescriptor;
    use windows_sys::Win32::Security::MapGenericMask;
    use windows_sys::Win32::Security::SECURITY_DESCRIPTOR;
    use windows_sys::Win32::Security::SecurityImpersonation;
    use windows_sys::Win32::Security::SetSecurityDescriptorDacl;
    use windows_sys::Win32::Security::SetSecurityDescriptorGroup;
    use windows_sys::Win32::Security::SetSecurityDescriptorOwner;
    use windows_sys::Win32::Storage::FileSystem::READ_CONTROL;
    use windows_sys::Win32::Storage::FileSystem::WRITE_DAC;
    use windows_sys::Win32::System::Threading::PROCESS_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::PROCESS_CREATE_THREAD;
    use windows_sys::Win32::System::Threading::PROCESS_QUERY_INFORMATION;
    use windows_sys::Win32::System::Threading::PROCESS_TERMINATE;
    use windows_sys::Win32::System::Threading::PROCESS_VM_OPERATION;
    use windows_sys::Win32::System::Threading::PROCESS_VM_READ;
    use windows_sys::Win32::System::Threading::PROCESS_VM_WRITE;

    unsafe {
        let base = OwnedHandle::from_raw_handle(get_current_token_for_restriction()? as _);
        let base_raw = base.as_raw_handle() as HANDLE;
        let capability = LocalSid::from_string("S-1-5-21-10-20-30-40")?;
        let victim =
            OwnedHandle::from_raw_handle(create_workspace_write_token_with_caps_and_user_from(
                base_raw,
                &[capability.as_ptr()],
                &[],
            )? as _);
        let mut needed = 0;
        GetTokenInformation(
            victim.as_raw_handle() as _,
            TokenDefaultDacl,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        let mut buffer = vec![0u8; needed as usize];
        ensure!(
            GetTokenInformation(
                victim.as_raw_handle() as _,
                TokenDefaultDacl,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed
            ) != 0,
            "GetTokenInformation(TokenDefaultDacl) failed: {}",
            GetLastError()
        );
        let default_dacl = std::ptr::read_unaligned(buffer.as_ptr().cast::<TokenDefaultDaclInfo>());
        let mapping = GENERIC_MAPPING {
            GenericRead: READ_CONTROL | PROCESS_VM_READ | PROCESS_QUERY_INFORMATION,
            GenericWrite: READ_CONTROL | PROCESS_VM_WRITE | PROCESS_VM_OPERATION,
            GenericExecute: READ_CONTROL,
            GenericAll: PROCESS_ALL_ACCESS,
        };
        // Object creation maps the token default DACL's generic ACE masks to
        // object-specific rights. AccessCheck expects that mapping already done.
        for index in 0..u32::from((*default_dacl.default_dacl).AceCount) {
            let mut ace = std::ptr::null_mut();
            ensure!(GetAce(default_dacl.default_dacl, index, &mut ace) != 0);
            MapGenericMask(&mut (*ace.cast::<ACCESS_ALLOWED_ACE>()).Mask, &mapping);
        }
        let mut owner = get_user_sid_bytes(base_raw)?;
        let mut descriptor: SECURITY_DESCRIPTOR = std::mem::zeroed();
        let sd = std::ptr::addr_of_mut!(descriptor).cast();
        ensure!(InitializeSecurityDescriptor(sd, 1) != 0);
        ensure!(SetSecurityDescriptorOwner(sd, owner.as_mut_ptr().cast(), 0) != 0);
        ensure!(SetSecurityDescriptorGroup(sd, owner.as_mut_ptr().cast(), 0) != 0);
        ensure!(SetSecurityDescriptorDacl(sd, 1, default_dacl.default_dacl, 0) != 0);

        // Model another logon of the same account without the victim's logon
        // SID. Build from the base token, retaining the shared user, Everyone,
        // and filesystem capability in the write-restricting SID set.
        let mut logon = get_logon_sid_bytes(base_raw)?;
        let mut everyone = world_sid()?;
        let disabled = SID_AND_ATTRIBUTES {
            Sid: logon.as_mut_ptr().cast(),
            Attributes: 0,
        };
        let restricting = [
            capability.as_ptr(),
            owner.as_mut_ptr().cast(),
            everyone.as_mut_ptr().cast(),
        ]
        .map(|sid| SID_AND_ATTRIBUTES {
            Sid: sid,
            Attributes: 0,
        });
        let mut other = std::ptr::null_mut();
        ensure!(
            CreateRestrictedToken(
                base_raw,
                DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED,
                1,
                &disabled,
                0,
                std::ptr::null(),
                restricting.len() as u32,
                restricting.as_ptr(),
                &mut other
            ) != 0,
            "CreateRestrictedToken(other logon) failed: {}",
            GetLastError()
        );
        let other = OwnedHandle::from_raw_handle(other as _);
        for (token, same_logon) in [(&victim, true), (&other, false)] {
            let mut impersonation = std::ptr::null_mut();
            ensure!(
                DuplicateToken(
                    token.as_raw_handle() as _,
                    SecurityImpersonation,
                    &mut impersonation
                ) != 0,
                "DuplicateToken failed: {}",
                GetLastError()
            );
            let impersonation = OwnedHandle::from_raw_handle(impersonation as _);
            for access in [
                PROCESS_VM_READ,
                PROCESS_VM_WRITE,
                PROCESS_VM_OPERATION,
                PROCESS_CREATE_THREAD,
                PROCESS_TERMINATE,
                PROCESS_QUERY_INFORMATION,
                WRITE_DAC,
                0x43a,
            ] {
                let mut privileges = [0u64; 128];
                let mut privilege_bytes = std::mem::size_of_val(&privileges) as u32;
                let mut granted = 0;
                let mut allowed = 0;
                ensure!(
                    AccessCheck(
                        sd,
                        impersonation.as_raw_handle() as _,
                        access,
                        &mapping,
                        privileges.as_mut_ptr().cast(),
                        &mut privilege_bytes,
                        &mut granted,
                        &mut allowed
                    ) != 0,
                    "AccessCheck failed: {}",
                    GetLastError()
                );
                assert_eq!(allowed != 0, same_logon, "access {access:#x}");
            }
        }
    }
    Ok(())
}

#[test]
fn legacy_restriction_flags_and_sid_sets_access_matrix() -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::io::FromRawHandle;
    use std::os::windows::io::OwnedHandle;
    use windows_sys::Win32::Security::ACL_REVISION;
    use windows_sys::Win32::Security::AccessCheck;
    use windows_sys::Win32::Security::AddAccessAllowedAce;
    use windows_sys::Win32::Security::DuplicateToken;
    use windows_sys::Win32::Security::GENERIC_MAPPING;
    use windows_sys::Win32::Security::InitializeAcl;
    use windows_sys::Win32::Security::InitializeSecurityDescriptor;
    use windows_sys::Win32::Security::IsTokenRestricted;
    use windows_sys::Win32::Security::SECURITY_DESCRIPTOR;
    use windows_sys::Win32::Security::SecurityImpersonation;
    use windows_sys::Win32::Security::SetSecurityDescriptorDacl;
    use windows_sys::Win32::Security::SetSecurityDescriptorGroup;
    use windows_sys::Win32::Security::SetSecurityDescriptorOwner;
    use windows_sys::Win32::Storage::FileSystem::DELETE;
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::Storage::FileSystem::FILE_DELETE_CHILD;
    use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_EXECUTE;
    use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;
    use windows_sys::Win32::Storage::FileSystem::FILE_READ_DATA;
    use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_DATA;

    struct MatrixToken {
        name: &'static str,
        write_restricted: bool,
        broad_sids: bool,
        base_control: bool,
        handle: OwnedHandle,
    }
    #[derive(Clone, Copy)]
    enum Trustee {
        None,
        Capability,
        Unrelated,
        Everyone,
        Logon,
    }
    #[derive(serde::Serialize)]
    struct Row {
        token: &'static str,
        descriptor: &'static str,
        right: &'static str,
        desired: u32,
        expected: bool,
        api_ok: bool,
        access_status: Option<bool>,
        granted: Option<u32>,
        os_error: Option<u32>,
    }

    // No filesystem ACL writes or process launches. Every
    // descriptor is in memory; all four restricted tokens share one base.
    // These are mechanism predictions, including unsafe allows, not a claim
    // that a passing probe establishes a safe or usable production policy.
    unsafe {
        let base = OwnedHandle::from_raw_handle(get_current_token_for_restriction()?);
        ensure!(
            IsTokenRestricted(base.as_raw_handle()) == 0,
            "matrix requires an unrestricted base token"
        );
        let mut user = get_user_sid_bytes(base.as_raw_handle())?;
        let mut logon = get_logon_sid_bytes(base.as_raw_handle())?;
        let mut everyone = world_sid()?;
        let capability = LocalSid::from_string("S-1-5-21-10-20-30-40")?;
        let unrelated = LocalSid::from_string("S-1-5-21-50-60-70-80")?;
        let user_sid = user.as_mut_ptr().cast();
        let logon_sid = logon.as_mut_ptr().cast();
        let everyone_sid = everyone.as_mut_ptr().cast();
        let mut tokens = Vec::new();
        for (name, write_restricted, broad_sids, base_control) in [
            ("base-control", false, false, true),
            ("wr-cap", true, false, false),
            ("wr-broad", true, true, false),
            ("all-cap", false, false, false),
            ("all-broad", false, true, false),
        ] {
            let mut restricted = None;
            if !base_control {
                let mut sids = vec![capability.as_ptr()];
                if broad_sids {
                    sids.extend([logon_sid, everyone_sid]);
                }
                let entries = sids
                    .into_iter()
                    .map(|sid| SID_AND_ATTRIBUTES {
                        Sid: sid,
                        Attributes: 0,
                    })
                    .collect::<Vec<_>>();
                let flags = DISABLE_MAX_PRIVILEGE
                    | LUA_TOKEN
                    | if write_restricted {
                        WRITE_RESTRICTED
                    } else {
                        0
                    };
                let mut token = std::ptr::null_mut();
                ensure!(
                    CreateRestrictedToken(
                        base.as_raw_handle(),
                        flags,
                        /*disablesidcount*/ 0,
                        std::ptr::null(),
                        /*deleteprivilegecount*/ 0,
                        std::ptr::null(),
                        entries.len() as u32,
                        entries.as_ptr(),
                        &mut token
                    ) != 0,
                    "matrix token {name}: CreateRestrictedToken failed: {}",
                    GetLastError()
                );
                restricted = Some(OwnedHandle::from_raw_handle(token));
            }
            let source = restricted.as_ref().unwrap_or(&base);
            let mut impersonation = std::ptr::null_mut();
            ensure!(
                DuplicateToken(
                    source.as_raw_handle(),
                    SecurityImpersonation,
                    &mut impersonation
                ) != 0,
                "matrix token {name}: DuplicateToken failed: {}",
                GetLastError()
            );
            tokens.push(MatrixToken {
                name,
                write_restricted,
                broad_sids,
                base_control,
                handle: OwnedHandle::from_raw_handle(impersonation),
            });
            // The owned impersonation duplicate is sufficient for AccessCheck;
            // the fresh primary token is closed here, never installed on a thread.
        }

        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        // Match the intended per-root grant, deliberately without DELETE_CHILD.
        let root_write = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;
        let descriptors = [
            ("user-only", Trustee::None, 0),
            ("active-write", Trustee::Capability, root_write),
            ("active-read", Trustee::Capability, FILE_GENERIC_READ),
            ("unmatched-write", Trustee::Unrelated, root_write),
            ("world-full", Trustee::Everyone, FILE_ALL_ACCESS),
            ("logon-full", Trustee::Logon, FILE_ALL_ACCESS),
            ("world-read", Trustee::Everyone, FILE_GENERIC_READ),
        ];
        let rights = [
            ("READ_DATA", FILE_READ_DATA),
            ("WRITE_DATA_ADD_FILE", FILE_WRITE_DATA),
            ("WRITE_ATTRIBUTES", FILE_WRITE_ATTRIBUTES),
            ("DELETE", DELETE),
            ("DELETE_CHILD", FILE_DELETE_CHILD),
            ("ALL_ACCESS", FILE_ALL_ACCESS),
        ];
        let mut rows = Vec::new();
        let mut report_bytes = 0;
        for (descriptor_name, trustee, extra_mask) in descriptors {
            let mut acl_storage = [0u32; 128];
            let acl = acl_storage.as_mut_ptr().cast::<ACL>();
            ensure!(
                InitializeAcl(
                    acl,
                    std::mem::size_of_val(&acl_storage) as u32,
                    ACL_REVISION
                ) != 0,
                "matrix {descriptor_name}: InitializeAcl failed: {}",
                GetLastError()
            );
            ensure!(
                AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, user_sid) != 0,
                "matrix {descriptor_name}: user baseline ACE failed: {}",
                GetLastError()
            );
            let extra_sid = match trustee {
                Trustee::None => None,
                Trustee::Capability => Some(capability.as_ptr()),
                Trustee::Unrelated => Some(unrelated.as_ptr()),
                Trustee::Everyone => Some(everyone_sid),
                Trustee::Logon => Some(logon_sid),
            };
            if let Some(sid) = extra_sid {
                ensure!(
                    AddAccessAllowedAce(acl, ACL_REVISION, extra_mask, sid) != 0,
                    "matrix {descriptor_name}: extra ACE failed: {}",
                    GetLastError()
                );
            }
            let mut descriptor: SECURITY_DESCRIPTOR = std::mem::zeroed();
            let sd = std::ptr::addr_of_mut!(descriptor).cast();
            ensure!(
                InitializeSecurityDescriptor(sd, /*dwrevision*/ 1) != 0,
                "matrix {descriptor_name}: InitializeSecurityDescriptor failed: {}",
                GetLastError()
            );
            ensure!(
                SetSecurityDescriptorOwner(sd, user_sid, /*bownerdefaulted*/ 0) != 0,
                "matrix {descriptor_name}: owner failed: {}",
                GetLastError()
            );
            ensure!(
                SetSecurityDescriptorGroup(sd, user_sid, /*bgroupdefaulted*/ 0) != 0,
                "matrix {descriptor_name}: group failed: {}",
                GetLastError()
            );
            ensure!(
                SetSecurityDescriptorDacl(
                    sd, /*bdaclpresent*/ 1, acl, /*bdacldefaulted*/ 0
                ) != 0,
                "matrix {descriptor_name}: DACL failed: {}",
                GetLastError()
            );
            for token in &tokens {
                let restricting_mask = match trustee {
                    Trustee::Capability => extra_mask,
                    Trustee::Everyone | Trustee::Logon if token.broad_sids => extra_mask,
                    Trustee::None | Trustee::Unrelated | Trustee::Everyone | Trustee::Logon => 0,
                };
                for (right, desired) in rights {
                    // Prebound discriminator: WR skips the restricting side for
                    // READ_DATA and (suspected unsafe classification) DELETE_CHILD.
                    // Wrong predictions fail with all observed rows retained.
                    let expected = token.base_control
                        || (token.write_restricted
                            && matches!(desired, FILE_READ_DATA | FILE_DELETE_CHILD))
                        || restricting_mask & desired == desired;
                    let mut privileges = [0u64; 128];
                    let mut privilege_bytes = std::mem::size_of_val(&privileges) as u32;
                    let mut granted = 0;
                    let mut allowed = 0;
                    // All SID/ACL/descriptor buffers and the duplicate remain
                    // live; masks are explicit and never MAXIMUM_ALLOWED.
                    let api_ok = AccessCheck(
                        sd,
                        token.handle.as_raw_handle(),
                        desired,
                        &mapping,
                        privileges.as_mut_ptr().cast(),
                        &mut privilege_bytes,
                        &mut granted,
                        &mut allowed,
                    ) != 0;
                    let os_error = (!api_ok || allowed == 0).then(|| GetLastError());
                    let row = Row {
                        token: token.name,
                        descriptor: descriptor_name,
                        right,
                        desired,
                        expected,
                        api_ok,
                        access_status: api_ok.then_some(allowed != 0),
                        granted: api_ok.then_some(granted),
                        os_error,
                    };
                    let line = serde_json::to_string(&row)?;
                    report_bytes += line.len() + "TOKEN-MATRIX \n".len();
                    // Reserve room for the summary; emit incrementally so even
                    // a later setup/API failure retains prior observations.
                    ensure!(
                        report_bytes <= 64 * 1024 - 512,
                        "matrix report exceeds its fixed 64 KiB bound"
                    );
                    println!("TOKEN-MATRIX {line}");
                    rows.push(row);
                }
            }
        }
        let api_failures = rows.iter().filter(|row| !row.api_ok).count();
        let mismatches = rows
            .iter()
            .filter(|row| {
                row.api_ok
                    && (row.access_status != Some(row.expected)
                        || row.granted != Some(if row.expected { row.desired } else { 0 }))
            })
            .count();
        println!(
            "TOKEN-MATRIX {{\"summary\":true,\"rows\":{},\"api_failures\":{api_failures},\"prediction_mismatches\":{mismatches},\"probe_only\":true}}",
            rows.len()
        );
        assert_eq!(
            rows.len(),
            5 * 7 * 6,
            "four restricted variants plus a nonvacuous base control"
        );
        ensure!(
            api_failures == 0 && mismatches == 0,
            "matrix had {api_failures} API failures and {mismatches} prediction mismatches; inspect TOKEN-MATRIX rows"
        );
    }
    Ok(())
}
