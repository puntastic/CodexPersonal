use codex_protocol::config_types::SqliteRolloutImport;
use codex_utils_path::normalize_for_path_comparison;
use std::io;
use std::path::Path;

/// Keep the opt-in away from the ordinary profile and honor managed home selection.
pub(super) fn validate(
    policy: SqliteRolloutImport,
    configured_home: Option<&Path>,
    resolved_home: &Path,
    codex_home: &Path,
) -> io::Result<()> {
    if policy == SqliteRolloutImport::Enabled {
        return Ok(());
    }
    let configured_home = configured_home.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sqlite_rollout_import = \"disabled\" requires an explicit sqlite_home; CODEX_SQLITE_HOME alone is not sufficient",
        )
    })?;
    // Existing directories make alias checks meaningful before opening any DB.
    if !configured_home.is_dir() || !resolved_home.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sqlite_rollout_import = \"disabled\" requires an existing sqlite_home directory",
        ));
    }
    let configured_home = normalize_for_path_comparison(configured_home)?;
    let resolved_home = normalize_for_path_comparison(resolved_home)?;
    if configured_home != resolved_home {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sqlite_rollout_import = \"disabled\" cannot use a sqlite_home redirected by requirements",
        ));
    }
    if resolved_home == normalize_for_path_comparison(codex_home)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sqlite_rollout_import = \"disabled\" requires sqlite_home distinct from CODEX_HOME",
        ));
    }
    Ok(())
}
