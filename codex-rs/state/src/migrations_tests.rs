use codex_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;
use sqlx::Connection;
use sqlx::Row;
use sqlx::migrate::Migration;
use sqlx::migrate::Migrator;
use std::borrow::Cow;
#[cfg(windows)]
use std::fmt::Write as _;

use super::HISTORY_MODE_MIGRATION_VERSION;
use super::STATE_MIGRATOR;
use super::THREAD_HISTORY_MIGRATOR;
#[cfg(windows)]
use super::migration_with_windows_crlf;
#[cfg(windows)]
use super::normalize_windows_crlf;
use super::repair_legacy_recency_migration_version;
use super::runtime_state_migrator;
use crate::PINNED_THREAD_SECTION_ID;
use crate::PINNED_THREAD_SECTION_NAME;

const CUSTOM_THREAD_SECTION_ID: &str = "01984de2-8f74-7c91-a3b2-5c5e937cf317";
const COLLISION_THREAD_ID: &str = "00000000-0000-0000-0000-000000000053";
const COLLISION_THREAD_RECENCY_AT_MS: i64 = 1_700_000_123_456;

#[derive(Clone, Copy)]
enum CollisionStartingState {
    Fresh,
    Upstream,
    OldFork,
    CurrentFork,
}

fn owned_migrator_like(template: &Migrator, migrations: Vec<Migration>) -> Migrator {
    Migrator {
        migrations: Cow::Owned(migrations),
        ignore_missing: template.ignore_missing,
        locking: template.locking,
        table_name: template.table_name.clone(),
        create_schemas: template.create_schemas.clone(),
        no_tx: template.no_tx,
    }
}

fn migrator_through_from(template: &Migrator, version: i64) -> Migrator {
    owned_migrator_like(
        template,
        template
            .migrations
            .iter()
            .filter(|migration| migration.version <= version)
            .cloned()
            .collect(),
    )
}

fn migrator_through(version: i64) -> Migrator {
    migrator_through_from(&STATE_MIGRATOR, version)
}

fn fork_migrator_with_history_mode_at(template: &Migrator, legacy_version: i64) -> Migrator {
    let history_mode_migration = template
        .migrations
        .iter()
        .find(|migration| migration.version == HISTORY_MODE_MIGRATION_VERSION)
        .expect("canonical history-mode migration should exist");
    let mut migrations = template
        .migrations
        .iter()
        .filter(|migration| migration.version < legacy_version)
        .cloned()
        .collect::<Vec<_>>();
    migrations.push(Migration::new(
        legacy_version,
        history_mode_migration.description.clone(),
        history_mode_migration.migration_type,
        history_mode_migration.sql.clone(),
        history_mode_migration.no_tx,
    ));
    owned_migrator_like(template, migrations)
}

async fn insert_collision_test_thread(pool: &sqlx::SqlitePool, history_mode: &str) {
    sqlx::query(
        r#"
INSERT INTO projects (id, name, position, created_at_ms, updated_at_ms)
VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind("project-53")
    .bind("Migration collision project")
    .bind(0_i64)
    .bind(1_700_000_000_000_i64)
    .bind(1_700_000_123_456_i64)
    .execute(pool)
    .await
    .expect("collision test project should insert");
    sqlx::query(
        r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    recency_at,
    created_at_ms,
    updated_at_ms,
    recency_at_ms,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode,
    history_mode,
    project_id
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(COLLISION_THREAD_ID)
    .bind("/tmp/migration-collision.jsonl")
    .bind(1_700_000_000_i64)
    .bind(1_700_000_123_i64)
    .bind(1_700_000_123_i64)
    .bind(1_700_000_000_000_i64)
    .bind(COLLISION_THREAD_RECENCY_AT_MS)
    .bind(COLLISION_THREAD_RECENCY_AT_MS)
    .bind("cli")
    .bind("openai")
    .bind("/tmp")
    .bind("")
    .bind("read-only")
    .bind("on-request")
    .bind(history_mode)
    .bind("project-53")
    .execute(pool)
    .await
    .expect("collision test thread should insert");
}

#[cfg(windows)]
#[test]
fn runtime_migrations_use_canonical_crlf_on_windows() {
    let template = STATE_MIGRATOR
        .migrations
        .first()
        .expect("state migration should exist");
    let lf_migration = Migration::new(
        1,
        Cow::Borrowed("line endings"),
        template.migration_type,
        sqlx::SqlStr::from_static("SELECT 1;\nSELECT 2;\n"),
        template.no_tx,
    );
    let crlf_migration = Migration::new(
        1,
        Cow::Borrowed("line endings"),
        template.migration_type,
        sqlx::SqlStr::from_static("SELECT 1;\r\nSELECT 2;\r\n"),
        template.no_tx,
    );

    let normalized_lf = migration_with_windows_crlf(&lf_migration);
    let normalized_crlf = migration_with_windows_crlf(&crlf_migration);

    assert_eq!(normalized_lf.sql, crlf_migration.sql);
    assert_eq!(normalized_lf.checksum, crlf_migration.checksum);
    assert_eq!(normalized_crlf.sql, crlf_migration.sql);
    assert_eq!(normalized_crlf.checksum, crlf_migration.checksum);
    assert_eq!(
        normalize_windows_crlf("one\r\ntwo\nthree\rfour"),
        "one\r\ntwo\r\nthree\r\nfour"
    );

    let runtime = runtime_state_migrator();
    let state_v1 = runtime
        .migrations
        .iter()
        .find(|migration| migration.version == 1)
        .expect("state migration v1 should exist");
    let mut state_v1_checksum = String::with_capacity(state_v1.checksum.len() * 2);
    for byte in state_v1.checksum.iter() {
        write!(&mut state_v1_checksum, "{byte:02X}")
            .expect("writing a checksum byte to a String should succeed");
    }
    assert_eq!(
        state_v1_checksum,
        "54BBD6F47905A4E4C674034575963D82DA7B534E66E9A37A81EC2AFB6A4B56CE6DE9B3ECF3032796A800F650239847D4"
    );
    for (actual, embedded) in runtime
        .migrations
        .iter()
        .zip(STATE_MIGRATOR.migrations.iter())
    {
        let expected = migration_with_windows_crlf(embedded);
        assert_eq!(actual.sql, expected.sql);
        assert_eq!(actual.checksum, expected.checksum);
    }
}

#[cfg(not(windows))]
#[test]
fn runtime_migrations_preserve_embedded_bytes_off_windows() {
    let runtime = runtime_state_migrator();

    assert!(matches!(&runtime.migrations, Cow::Borrowed(_)));
    assert_eq!(runtime.migrations.len(), STATE_MIGRATOR.migrations.len());
    for (actual, embedded) in runtime
        .migrations
        .iter()
        .zip(STATE_MIGRATOR.migrations.iter())
    {
        assert_eq!(actual.version, embedded.version);
        assert_eq!(actual.description, embedded.description);
        assert_eq!(actual.migration_type, embedded.migration_type);
        assert_eq!(actual.sql, embedded.sql);
        assert_eq!(actual.checksum, embedded.checksum);
        assert_eq!(actual.no_tx, embedded.no_tx);
    }
}

#[test]
fn history_mode_migration_keeps_deployed_checksum() {
    let runtime = runtime_state_migrator();
    let migration = runtime
        .migrations
        .iter()
        .find(|migration| migration.version == HISTORY_MODE_MIGRATION_VERSION)
        .expect("canonical history-mode migration should exist");
    let mut checksum = String::with_capacity(migration.checksum.len() * 2);
    for byte in migration.checksum.iter() {
        write!(&mut checksum, "{byte:02X}")
            .expect("writing a checksum byte to a String should succeed");
    }

    #[cfg(windows)]
    assert_eq!(
        checksum,
        "72B662B90B6907C514CBC0C46F3BD5267B4F07815AFF6ED418F03E32B38724CD31C3B6235BEC62C06586A7A50E0B97ED"
    );
    #[cfg(not(windows))]
    assert_eq!(
        checksum,
        "028EB64082777A8C6F3618131760376A5ED7BE7616EF51FBC6EFE6912BCEAA420A02706EED7DB497933901E80F14D4EC"
    );
}

#[tokio::test]
async fn history_mode_family_migration_normalizes_reference_generation() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let pool = sqlite
        .open_read_write_pool(&sqlite.state_db_path())
        .await
        .expect("sqlite database should open");
    migrator_through(/*version*/ 52)
        .run(&pool)
        .await
        .expect("pre-normalization migrations should apply");

    for (thread_id, history_mode) in [
        ("00000000-0000-0000-0000-000000000001", "legacy"),
        ("00000000-0000-0000-0000-000000000002", "paginated"),
        ("00000000-0000-0000-0000-000000000003", "paginated_refs_v1"),
    ] {
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    created_at_ms,
    updated_at_ms,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode,
    history_mode
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(thread_id)
        .bind(format!("/tmp/{thread_id}.jsonl"))
        .bind(1_700_000_000_i64)
        .bind(1_700_000_000_i64)
        .bind(1_700_000_000_000_i64)
        .bind(1_700_000_000_000_i64)
        .bind("cli")
        .bind("openai")
        .bind("/tmp")
        .bind("")
        .bind("read-only")
        .bind("on-request")
        .bind(history_mode)
        .execute(&pool)
        .await
        .expect("pre-normalization thread should insert");
    }

    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("history family normalization should apply");

    let history_modes =
        sqlx::query_as::<_, (String, String)>("SELECT id, history_mode FROM threads ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("normalized history families should load");
    assert_eq!(
        history_modes,
        vec![
            (
                "00000000-0000-0000-0000-000000000001".to_string(),
                "legacy".to_string(),
            ),
            (
                "00000000-0000-0000-0000-000000000002".to_string(),
                "paginated".to_string(),
            ),
            (
                "00000000-0000-0000-0000-000000000003".to_string(),
                "paginated".to_string(),
            ),
        ]
    );

    pool.close().await;
}

async fn assert_collision_state_converges(starting_state: CollisionStartingState) {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let state_path = sqlite.state_db_path();
    let runtime_migrator = runtime_state_migrator();

    match starting_state {
        CollisionStartingState::Fresh => {}
        CollisionStartingState::Upstream
        | CollisionStartingState::OldFork
        | CollisionStartingState::CurrentFork => {
            let pool = sqlite
                .open_read_write_pool(&state_path)
                .await
                .expect("sqlite database should open");
            migrator_through_from(&runtime_migrator, 51)
                .run(&pool)
                .await
                .expect("pre-collision migrations should apply");
            insert_collision_test_thread(&pool, "paginated_refs_v1").await;

            let starting_migrator = match starting_state {
                CollisionStartingState::Upstream => migrator_through_from(&runtime_migrator, 53),
                CollisionStartingState::OldFork => {
                    fork_migrator_with_history_mode_at(&runtime_migrator, 52)
                }
                CollisionStartingState::CurrentFork => {
                    fork_migrator_with_history_mode_at(&runtime_migrator, 53)
                }
                CollisionStartingState::Fresh => unreachable!("fresh state was handled above"),
            };
            starting_migrator
                .run(&pool)
                .await
                .expect("starting migration state should apply");
            if matches!(starting_state, CollisionStartingState::Upstream) {
                sqlx::query("UPDATE threads SET originator = ? WHERE id = ?")
                    .bind("upstream-originator")
                    .bind(COLLISION_THREAD_ID)
                    .execute(&pool)
                    .await
                    .expect("upstream originator should be recorded");
            }
            pool.close().await;
        }
    }

    let pool = sqlite
        .open_state_db(&runtime_migrator, /*telemetry_override*/ None)
        .await
        .expect("current state migrations should apply after collision repair");
    if matches!(starting_state, CollisionStartingState::Fresh) {
        insert_collision_test_thread(&pool, "paginated").await;
    }
    let expected_originator = match starting_state {
        CollisionStartingState::Fresh => "fresh-originator",
        CollisionStartingState::Upstream => "upstream-originator",
        CollisionStartingState::OldFork => "old-fork-originator",
        CollisionStartingState::CurrentFork => "current-fork-originator",
    };
    if !matches!(starting_state, CollisionStartingState::Upstream) {
        sqlx::query("UPDATE threads SET originator = ? WHERE id = ?")
            .bind(expected_originator)
            .bind(COLLISION_THREAD_ID)
            .execute(&pool)
            .await
            .expect("originator column should be writable after migration");
    }

    let applied = sqlx::query(
        r#"
SELECT version, description, success, checksum
FROM _sqlx_migrations
WHERE version >= 52
ORDER BY version
        "#,
    )
    .fetch_all(&pool)
    .await
    .expect("applied migrations should load")
    .into_iter()
    .map(|row| {
        (
            row.get::<i64, _>("version"),
            row.get::<String, _>("description"),
            row.get::<bool, _>("success"),
            row.get::<Vec<u8>, _>("checksum"),
        )
    })
    .collect::<Vec<_>>();
    let expected = runtime_migrator
        .migrations
        .iter()
        .filter(|migration| migration.version >= 52)
        .map(|migration| {
            (
                migration.version,
                migration.description.to_string(),
                true,
                migration.checksum.to_vec(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(applied, expected);

    let thread = sqlx::query_as::<_, (String, Option<String>, i64, Option<String>)>(
        "SELECT history_mode, originator, recency_at_ms, project_id FROM threads WHERE id = ?",
    )
    .bind(COLLISION_THREAD_ID)
    .fetch_one(&pool)
    .await
    .expect("collision test thread should load");
    assert_eq!(
        thread,
        (
            "paginated".to_string(),
            Some(expected_originator.to_string()),
            COLLISION_THREAD_RECENCY_AT_MS,
            Some("project-53".to_string()),
        )
    );

    let project_recency_index_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'idx_threads_project_recency'",
    )
    .fetch_one(&pool)
    .await
    .expect("project recency index should exist");
    assert_eq!(
        project_recency_index_sql
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        "CREATE INDEX idx_threads_project_recency ON threads(project_id, recency_at_ms DESC) WHERE archived = 0 AND project_id IS NOT NULL"
    );

    pool.close().await;
}

#[tokio::test]
async fn fresh_state_applies_collision_migrations_canonically() {
    assert_collision_state_converges(CollisionStartingState::Fresh).await;
}

#[tokio::test]
async fn upstream_state_preserves_versions_52_and_53() {
    assert_collision_state_converges(CollisionStartingState::Upstream).await;
}

#[tokio::test]
async fn old_fork_state_repairs_history_mode_from_version_52() {
    assert_collision_state_converges(CollisionStartingState::OldFork).await;
}

#[tokio::test]
async fn current_fork_state_repairs_history_mode_from_version_53() {
    assert_collision_state_converges(CollisionStartingState::CurrentFork).await;
}

#[tokio::test]
async fn thread_section_migration_preserves_legacy_pin_compatibility() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let state_path = sqlite.state_db_path();
    let pool = sqlite
        .open_read_write_pool(&state_path)
        .await
        .expect("sqlite database should open");
    migrator_through(/*version*/ 44)
        .run(&pool)
        .await
        .expect("released thread migrations should apply");

    for thread_id in [
        "00000000-0000-0000-0000-000000000043",
        "00000000-0000-0000-0000-000000000044",
    ] {
        if thread_id.ends_with("44") {
            sqlx::query("UPDATE threads SET is_pinned = 1 WHERE id = ?")
                .bind("00000000-0000-0000-0000-000000000043")
                .execute(&pool)
                .await
                .expect("legacy pin should remain writable before section migration");
            STATE_MIGRATOR
                .run(&pool)
                .await
                .expect("section migration should apply");
        }
        sqlx::query(
            r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    created_at_ms,
    updated_at_ms,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(thread_id)
        .bind("/tmp/legacy.jsonl")
        .bind(1_700_000_000_i64)
        .bind(1_700_000_000_i64)
        .bind(1_700_000_000_000_i64)
        .bind(1_700_000_000_000_i64)
        .bind("cli")
        .bind("openai")
        .bind("/tmp")
        .bind("")
        .bind("read-only")
        .bind("on-request")
        .execute(&pool)
        .await
        .expect("legacy thread insert should succeed");
    }

    let registered_sections = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT id, name, appearance FROM thread_sections ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("independent thread sections should load");
    assert_eq!(
        registered_sections,
        vec![(
            PINNED_THREAD_SECTION_ID.to_string(),
            PINNED_THREAD_SECTION_NAME.to_string(),
            None,
        )]
    );

    let threads = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT is_pinned, thread_section_id FROM threads ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("legacy and section-aware thread metadata should load");
    assert_eq!(threads, vec![(1, None), (0, None)]);

    sqlx::query("INSERT INTO thread_sections (id, name) VALUES (?, ?)")
        .bind(CUSTOM_THREAD_SECTION_ID)
        .bind("Custom section")
        .execute(&pool)
        .await
        .expect("custom sections should have independent persisted identities");

    let thread_id = "00000000-0000-0000-0000-000000000043";
    sqlx::query("UPDATE threads SET thread_section_id = ? WHERE id = ?")
        .bind(CUSTOM_THREAD_SECTION_ID)
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("threads should reference independently persisted sections");
    sqlx::query("UPDATE threads SET is_pinned = 0 WHERE id = ?")
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("released binaries should still update the legacy pin column");
    let thread = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT is_pinned, thread_section_id FROM threads WHERE id = ?",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("legacy pin updates should not overwrite the authoritative section");
    assert_eq!(thread, (0, Some(CUSTOM_THREAD_SECTION_ID.to_string())));

    sqlx::query("UPDATE threads SET thread_section_id = NULL WHERE id = ?")
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("threads should be removable from sections");

    let registered_sections =
        sqlx::query_as::<_, (String, String)>("SELECT id, name FROM thread_sections ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("empty sections should remain independently discoverable");
    assert_eq!(
        registered_sections,
        vec![
            (
                CUSTOM_THREAD_SECTION_ID.to_string(),
                "Custom section".to_string(),
            ),
            (
                PINNED_THREAD_SECTION_ID.to_string(),
                PINNED_THREAD_SECTION_NAME.to_string(),
            ),
        ]
    );

    let mut released_pin_migrator = migrator_through(/*version*/ 44);
    released_pin_migrator.ignore_missing = true;
    released_pin_migrator
        .run(&pool)
        .await
        .expect("released pin-capable binaries should tolerate newer migrations");

    pool.close().await;
}

#[tokio::test]
async fn thread_artifact_migration_preserves_existing_section_metadata() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let pool = sqlite
        .open_read_write_pool(&sqlite.state_db_path())
        .await
        .expect("sqlite database should open");
    migrator_through(/*version*/ 50)
        .run(&pool)
        .await
        .expect("released thread migrations should apply");
    sqlx::query("UPDATE thread_sections SET appearance = ? WHERE id = ?")
        .bind(r#"{"icon":"pin"}"#)
        .bind(PINNED_THREAD_SECTION_ID)
        .execute(&pool)
        .await
        .expect("released section appearance should remain writable");

    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("artifact migration should apply without rewriting released migrations");
    let section = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT id, name, appearance FROM thread_sections WHERE id = ?",
    )
    .bind(PINNED_THREAD_SECTION_ID)
    .fetch_one(&pool)
    .await
    .expect("existing section metadata should remain available");
    assert_eq!(
        section,
        (
            PINNED_THREAD_SECTION_ID.to_string(),
            PINNED_THREAD_SECTION_NAME.to_string(),
            Some(r#"{"icon":"pin"}"#.to_string()),
        )
    );

    let artifact_tables = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'thread_artifacts'",
    )
    .fetch_all(&pool)
    .await
    .expect("artifact table should exist");
    assert_eq!(artifact_tables, vec!["thread_artifacts"]);

    let mut released_migrator = migrator_through(/*version*/ 50);
    released_migrator.ignore_missing = true;
    released_migrator
        .run(&pool)
        .await
        .expect("released binaries should tolerate the additive artifact migration");
}

#[tokio::test]
async fn thread_section_order_migration_backfills_stably() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let pool = sqlite
        .open_read_write_pool(&sqlite.state_db_path())
        .await
        .expect("sqlite database should open");
    migrator_through(/*version*/ 45)
        .run(&pool)
        .await
        .expect("pre-ordering migrations should apply");

    sqlx::query("INSERT INTO thread_sections (id, name) VALUES (?, ?)")
        .bind(CUSTOM_THREAD_SECTION_ID)
        .bind("Custom section")
        .execute(&pool)
        .await
        .expect("custom section should exist before threads reference it");

    let older = "00000000-0000-0000-0000-000000000071";
    let newer = "00000000-0000-0000-0000-000000000072";
    let pinned = "00000000-0000-0000-0000-000000000073";
    let unsectioned = "00000000-0000-0000-0000-000000000074";
    for (thread_id, recency_at_ms, section) in [
        (older, 1_700_000_001_000_i64, Some(CUSTOM_THREAD_SECTION_ID)),
        (newer, 1_700_000_002_000, Some(CUSTOM_THREAD_SECTION_ID)),
        (pinned, 1_700_000_003_000, Some(PINNED_THREAD_SECTION_ID)),
        (unsectioned, 1_700_000_004_000, None),
    ] {
        sqlx::query(
            r#"
INSERT INTO threads (
    id, rollout_path, created_at, updated_at, recency_at,
    created_at_ms, updated_at_ms, recency_at_ms, source,
    model_provider, cwd, title, preview, sandbox_policy, approval_mode, thread_section_id
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(thread_id)
        .bind("/tmp/legacy.jsonl")
        .bind(recency_at_ms / 1000)
        .bind(recency_at_ms / 1000)
        .bind(recency_at_ms / 1000)
        .bind(recency_at_ms)
        .bind(recency_at_ms)
        .bind(recency_at_ms)
        .bind("cli")
        .bind("openai")
        .bind("/tmp")
        .bind("")
        .bind("preview")
        .bind("read-only")
        .bind("on-request")
        .bind(section)
        .execute(&pool)
        .await
        .expect("legacy section row should insert");
    }

    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("section ordering migration should apply");
    let custom_order = sqlx::query_scalar::<_, String>(
        "SELECT id FROM threads WHERE thread_section_id = ? ORDER BY section_position, id",
    )
    .bind(CUSTOM_THREAD_SECTION_ID)
    .fetch_all(&pool)
    .await
    .expect("backfilled custom order should load");
    assert_eq!(custom_order, vec![newer.to_string(), older.to_string()]);
    let positions =
        sqlx::query_scalar::<_, Option<i64>>("SELECT section_position FROM threads ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("section positions should load");
    assert_eq!(
        positions,
        vec![Some(2_000_000), Some(1_000_000), Some(1_000_000), None]
    );
    let entered = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT section_entered_at_ms FROM threads ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("section entry timestamps should load");
    assert_eq!(
        entered,
        vec![
            Some(1_700_000_001_000),
            Some(1_700_000_002_000),
            Some(1_700_000_003_000),
            None,
        ]
    );

    let section_position_index = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name = ?",
    )
    .bind("idx_threads_section_position")
    .fetch_optional(&pool)
    .await
    .expect("section position index should remain inspectable");
    assert_eq!(
        section_position_index,
        Some("idx_threads_section_position".to_string())
    );

    pool.close().await;
}

#[tokio::test]
async fn thread_item_update_ordinals_allow_older_writers() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let pre_update_ordinal_migrator = Migrator {
        migrations: Cow::Owned(
            THREAD_HISTORY_MIGRATOR
                .migrations
                .iter()
                .filter(|migration| migration.version < 4)
                .cloned()
                .collect(),
        ),
        ignore_missing: THREAD_HISTORY_MIGRATOR.ignore_missing,
        locking: THREAD_HISTORY_MIGRATOR.locking,
        table_name: THREAD_HISTORY_MIGRATOR.table_name.clone(),
        create_schemas: THREAD_HISTORY_MIGRATOR.create_schemas.clone(),
        no_tx: THREAD_HISTORY_MIGRATOR.no_tx,
    };
    let pool = sqlite
        .open_thread_history_db(
            &pre_update_ordinal_migrator,
            /*telemetry_override*/ None,
        )
        .await
        .expect("pre-update-ordinal migrations should apply");
    sqlx::query(
        r#"
INSERT INTO thread_items (thread_id, turn_id, item_id, rollout_ordinal, created_at_ms, item_type, item_json) VALUES
    ('thread-1', 'turn-1', 'existing-item-1', 11, 1_100, 'userMessage', '{}'),
    ('thread-1', 'turn-1', 'existing-item-2', 12, 1_200, 'userMessage', '{}')
        "#,
    )
    .execute(&pool)
    .await
    .expect("pre-migration items should be inserted");
    THREAD_HISTORY_MIGRATOR
        .run(&pool)
        .await
        .expect("update-ordinal migration should apply");
    sqlx::query(
        r#"
INSERT INTO thread_items (thread_id, turn_id, item_id, rollout_ordinal, created_at_ms, item_type, item_json) VALUES
    ('thread-1', 'turn-1', 'old-writer-item-1', 13, 1_300, 'userMessage', '{}'),
    ('thread-1', 'turn-1', 'old-writer-item-2', 14, 1_400, 'userMessage', '{}')
        "#,
    )
    .execute(&pool)
    .await
    .expect("older writers should be able to append multiple items after migration");
    let ordinals = sqlx::query_as::<_, (i64, i64)>(
        "SELECT rollout_ordinal, updated_at_ordinal FROM thread_items WHERE thread_id = ? ORDER BY rollout_ordinal",
    )
    .bind("thread-1")
    .fetch_all(&pool)
    .await
    .expect("old-writer items should load");
    assert_eq!(ordinals, vec![(11, 11), (12, 12), (13, 0), (14, 0)]);

    pool.close().await;
}

#[tokio::test]
async fn realtime_items_preserve_older_thread_history_writers() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let older_migrator = Migrator {
        migrations: Cow::Owned(
            THREAD_HISTORY_MIGRATOR
                .migrations
                .iter()
                .filter(|migration| migration.version < 5)
                .cloned()
                .collect(),
        ),
        ignore_missing: true,
        locking: THREAD_HISTORY_MIGRATOR.locking,
        table_name: THREAD_HISTORY_MIGRATOR.table_name.clone(),
        create_schemas: THREAD_HISTORY_MIGRATOR.create_schemas.clone(),
        no_tx: THREAD_HISTORY_MIGRATOR.no_tx,
    };
    let pool = sqlite
        .open_thread_history_db(&older_migrator, /*telemetry_override*/ None)
        .await
        .expect("existing thread history migrations should apply");
    sqlx::query(
        "INSERT INTO thread_items (thread_id, turn_id, item_id, rollout_ordinal, created_at_ms, item_json) VALUES ('thread-1', 'turn-1', 'existing-item', 1, 100, '{}')",
    )
    .execute(&pool)
    .await
    .expect("existing turn-scoped item should be inserted");

    THREAD_HISTORY_MIGRATOR
        .run(&pool)
        .await
        .expect("realtime item migration should apply");
    let turn_id_not_null = sqlx::query_scalar::<_, i64>(
        "SELECT \"notnull\" FROM pragma_table_info('thread_items') WHERE name = 'turn_id'",
    )
    .fetch_one(&pool)
    .await
    .expect("existing thread item schema should remain inspectable");
    assert_eq!(turn_id_not_null, 1);

    sqlx::query(
        "INSERT INTO thread_realtime_items (thread_id, item_id, rollout_ordinal, created_at_ms, item_type, item_json) VALUES ('thread-1', 'realtime-item', 2, 200, 'realtime_session_started', '{}')",
    )
    .execute(&pool)
    .await
    .expect("thread-scoped realtime item should be inserted separately");
    sqlx::query(
        "INSERT INTO thread_history_projection_state (thread_id, next_rollout_byte_offset, next_rollout_ordinal) VALUES ('thread-1', 0, 0)",
    )
    .execute(&pool)
    .await
    .expect("thread projection checkpoint should be inserted");

    let older_pool = sqlite
        .open_thread_history_db(&older_migrator, /*telemetry_override*/ None)
        .await
        .expect("older binaries should tolerate the additive realtime migration");
    sqlx::query(
        "INSERT INTO thread_items (thread_id, turn_id, item_id, rollout_ordinal, created_at_ms, item_json) VALUES ('thread-1', 'turn-1', 'older-writer-item', 3, 300, '{}')",
    )
    .execute(&older_pool)
    .await
    .expect("older binaries should continue writing ordinary turn-scoped items");
    let ordinary_items = sqlx::query_as::<_, (String, String)>(
        "SELECT item_id, turn_id FROM thread_items WHERE thread_id = ? ORDER BY rollout_ordinal",
    )
    .bind("thread-1")
    .fetch_all(&older_pool)
    .await
    .expect("older binaries should never observe turnless realtime items");
    assert_eq!(
        ordinary_items,
        vec![
            ("existing-item".to_string(), "turn-1".to_string()),
            ("older-writer-item".to_string(), "turn-1".to_string()),
        ]
    );
    sqlx::query("DELETE FROM thread_history_projection_state WHERE thread_id = ?")
        .bind("thread-1")
        .execute(&older_pool)
        .await
        .expect("older binaries should delete their known projection checkpoint");
    let remaining_realtime_items = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM thread_realtime_items WHERE thread_id = ?",
    )
    .bind("thread-1")
    .fetch_one(&pool)
    .await
    .expect("read realtime items after an older writer deleted the thread");
    assert_eq!(remaining_realtime_items, 0);

    older_pool.close().await;
    pool.close().await;
}

#[tokio::test]
async fn agent_job_tables_are_dropped_when_upgrading() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let state_path = sqlite.state_db_path();
    let pool = sqlite
        .open_read_write_pool(&state_path)
        .await
        .expect("sqlite database should open");
    migrator_through(/*version*/ 15)
        .run(&pool)
        .await
        .expect("agent job migrations should apply");

    sqlx::query(
        r#"
INSERT INTO agent_jobs (
    id,
    name,
    status,
    instruction,
    input_headers_json,
    input_csv_path,
    output_csv_path,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("job-1")
    .bind("legacy job")
    .bind("running")
    .bind("process rows")
    .bind(r#"["path"]"#)
    .bind("/tmp/input.csv")
    .bind("/tmp/output.csv")
    .bind(1_700_000_000_i64)
    .bind(1_700_000_000_i64)
    .execute(&pool)
    .await
    .expect("legacy agent job should insert");
    sqlx::query(
        r#"
INSERT INTO agent_job_items (
    job_id,
    item_id,
    row_index,
    row_json,
    status,
    result_json,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("job-1")
    .bind("item-1")
    .bind(0_i64)
    .bind(r#"{"path":"secret.csv"}"#)
    .bind("completed")
    .bind(r#"{"result":"legacy"}"#)
    .bind(1_700_000_000_i64)
    .bind(1_700_000_000_i64)
    .execute(&pool)
    .await
    .expect("legacy agent job item should insert");

    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("current migrations should apply");

    let agent_job_tables = sqlx::query_scalar::<_, String>(
        r#"
SELECT name
FROM sqlite_master
WHERE type = 'table' AND name IN ('agent_jobs', 'agent_job_items')
ORDER BY name
        "#,
    )
    .fetch_all(&pool)
    .await
    .expect("remaining agent job tables should load");
    assert_eq!(agent_job_tables, Vec::<String>::new());

    pool.close().await;
}

#[tokio::test]
async fn recency_migration_backfills_and_seeds_old_binary_inserts() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let state_path = sqlite.state_db_path();
    let pool = sqlite
        .open_read_write_pool(&state_path)
        .await
        .expect("sqlite database should open");
    migrator_through(/*version*/ 37)
        .run(&pool)
        .await
        .expect("pre-recency migrations should apply");

    sqlx::query(
        r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    created_at_ms,
    updated_at_ms,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("00000000-0000-0000-0000-000000000001")
    .bind("/tmp/first.jsonl")
    .bind(1_700_000_000_i64)
    .bind(1_700_000_100_i64)
    .bind(1_700_000_000_123_i64)
    .bind(1_700_000_100_456_i64)
    .bind("cli")
    .bind("openai")
    .bind("/tmp")
    .bind("")
    .bind("read-only")
    .bind("on-request")
    .execute(&pool)
    .await
    .expect("legacy row should insert");

    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("recency migration should apply");

    let backfilled = sqlx::query(
        "SELECT updated_at, updated_at_ms, recency_at, recency_at_ms FROM threads WHERE id = ?",
    )
    .bind("00000000-0000-0000-0000-000000000001")
    .fetch_one(&pool)
    .await
    .expect("backfilled row should load");
    assert_eq!(backfilled.get::<i64, _>("recency_at"), 1_700_000_100);
    assert_eq!(backfilled.get::<i64, _>("recency_at_ms"), 1_700_000_100_456);

    sqlx::query(
        r#"
INSERT INTO threads (
    id,
    rollout_path,
    created_at,
    updated_at,
    created_at_ms,
    updated_at_ms,
    source,
    model_provider,
    cwd,
    title,
    sandbox_policy,
    approval_mode
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("00000000-0000-0000-0000-000000000002")
    .bind("/tmp/second.jsonl")
    .bind(1_700_000_200_i64)
    .bind(1_700_000_300_i64)
    .bind(1_700_000_200_123_i64)
    .bind(1_700_000_300_456_i64)
    .bind("cli")
    .bind("openai")
    .bind("/tmp")
    .bind("")
    .bind("read-only")
    .bind("on-request")
    .execute(&pool)
    .await
    .expect("old-binary row should insert");

    let seeded = sqlx::query("SELECT recency_at, recency_at_ms FROM threads WHERE id = ?")
        .bind("00000000-0000-0000-0000-000000000002")
        .fetch_one(&pool)
        .await
        .expect("old-binary row should load");
    assert_eq!(seeded.get::<i64, _>("recency_at"), 1_700_000_300);
    assert_eq!(seeded.get::<i64, _>("recency_at_ms"), 1_700_000_300_456);

    pool.close().await;
}

#[tokio::test]
async fn repairs_recency_migration_that_was_applied_as_version_38() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let state_path = sqlite.state_db_path();
    let pool = sqlite
        .open_read_write_pool(&state_path)
        .await
        .expect("sqlite database should open");
    migrator_through(/*version*/ 37)
        .run(&pool)
        .await
        .expect("pre-recency migrations should apply");

    let recency_migration = STATE_MIGRATOR
        .migrations
        .iter()
        .find(|migration| migration.version == 39)
        .expect("recency migration should exist");
    let mut legacy_migrations = STATE_MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version <= 37)
        .cloned()
        .collect::<Vec<_>>();
    legacy_migrations.push(Migration::new(
        38,
        recency_migration.description.clone(),
        recency_migration.migration_type,
        recency_migration.sql.clone(),
        recency_migration.no_tx,
    ));
    let legacy_recency_migrator = Migrator::with_migrations(legacy_migrations);
    legacy_recency_migrator
        .run(&pool)
        .await
        .expect("legacy recency migration should apply as version 38");

    repair_legacy_recency_migration_version(&pool, &STATE_MIGRATOR)
        .await
        .expect("legacy migration history should be repaired");
    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("current migrations should apply after repair");

    let applied = sqlx::query(
        "SELECT version, checksum FROM _sqlx_migrations WHERE version >= 38 ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .expect("applied migrations should load")
    .into_iter()
    .map(|row| {
        (
            row.get::<i64, _>("version"),
            row.get::<Vec<u8>, _>("checksum"),
        )
    })
    .collect::<Vec<_>>();
    let expected = STATE_MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version >= 38)
        .map(|migration| (migration.version, migration.checksum.to_vec()))
        .collect::<Vec<_>>();
    assert_eq!(applied, expected);

    pool.close().await;
}

#[tokio::test]
async fn repair_recency_migration_succeeds_while_another_connection_holds_writer_slot() {
    let sqlite_home = crate::runtime::test_support::unique_temp_dir();
    tokio::fs::create_dir_all(&sqlite_home)
        .await
        .expect("sqlite home should be created");
    let _cleanup = scopeguard::guard(sqlite_home.clone(), |sqlite_home| {
        let _ = std::fs::remove_dir_all(sqlite_home);
    });
    let sqlite = crate::SqliteConfig::new_for_testing(sqlite_home.as_path().abs());
    let state_path = sqlite.state_db_path();
    let pool = sqlite
        .open_read_write_pool(&state_path)
        .await
        .expect("database should open");
    STATE_MIGRATOR
        .run(&pool)
        .await
        .expect("current migrations should apply");
    let read_pool = sqlite
        .open_read_only_pool(&state_path, /*busy_timeout*/ None)
        .await
        .expect("read-only pool should open");
    let mut write_connection = pool.acquire().await.expect("write connection should open");
    let write_transaction = write_connection
        .begin_with("BEGIN IMMEDIATE")
        .await
        .expect("write transaction should acquire the writer slot");

    let repair_result = repair_legacy_recency_migration_version(&read_pool, &STATE_MIGRATOR).await;

    write_transaction
        .rollback()
        .await
        .expect("write transaction should roll back");
    drop(write_connection);
    read_pool.close().await;
    pool.close().await;
    repair_result.expect("current migration history should not need the writer slot");
}
