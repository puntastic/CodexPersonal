//! Orchestrates legacy rollout migration into paginated history.
//!
//! This is the high-level migration state machine: find rollout files, decide whether each one is
//! eligible, take the maintenance and writer locks, canonicalize into a staged JSONL file,
//! project that staged file into SQLite, verify the projection, then atomically publish it.
//!
//! The important invariant is that we always leave behind either the original legacy rollout or a
//! recoverable paginated rollout. Once the rollout path is replaced, the durable `.pending`
//! journal must be enough for a later migration run to finish SQLite recovery safely.

use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::io::BufRead as StdBufRead;
use std::io::Read as StdRead;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use chrono::DateTime;
use codex_app_server_protocol::project_rollout_line;
use codex_protocol::ThreadId;
use codex_protocol::protocol::InternalSessionSource;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadSource;
use codex_rollout::CompactedHistoryResolver;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutLine;
use serde::Serialize;
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::BufWriter;
use tokio::time::Instant;

use super::LocalThreadStore;
use super::helpers::distinct_thread_metadata_title;
use super::thread_history;
use super::thread_history::ProjectedRolloutLine;
use super::thread_history::RolloutProjectionStep;
use crate::ThreadStoreError;
use crate::ThreadStoreResult;

mod canonicalizer;
mod legacy_event;
mod line_parser;
mod occurrence_ids;
mod publish;
mod rollback;
mod rollback_plan;
mod rollback_replay;
mod startup;
mod subagent;
mod telemetry;

use canonicalizer::CompactionResolution;
use canonicalizer::LegacyRolloutCanonicalizer;
use publish::MigrationRecoveryHandle;
pub(super) use publish::PreparedThreadDeleteMigrationArtifacts;
use publish::PreservedSourceFingerprint;
use publish::compressed_staged_rollout_path;
use publish::decompress_rollout_to_path;
use publish::decompressed_staged_rollout_path;
pub(super) use publish::ensure_migrated_generation_is_writable;
use publish::migration_journal_path;
use publish::pending_migration_thread_ids;
pub(super) use publish::prepare_thread_delete_migration_artifacts;
use publish::preserved_legacy_rollout_path;
use publish::publish_staged_rollout;
use publish::read_migration_journal_recovery;
use publish::remove_file_if_present;
use publish::remove_migration_journal;
use publish::remove_preserved_source_alias_if_present;
pub(super) use publish::remove_thread_delete_migration_artifacts;
use publish::retain_migration_recovery;
use publish::retained_migration_recovery_path;
use publish::retained_migration_thread_ids;
use publish::retire_preserved_source;
use publish::rewrite_subagent_history_boundary;
use publish::rewritten_staged_rollout_path;
use publish::staged_rollout_path;
use publish::sync_parent_directory;
use publish::validate_migration_recovery;
use publish::verify_migration_source_unchanged;
#[cfg(test)]
use publish::write_migration_journal;
use publish::write_migration_journal_with_recovery;
use rollback_plan::RollbackPlan;
use rollback_plan::RollbackPlanner;
use telemetry::RolloutMigrationTelemetry;
use telemetry::RolloutMigrationTrigger;

const PROJECTION_BATCH_BYTES: u64 = 256 * 1024;
// Legacy inline compaction checkpoints in production rollouts reached 21.7 MiB before reference-
// backed histories existed. Keep a bounded margin above that observed shape so migration can
// remove the amplification instead of rejecting the records it was introduced to repair.
const MAX_ROLLOUT_LINE_BYTES: usize = 24 * 1024 * 1024;

enum CanonicalizationAttempt {
    Complete {
        expected_length: u64,
        expected_ordinal: u64,
    },
    NeedsRollbackPlan,
}

enum RolloutMigrationPaths {
    Discover,
    Known(Vec<PathBuf>),
}

struct CanonicalizationSource<'a> {
    thread_id: ThreadId,
    rollout_id: ThreadId,
    source_path: &'a Path,
    staged_path: &'a Path,
    source_permissions: &'a std::fs::Permissions,
    canonical_session_meta: &'a RolloutLine,
    reserved_response_item_ids: HashSet<String>,
}

struct PublishedMigration {
    rollout_path: PathBuf,
    preserved_source_path: PathBuf,
}

/// Controls whether eligible rollouts are reported or migrated.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutMigrationMode {
    /// Report eligible files without modifying local storage.
    #[default]
    DryRun,
    /// Publish paginated rollouts and materialize their SQLite history.
    Apply,
}

/// Selection and throughput limits for a local rollout migration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolloutMigrationOptions {
    pub mode: RolloutMigrationMode,
    pub thread_ids: Vec<ThreadId>,
    /// Optional aggregate rollout I/O limit. Without one, migration runs as fast as local I/O
    /// allows while still yielding between projection-sized batches.
    pub max_mib_per_second: Option<u64>,
}

impl Default for RolloutMigrationOptions {
    fn default() -> Self {
        Self {
            mode: RolloutMigrationMode::DryRun,
            thread_ids: Vec::new(),
            max_mib_per_second: None,
        }
    }
}

/// The observable result of inspecting one local rollout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutMigrationStatus {
    Eligible,
    Migrated,
    AlreadyPaginated,
    SkippedEmpty,
    SkippedBusy,
    Failed,
}

/// A bounded explanation for why one rollout migration failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutMigrationFailureReason {
    MissingSqliteMetadata,
    InvalidSessionMetadata,
    RolloutReadFailed,
    OversizedRolloutRecord,
    MalformedRolloutRecord,
    LegacyRolloutConversionFailed,
    SqliteMaterializationFailed,
    RolloutPublishFailed,
    InterruptedMigrationRecoveryFailed,
    RecoveryStateConflict,
    Unknown,
}

/// The per-thread result of a rollout migration run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RolloutMigrationOutcome {
    pub thread_id: Option<ThreadId>,
    pub rollout_path: PathBuf,
    pub status: RolloutMigrationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<RolloutMigrationFailureReason>,
    /// Byte-exact legacy source retained across publication for older-writer reconciliation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_source_path: Option<PathBuf>,
    pub bytes_processed: u64,
    pub message: Option<String>,
}

/// The complete result of scanning active rollout files.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RolloutMigrationReport {
    pub outcomes: Vec<RolloutMigrationOutcome>,
}

/// Result of the explicit second phase that releases a preserved legacy source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservedSourceRetirementStatus {
    Retired,
    Refused,
}

/// One operator-visible receipt from preserved-source retirement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PreservedSourceRetirementOutcome {
    pub thread_id: ThreadId,
    pub rollout_path: PathBuf,
    pub preserved_source_path: PathBuf,
    pub status: PreservedSourceRetirementStatus,
    pub message: Option<String>,
}

/// Complete receipt for an explicit confirmed-quiescence retirement pass.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PreservedSourceRetirementReport {
    pub outcomes: Vec<PreservedSourceRetirementOutcome>,
}

/// Incremental progress emitted while rollout migration scans discovered paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RolloutMigrationProgress {
    pub processed_paths: usize,
    pub total_paths: usize,
    /// None means the scanned path was excluded by the requested thread filter.
    pub outcome_status: Option<RolloutMigrationStatus>,
}

struct RolloutMigrationRateLimiter {
    started_at: Instant,
    bytes_processed: u64,
    bytes_per_second: Option<u64>,
    bytes_since_yield: u64,
}

struct RolloutRecord {
    line: Option<RolloutLine>,
    byte_count: u64,
}

#[derive(Debug, Default)]
struct LogicalRolloutRecord {
    bytes: Vec<u8>,
    source_bytes: u64,
    logical_bytes: u64,
    in_string: bool,
    escaped: bool,
    repaired: bool,
    terminated: bool,
}

impl LogicalRolloutRecord {
    fn read_limit(&self) -> u64 {
        if self.logical_bytes > MAX_ROLLOUT_LINE_BYTES as u64 {
            return PROJECTION_BATCH_BYTES;
        }
        (MAX_ROLLOUT_LINE_BYTES as u64 + 1 - self.logical_bytes).clamp(1, PROJECTION_BATCH_BYTES)
    }

    fn append_chunk(&mut self, chunk: &[u8]) -> Result<bool, &'static str> {
        self.source_bytes = self
            .source_bytes
            .checked_add(chunk.len() as u64)
            .ok_or("rollout record source byte count overflow")?;

        let terminated = chunk.last() == Some(&b'\n');
        let content = if terminated {
            &chunk[..chunk.len() - 1]
        } else {
            chunk
        };
        self.scan_json_string_state(content);

        // Some historical Windows writes split a single JSON string across physical JSONL lines.
        // Repair only an unescaped LF while the lexical scanner is still inside that string. A
        // pending escape or a newline outside a string ends the physical record and is parsed
        // normally, so arbitrary malformed records can never use this path to resynchronize.
        if terminated && self.in_string && !self.escaped {
            let content = content.strip_suffix(b"\r").unwrap_or(content);
            self.append_logical_bytes(content)?;
            self.append_logical_bytes(br"\n")?;
            self.repaired = true;
            self.terminated = false;
            return Ok(false);
        }

        self.append_logical_bytes(chunk)?;
        self.terminated = terminated;
        Ok(terminated)
    }

    fn scan_json_string_state(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if *byte == b'\\' {
                    self.escaped = true;
                } else if *byte == b'"' {
                    self.in_string = false;
                }
            } else if *byte == b'"' {
                self.in_string = true;
            }
        }
    }

    fn append_logical_bytes(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        let previous_bytes = self.logical_bytes;
        self.logical_bytes = self
            .logical_bytes
            .checked_add(bytes.len() as u64)
            .ok_or("rollout record logical byte count overflow")?;
        if self.logical_bytes <= MAX_ROLLOUT_LINE_BYTES as u64 {
            self.bytes.extend_from_slice(bytes);
        } else if previous_bytes <= MAX_ROLLOUT_LINE_BYTES as u64 {
            self.bytes.clear();
        }
        Ok(())
    }

    fn finish(
        self,
        cursor: RolloutRecordCursor,
        trailing_partial: TrailingPartialPolicy,
    ) -> Result<RolloutRecord, RolloutRecordValidationError> {
        self.finish_with_parser(cursor, trailing_partial, /*strict_paginated*/ false)
    }

    fn finish_strict_paginated(
        self,
        cursor: RolloutRecordCursor,
    ) -> Result<RolloutRecord, RolloutRecordValidationError> {
        self.finish_with_parser(
            cursor,
            TrailingPartialPolicy::Reject,
            /*strict_paginated*/ true,
        )
    }

    fn finish_with_parser(
        self,
        cursor: RolloutRecordCursor,
        trailing_partial: TrailingPartialPolicy,
        strict_paginated: bool,
    ) -> Result<RolloutRecord, RolloutRecordValidationError> {
        if self.logical_bytes > MAX_ROLLOUT_LINE_BYTES as u64 {
            return Err(RolloutRecordValidationError::Oversized {
                ordinal: cursor.ordinal,
                byte_offset: cursor.byte_offset,
                observed_bytes: self.logical_bytes,
                source_bytes: self.source_bytes,
                limit_bytes: MAX_ROLLOUT_LINE_BYTES as u64,
            });
        }
        let line = parse_rollout_record_with_mode(
            &self.bytes,
            self.terminated,
            self.source_bytes,
            cursor,
            trailing_partial,
            self.repaired,
            strict_paginated,
        )?;
        Ok(RolloutRecord {
            line,
            byte_count: self.source_bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RolloutRecordCursor {
    ordinal: u64,
    byte_offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrailingPartialPolicy {
    DiscardIncomplete,
    Reject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RolloutInspectionDepth {
    MetadataOnly,
    FullLegacy,
}

impl RolloutRecordCursor {
    fn advance(&mut self, byte_count: u64) -> Result<(), &'static str> {
        self.byte_offset = self
            .byte_offset
            .checked_add(byte_count)
            .ok_or("rollout record byte offset overflow")?;
        self.ordinal = self
            .ordinal
            .checked_add(1)
            .ok_or("rollout record ordinal overflow")?;
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
enum RolloutRecordValidationError {
    Oversized {
        ordinal: u64,
        byte_offset: u64,
        observed_bytes: u64,
        source_bytes: u64,
        limit_bytes: u64,
    },
    Malformed {
        ordinal: u64,
        byte_offset: u64,
        observed_bytes: u64,
        source_bytes: u64,
        parser_error: String,
    },
}

impl RolloutRecordValidationError {
    fn failure_reason(&self) -> RolloutMigrationFailureReason {
        match self {
            Self::Oversized { .. } => RolloutMigrationFailureReason::OversizedRolloutRecord,
            Self::Malformed { .. } => RolloutMigrationFailureReason::MalformedRolloutRecord,
        }
    }

    fn bytes_processed(&self) -> u64 {
        match self {
            Self::Oversized {
                byte_offset,
                source_bytes,
                ..
            }
            | Self::Malformed {
                byte_offset,
                source_bytes,
                ..
            } => byte_offset.saturating_add(*source_bytes),
        }
    }
}

impl std::fmt::Display for RolloutRecordValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversized {
                ordinal,
                byte_offset,
                observed_bytes,
                limit_bytes,
                ..
            } => write!(
                formatter,
                "rollout record at ordinal {ordinal}, byte offset {byte_offset} is {observed_bytes} bytes, exceeding the {limit_bytes}-byte limit"
            ),
            Self::Malformed {
                ordinal,
                byte_offset,
                observed_bytes,
                parser_error,
                ..
            } => write!(
                formatter,
                "rollout record at ordinal {ordinal}, byte offset {byte_offset} is malformed ({observed_bytes} bytes): {parser_error}"
            ),
        }
    }
}

enum RolloutScanFailure {
    Read {
        error: io::Error,
        bytes_processed: u64,
    },
    InvalidSessionMetadata {
        message: String,
        bytes_processed: u64,
    },
    Record(RolloutRecordValidationError),
}

struct RolloutInspection {
    metadata: SessionMetaLine,
    bytes_processed: u64,
}

struct RolloutValidationFailure {
    failure: RolloutMigrationFailure,
    bytes_processed: u64,
    read_error_kind: Option<io::ErrorKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RolloutMigrationKind {
    Ordinary,
    Subagent,
}

struct RolloutMigrationFailure {
    reason: RolloutMigrationFailureReason,
    error: ThreadStoreError,
}

impl RolloutMigrationFailure {
    fn new(reason: RolloutMigrationFailureReason, error: ThreadStoreError) -> Self {
        Self { reason, error }
    }
}

type ClassifiedMigrationResult<T> = Result<T, RolloutMigrationFailure>;

fn with_failure_reason<T>(
    result: ThreadStoreResult<T>,
    reason: RolloutMigrationFailureReason,
) -> ClassifiedMigrationResult<T> {
    result.map_err(|error| RolloutMigrationFailure::new(reason, error))
}

impl RolloutMigrationRateLimiter {
    fn new(max_mib_per_second: Option<u64>) -> ThreadStoreResult<Self> {
        let bytes_per_second = max_mib_per_second
            .map(|rate| {
                rate.checked_mul(1024 * 1024)
                    .filter(|rate| *rate > 0)
                    .ok_or_else(|| ThreadStoreError::InvalidRequest {
                        message: "--max-mib-per-second must be a positive supported integer"
                            .to_string(),
                    })
            })
            .transpose()?;
        Ok(Self {
            started_at: Instant::now(),
            bytes_processed: 0,
            bytes_per_second,
            bytes_since_yield: 0,
        })
    }

    async fn account(&mut self, bytes: u64) {
        self.bytes_processed = self.bytes_processed.saturating_add(bytes);
        self.bytes_since_yield = self.bytes_since_yield.saturating_add(bytes);
        if self.bytes_since_yield < PROJECTION_BATCH_BYTES {
            return;
        }
        self.bytes_since_yield = 0;
        let Some(bytes_per_second) = self.bytes_per_second else {
            tokio::task::yield_now().await;
            return;
        };
        let expected =
            Duration::from_secs_f64(self.bytes_processed as f64 / bytes_per_second as f64);
        if let Some(remaining) = expected.checked_sub(self.started_at.elapsed()) {
            tokio::time::sleep(remaining).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
}

impl LocalThreadStore {
    /// Check whether startup needs to migrate legacy rollouts, then migrate in the background
    /// when it does.
    pub async fn migrate_rollouts_on_startup(&self) -> ThreadStoreResult<()> {
        startup::migrate_rollouts_on_startup(self).await
    }

    /// Release preserved legacy sources after the operator has confirmed that all older Codex
    /// writers are quiescent.
    ///
    /// This is deliberately separate from migration and startup. Each source is released only
    /// when the visible rollout uses the gated reference format and the preserved inode still
    /// matches the fingerprint captured at publication.
    pub async fn retire_preserved_rollout_sources_after_confirmed_quiescence(
        &self,
        thread_ids: Vec<ThreadId>,
    ) -> ThreadStoreResult<PreservedSourceRetirementReport> {
        let _maintenance_guard =
            codex_rollout::try_acquire_rollout_maintenance_lock(&self.config.codex_home)
                .map_err(migration_error)?
                .ok_or_else(|| ThreadStoreError::Conflict {
                    message: "rollout compression or another migration is already running"
                        .to_string(),
                })?;
        let mut retained_thread_ids = retained_migration_thread_ids(&self.config.codex_home)
            .await?
            .into_iter()
            .filter(|thread_id| matches_selection(&thread_ids, Some(*thread_id)))
            .collect::<Vec<_>>();
        retained_thread_ids.sort_by_key(ToString::to_string);

        let mut report = PreservedSourceRetirementReport::default();
        for thread_id in retained_thread_ids {
            let retained_path =
                retained_migration_recovery_path(&self.config.codex_home, thread_id);
            let Some(recovery) = read_migration_journal_recovery(&retained_path).await? else {
                continue;
            };
            validate_migration_recovery(
                &self.config.codex_home,
                thread_id,
                &retained_path,
                &recovery,
                /*selected_published_path*/ None,
                /*require_publication_fingerprint*/ true,
            )
            .await?;
            let preserved_source_path = recovery.preserved_source_path.clone();
            let recorded_published_path = recovery.published_rollout_path();
            let mut rollout_path = if tokio::fs::try_exists(recorded_published_path)
                .await
                .map_err(migration_error)?
            {
                Some(recorded_published_path.to_path_buf())
            } else {
                find_current_rollout_path(&self.config.codex_home, recorded_published_path).await?
            };
            let Some(mut current_rollout_path) = rollout_path.take() else {
                report.outcomes.push(retirement_refused_outcome(
                    thread_id,
                    recovery.rollout_path,
                    preserved_source_path,
                    "current rollout is missing; preserved source was not retired".to_string(),
                ));
                continue;
            };

            let _live_writer_guard = self.live_writer_locks.lock(thread_id).await;
            let _writer_guard = match self.writer_lock_coordinator.acquire(thread_id) {
                Ok(guard) => guard,
                Err(ThreadStoreError::Conflict { message }) => {
                    report.outcomes.push(retirement_refused_outcome(
                        thread_id,
                        current_rollout_path,
                        preserved_source_path,
                        message,
                    ));
                    continue;
                }
                Err(error) => return Err(error),
            };
            if !tokio::fs::try_exists(&current_rollout_path)
                .await
                .map_err(migration_error)?
                && let Some(moved_path) =
                    find_current_rollout_path(&self.config.codex_home, &current_rollout_path)
                        .await?
            {
                current_rollout_path = moved_path;
            }
            let metadata = match codex_rollout::read_session_meta_line(&current_rollout_path).await
            {
                Ok(metadata) => metadata,
                Err(error) => {
                    report.outcomes.push(retirement_refused_outcome(
                        thread_id,
                        current_rollout_path,
                        preserved_source_path,
                        format!("current rollout could not be validated: {error}"),
                    ));
                    continue;
                }
            };
            if metadata.meta.id != thread_id {
                report.outcomes.push(retirement_refused_outcome(
                    thread_id,
                    current_rollout_path,
                    preserved_source_path,
                    "current rollout belongs to a different thread".to_string(),
                ));
                continue;
            }
            if metadata.meta.history_mode != ThreadHistoryMode::PaginatedRefsV1 {
                report.outcomes.push(retirement_refused_outcome(
                    thread_id,
                    current_rollout_path,
                    preserved_source_path,
                    format!(
                        "current rollout is not gated paginated_refs_v1 (found {})",
                        metadata.meta.history_mode.as_str()
                    ),
                ));
                continue;
            }
            if let Err(error) = validate_migration_recovery(
                &self.config.codex_home,
                thread_id,
                &retained_path,
                &recovery,
                Some(&current_rollout_path),
                /*require_publication_fingerprint*/ true,
            )
            .await
            {
                report.outcomes.push(retirement_refused_outcome(
                    thread_id,
                    current_rollout_path,
                    preserved_source_path,
                    error.to_string(),
                ));
                continue;
            }

            match retire_preserved_source(&current_rollout_path, &retained_path).await {
                Ok(Some(retired_path)) => report.outcomes.push(PreservedSourceRetirementOutcome {
                    thread_id,
                    rollout_path: current_rollout_path,
                    preserved_source_path: retired_path,
                    status: PreservedSourceRetirementStatus::Retired,
                    message: Some(
                        "preserved legacy source retired after confirmed quiescence".to_string(),
                    ),
                }),
                Ok(None) => {}
                Err(ThreadStoreError::Conflict { message }) => {
                    report.outcomes.push(retirement_refused_outcome(
                        thread_id,
                        current_rollout_path,
                        preserved_source_path,
                        message,
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(report)
    }

    /// Inspect or migrate eligible legacy rollout files beneath active and archived sessions.
    pub async fn migrate_rollouts(
        &self,
        options: RolloutMigrationOptions,
    ) -> ThreadStoreResult<RolloutMigrationReport> {
        self.migrate_rollouts_with_progress_for_trigger(
            options,
            |_| {},
            RolloutMigrationTrigger::Manual,
            RolloutMigrationPaths::Discover,
        )
        .await
    }

    /// Inspect or migrate rollouts while reporting each discovered path after it is processed.
    pub async fn migrate_rollouts_with_progress(
        &self,
        options: RolloutMigrationOptions,
        on_progress: impl FnMut(RolloutMigrationProgress),
    ) -> ThreadStoreResult<RolloutMigrationReport> {
        self.migrate_rollouts_with_progress_for_trigger(
            options,
            on_progress,
            RolloutMigrationTrigger::Manual,
            RolloutMigrationPaths::Discover,
        )
        .await
    }

    async fn migrate_rollouts_with_progress_for_trigger(
        &self,
        options: RolloutMigrationOptions,
        mut on_progress: impl FnMut(RolloutMigrationProgress),
        trigger: RolloutMigrationTrigger,
        paths: RolloutMigrationPaths,
    ) -> ThreadStoreResult<RolloutMigrationReport> {
        let telemetry = RolloutMigrationTelemetry::new(trigger, &options);
        let result = self
            .migrate_rollouts_with_progress_inner(options, &mut on_progress, paths)
            .await;
        telemetry.finish(&result);
        result
    }

    async fn migrate_rollouts_with_progress_inner(
        &self,
        options: RolloutMigrationOptions,
        on_progress: &mut impl FnMut(RolloutMigrationProgress),
        paths: RolloutMigrationPaths,
    ) -> ThreadStoreResult<RolloutMigrationReport> {
        let mut limiter = RolloutMigrationRateLimiter::new(options.max_mib_per_second)?;
        let _maintenance_guard = match options.mode {
            RolloutMigrationMode::DryRun => None,
            RolloutMigrationMode::Apply => Some(
                codex_rollout::try_acquire_rollout_maintenance_lock(&self.config.codex_home)
                    .map_err(migration_error)?
                    .ok_or_else(|| ThreadStoreError::Conflict {
                        message: "rollout compression or another migration is already running"
                            .to_string(),
                    })?,
            ),
        };
        let mut paths = match paths {
            RolloutMigrationPaths::Discover => {
                find_all_rollout_paths(&self.config.codex_home).await?
            }
            RolloutMigrationPaths::Known(paths) => paths,
        };
        if options.mode == RolloutMigrationMode::Apply {
            let pending_thread_ids = pending_migration_thread_ids(&self.config.codex_home).await?;
            paths.sort_by_key(|path| {
                !thread_id_from_rollout_filename(path)
                    .is_some_and(|thread_id| pending_thread_ids.contains(&thread_id))
            });
        }
        // The name index is global, so read it once before either migrating or repairing names.
        let legacy_names = if options.mode == RolloutMigrationMode::Apply {
            let thread_ids = paths
                .iter()
                .filter_map(|path| thread_id_from_rollout_filename(path))
                .collect();
            codex_rollout::find_thread_names_by_ids(&self.config.codex_home, &thread_ids)
                .await
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        let total_paths = paths.len();
        let mut report = RolloutMigrationReport::default();

        for (index, path) in paths.into_iter().enumerate() {
            let outcome = self
                .migrate_rollout_path(path, &options, &legacy_names, &mut limiter)
                .await?;
            let outcome_status = outcome.as_ref().map(|outcome| outcome.status);
            if let Some(outcome) = outcome {
                report.outcomes.push(outcome);
            }
            on_progress(RolloutMigrationProgress {
                processed_paths: index + 1,
                total_paths,
                outcome_status,
            });
        }

        Ok(report)
    }

    async fn migrate_rollout_path(
        &self,
        mut path: PathBuf,
        options: &RolloutMigrationOptions,
        legacy_names: &HashMap<ThreadId, String>,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<Option<RolloutMigrationOutcome>> {
        let inspection_bytes_before = limiter.bytes_processed;
        let initial_inspection_depth = if options.thread_ids.is_empty() {
            RolloutInspectionDepth::FullLegacy
        } else {
            RolloutInspectionDepth::MetadataOnly
        };
        let mut retried_moved_path = false;
        let inspection = loop {
            let failure = match inspect_rollout_records(&path, initial_inspection_depth).await {
                Ok(inspection) => break inspection,
                Err(failure) => {
                    limiter.account(failure.bytes_processed).await;
                    failure
                }
            };
            if !retried_moved_path
                && failure.read_error_kind == Some(io::ErrorKind::NotFound)
                && let Some(current_path) =
                    find_current_rollout_path(&self.config.codex_home, &path).await?
            {
                // A different Codex process can archive or compress this rollout after path
                // discovery but before we take its writer lock. Retry the same rollout under
                // its current root/suffix before treating the missing snapshot path as failed.
                path = current_path;
                retried_moved_path = true;
                continue;
            }
            let thread_id = thread_id_from_rollout_filename(&path);
            if !matches_selection(&options.thread_ids, thread_id) {
                return Ok(None);
            }
            let initially_empty = tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.len() == 0);
            // Writers create the rollout path before SessionMeta is durable. Re-read under the
            // writer lock so a writer that just finished does not become a permanent empty skip.
            let failure = if initially_empty
                && options.mode == RolloutMigrationMode::Apply
                && let Some(thread_id) = thread_id
            {
                let _writer_guard = match self.writer_lock_coordinator.acquire(thread_id) {
                    Ok(guard) => guard,
                    Err(ThreadStoreError::Conflict { message }) => {
                        return Ok(Some(skipped_busy_outcome(
                            thread_id, path, message, /*bytes_processed*/ 0,
                        )));
                    }
                    Err(error) => return Err(error),
                };
                match inspect_rollout_records(&path, initial_inspection_depth).await {
                    Ok(inspection) => break inspection,
                    Err(failure) => {
                        limiter.account(failure.bytes_processed).await;
                        failure
                    }
                }
            } else {
                failure
            };
            let empty = tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.len() == 0);
            return Ok(Some(RolloutMigrationOutcome {
                thread_id,
                rollout_path: path,
                status: if empty {
                    RolloutMigrationStatus::SkippedEmpty
                } else {
                    RolloutMigrationStatus::Failed
                },
                failure_reason: (!empty).then_some(failure.failure.reason),
                preserved_source_path: None,
                bytes_processed: limiter
                    .bytes_processed
                    .saturating_sub(inspection_bytes_before),
                message: (!empty).then(|| failure.failure.error.to_string()),
            }));
        };
        let initial_inspection_bytes = inspection.bytes_processed;
        let metadata = inspection.metadata;
        let thread_id = metadata.meta.id;
        if !matches_selection(&options.thread_ids, Some(thread_id)) {
            return Ok(None);
        }
        limiter.account(initial_inspection_bytes).await;
        if requires_reference_migration(metadata.meta.history_mode)
            && initial_inspection_depth == RolloutInspectionDepth::MetadataOnly
        {
            let mut retried_validation_path = false;
            let full_inspection = loop {
                match inspect_rollout_records(&path, RolloutInspectionDepth::FullLegacy).await {
                    Err(validation)
                        if !retried_validation_path
                            && validation.read_error_kind == Some(io::ErrorKind::NotFound) =>
                    {
                        if let Some(current_path) =
                            find_current_rollout_path(&self.config.codex_home, &path).await?
                        {
                            path = current_path;
                            retried_validation_path = true;
                            continue;
                        }
                        break Err(validation);
                    }
                    result => break result,
                }
            };
            match full_inspection {
                Ok(inspection) => {
                    limiter.account(inspection.bytes_processed).await;
                    if inspection.metadata.meta.id != thread_id
                        || inspection.metadata.meta.history_mode != metadata.meta.history_mode
                        || inspection.metadata.meta.source != metadata.meta.source
                        || inspection.metadata.meta.thread_source != metadata.meta.thread_source
                    {
                        let bytes_processed = limiter
                            .bytes_processed
                            .saturating_sub(inspection_bytes_before);
                        return Ok(Some(skipped_busy_outcome(
                            thread_id,
                            path,
                            "rollout metadata changed during migration validation; retry"
                                .to_string(),
                            bytes_processed,
                        )));
                    }
                }
                Err(validation) => {
                    limiter.account(validation.bytes_processed).await;
                    let bytes_processed = limiter
                        .bytes_processed
                        .saturating_sub(inspection_bytes_before);
                    return Ok(Some(migration_outcome(
                        thread_id,
                        path,
                        Err(validation.failure),
                        bytes_processed,
                    )));
                }
            }
        }
        let inspection_bytes = limiter
            .bytes_processed
            .saturating_sub(inspection_bytes_before);
        let is_memory_consolidation = matches!(
            &metadata.meta.source,
            SessionSource::Internal(InternalSessionSource::MemoryConsolidation)
                | SessionSource::SubAgent(SubAgentSource::MemoryConsolidation)
        ) || matches!(
            &metadata.meta.thread_source,
            Some(ThreadSource::MemoryConsolidation)
        );
        let kind = if !is_memory_consolidation
            && (matches!(&metadata.meta.source, SessionSource::SubAgent(_))
                || matches!(&metadata.meta.thread_source, Some(ThreadSource::Subagent)))
        {
            RolloutMigrationKind::Subagent
        } else {
            RolloutMigrationKind::Ordinary
        };

        let journal_path = migration_journal_path(&self.config.codex_home, thread_id);
        if metadata.meta.history_mode == ThreadHistoryMode::Paginated {
            // Old inline-Paginated rollouts can be referenced by descendants through frozen byte
            // offsets. Rewriting even their header in place shifts those offsets, and an
            // exhaustive race-free inbound-reference inventory is unavailable. Contain that
            // format byte-for-byte in this release; the target incident corpus is Legacy.
            return Ok(Some(migration_outcome(
                thread_id,
                path,
                Ok(RolloutMigrationStatus::AlreadyPaginated),
                inspection_bytes,
            )));
        }
        let pending_published_migration = metadata.meta.history_mode
            == ThreadHistoryMode::PaginatedRefsV1
            && options.mode == RolloutMigrationMode::Apply
            && tokio::fs::try_exists(&journal_path)
                .await
                .map_err(migration_error)?;
        if metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1 {
            let bytes_before = inspection_bytes_before;
            let retained_path =
                retained_migration_recovery_path(&self.config.codex_home, thread_id);
            if options.mode == RolloutMigrationMode::Apply
                && tokio::fs::try_exists(&retained_path)
                    .await
                    .map_err(migration_error)?
            {
                let _live_writer_guard = self.live_writer_locks.lock(thread_id).await;
                let _writer_guard = match self.writer_lock_coordinator.acquire(thread_id) {
                    Ok(guard) => guard,
                    Err(ThreadStoreError::Conflict { message }) => {
                        return Ok(Some(skipped_busy_outcome(
                            thread_id,
                            path,
                            message,
                            limiter.bytes_processed.saturating_sub(bytes_before),
                        )));
                    }
                    Err(error) => return Err(error),
                };
                match verify_migration_source_unchanged(
                    &self.config.codex_home,
                    thread_id,
                    &retained_path,
                    &path,
                )
                .await
                {
                    Ok(()) => {}
                    Err(ThreadStoreError::Conflict { message }) => {
                        return Ok(Some(recovery_conflict_outcome(
                            thread_id,
                            path,
                            message,
                            limiter.bytes_processed.saturating_sub(bytes_before),
                        )));
                    }
                    Err(error) => return Err(error),
                }
            }
            let recorded_recovery = if pending_published_migration {
                read_migration_journal_recovery(&journal_path).await?
            } else {
                None
            };
            let result = if pending_published_migration {
                let _live_writer_guard = self.live_writer_locks.lock(thread_id).await;
                let _writer_guard = match self.writer_lock_coordinator.acquire(thread_id) {
                    Ok(guard) => guard,
                    Err(ThreadStoreError::Conflict { message }) => {
                        return Ok(Some(skipped_busy_outcome(
                            thread_id,
                            path,
                            message,
                            limiter.bytes_processed.saturating_sub(bytes_before),
                        )));
                    }
                    Err(error) => return Err(error),
                };
                match self
                    .recover_published_migration(
                        thread_id,
                        &path,
                        &journal_path,
                        legacy_names,
                        limiter,
                    )
                    .await
                {
                    Ok(recovered_path) => {
                        path = recovered_path;
                        Ok(RolloutMigrationStatus::Migrated)
                    }
                    Err(ThreadStoreError::Conflict { message }) => {
                        let bytes_processed = limiter.bytes_processed.saturating_sub(bytes_before);
                        return Ok(Some(recovery_conflict_outcome(
                            thread_id,
                            path,
                            message,
                            bytes_processed,
                        )));
                    }
                    Err(error) => Err(RolloutMigrationFailure::new(
                        RolloutMigrationFailureReason::InterruptedMigrationRecoveryFailed,
                        error,
                    )),
                }
            } else {
                if options.mode == RolloutMigrationMode::Apply {
                    self.promote_legacy_name(thread_id, metadata.meta.history_mode, legacy_names)
                        .await?;
                }
                Ok(RolloutMigrationStatus::AlreadyPaginated)
            };
            let bytes_processed = limiter.bytes_processed.saturating_sub(bytes_before);
            let mut outcome = migration_outcome(thread_id, path.clone(), result, bytes_processed);
            outcome.preserved_source_path = if let Some(recovery) = recorded_recovery {
                Some(recovery.preserved_source_path)
            } else if let Ok(candidate) = preserved_legacy_rollout_path(&path)
                && tokio::fs::try_exists(&candidate)
                    .await
                    .map_err(migration_error)?
            {
                Some(candidate)
            } else {
                None
            };
            return Ok(Some(outcome));
        }

        if options.mode == RolloutMigrationMode::DryRun {
            return Ok(Some(migration_outcome(
                thread_id,
                path,
                Ok(RolloutMigrationStatus::Eligible),
                inspection_bytes,
            )));
        }

        let _live_writer_guard = self.live_writer_locks.lock(thread_id).await;
        let _writer_guard = match self.writer_lock_coordinator.acquire(thread_id) {
            Ok(guard) => guard,
            Err(ThreadStoreError::Conflict { message }) => {
                let bytes_processed = limiter
                    .bytes_processed
                    .saturating_sub(inspection_bytes_before);
                return Ok(Some(skipped_busy_outcome(
                    thread_id,
                    path,
                    message,
                    bytes_processed,
                )));
            }
            Err(error) => {
                let bytes_processed = limiter
                    .bytes_processed
                    .saturating_sub(inspection_bytes_before);
                return Ok(Some(migration_outcome(
                    thread_id,
                    path,
                    Err(RolloutMigrationFailure::new(
                        RolloutMigrationFailureReason::Unknown,
                        error,
                    )),
                    bytes_processed,
                )));
            }
        };
        // SessionMeta gives us the writer-lock id, so archiving can win between that read and
        // lock acquisition. Once the lock is ours, follow the same rollout to its current path.
        if !tokio::fs::try_exists(&path)
            .await
            .map_err(migration_error)?
            && let Some(current_path) =
                find_current_rollout_path(&self.config.codex_home, &path).await?
        {
            path = current_path;
        }
        let bytes_before = inspection_bytes_before;
        match inspect_rollout_records(&path, RolloutInspectionDepth::FullLegacy).await {
            Ok(inspection) => {
                limiter.account(inspection.bytes_processed).await;
                let locked_metadata = inspection.metadata.meta;
                if locked_metadata.id != thread_id
                    || locked_metadata.history_mode != metadata.meta.history_mode
                    || !requires_reference_migration(locked_metadata.history_mode)
                    || locked_metadata.source != metadata.meta.source
                    || locked_metadata.thread_source != metadata.meta.thread_source
                {
                    let bytes_processed = limiter.bytes_processed.saturating_sub(bytes_before);
                    return Ok(Some(skipped_busy_outcome(
                        thread_id,
                        path,
                        "rollout metadata changed while migration waited for its writer lock; retry"
                            .to_string(),
                        bytes_processed,
                    )));
                }
            }
            Err(validation) => {
                limiter.account(validation.bytes_processed).await;
                let bytes_processed = limiter.bytes_processed.saturating_sub(bytes_before);
                return Ok(Some(migration_outcome(
                    thread_id,
                    path,
                    Err(validation.failure),
                    bytes_processed,
                )));
            }
        }
        let (result, mut preserved_source_path) = match self
            .migrate_one_rollout(
                thread_id,
                metadata.meta.history_mode,
                &path,
                &journal_path,
                kind,
                legacy_names,
                limiter,
            )
            .await
        {
            Ok(published) => {
                path = published.rollout_path;
                (
                    Ok(RolloutMigrationStatus::Migrated),
                    Some(published.preserved_source_path),
                )
            }
            Err(failure) => {
                let projection_was_replaced = matches!(
                    failure.reason,
                    RolloutMigrationFailureReason::SqliteMaterializationFailed
                        | RolloutMigrationFailureReason::RolloutPublishFailed
                );
                let result = if let Err(cleanup_error) = self
                    .cleanup_failed_unpublished_migration(
                        thread_id,
                        metadata.meta.history_mode,
                        projection_was_replaced,
                        &path,
                        &journal_path,
                        limiter,
                    )
                    .await
                {
                    Err(RolloutMigrationFailure::new(
                        failure.reason,
                        migration_error(format!(
                            "{}; failed to clean up unpublished migration: {cleanup_error}",
                            failure.error
                        )),
                    ))
                } else {
                    Err(failure)
                };
                (result, None)
            }
        };
        if preserved_source_path.is_none()
            && let Ok(candidate) = preserved_legacy_rollout_path(&path)
            && tokio::fs::try_exists(&candidate)
                .await
                .map_err(migration_error)?
        {
            preserved_source_path = Some(candidate);
        }
        let bytes_processed = limiter.bytes_processed.saturating_sub(bytes_before);
        let mut outcome = match result {
            Err(RolloutMigrationFailure {
                error: ThreadStoreError::Conflict { message },
                ..
            }) => skipped_busy_outcome(thread_id, path, message, bytes_processed),
            result => migration_outcome(thread_id, path, result, bytes_processed),
        };
        outcome.preserved_source_path = preserved_source_path;
        Ok(Some(outcome))
    }

    async fn migrate_one_rollout(
        &self,
        thread_id: ThreadId,
        source_history_mode: ThreadHistoryMode,
        rollout_path: &Path,
        journal_path: &Path,
        kind: RolloutMigrationKind,
        legacy_names: &HashMap<ThreadId, String>,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ClassifiedMigrationResult<PublishedMigration> {
        let rollout_id = with_failure_reason(
            thread_id_from_rollout_filename(rollout_path).ok_or_else(|| {
                migration_error(format!(
                    "rollout filename does not contain a physical rollout ID: {}",
                    rollout_path.display()
                ))
            }),
            RolloutMigrationFailureReason::InvalidSessionMetadata,
        )?;
        if let Some(state_db) = &self.state_db
            && with_failure_reason(
                state_db
                    .get_thread(thread_id)
                    .await
                    .map_err(migration_error),
                RolloutMigrationFailureReason::Unknown,
            )?
            .is_none()
        {
            return Err(RolloutMigrationFailure::new(
                RolloutMigrationFailureReason::MissingSqliteMetadata,
                migration_error(format!("thread {thread_id} is missing its SQLite metadata")),
            ));
        }

        let compressed = rollout_path_is_compressed(rollout_path);
        let published_rollout_path = if compressed {
            codex_rollout::plain_rollout_path(rollout_path)
        } else {
            rollout_path.to_path_buf()
        };
        let staged_path = with_failure_reason(
            staged_rollout_path(rollout_path),
            RolloutMigrationFailureReason::Unknown,
        )?;
        let decompressed_path = with_failure_reason(
            compressed
                .then(|| decompressed_staged_rollout_path(rollout_path))
                .transpose(),
            RolloutMigrationFailureReason::Unknown,
        )?;
        let preserved_source_path = with_failure_reason(
            preserved_legacy_rollout_path(rollout_path),
            RolloutMigrationFailureReason::RolloutPublishFailed,
        )?;
        let recovery = MigrationRecoveryHandle::new_with_published_path(
            rollout_path.to_path_buf(),
            published_rollout_path.clone(),
            preserved_source_path.clone(),
        );
        with_failure_reason(
            write_migration_journal_with_recovery(journal_path, &recovery).await,
            RolloutMigrationFailureReason::RolloutPublishFailed,
        )?;

        let source_metadata = with_failure_reason(
            tokio::fs::metadata(rollout_path)
                .await
                .map_err(migration_error),
            RolloutMigrationFailureReason::RolloutReadFailed,
        )?;
        let source_fingerprint = with_failure_reason(
            PreservedSourceFingerprint::from_metadata(&source_metadata),
            RolloutMigrationFailureReason::RolloutReadFailed,
        )?;
        let source_modified = source_metadata.modified().ok();
        let source_permissions = source_metadata.permissions();
        let source_path = if let Some(decompressed_path) = decompressed_path.as_ref() {
            with_failure_reason(
                decompress_rollout_to_path(rollout_path, decompressed_path).await,
                RolloutMigrationFailureReason::RolloutReadFailed,
            )?;
            let decompressed_bytes = with_failure_reason(
                tokio::fs::metadata(decompressed_path)
                    .await
                    .map_err(migration_error),
                RolloutMigrationFailureReason::RolloutReadFailed,
            )?
            .len();
            limiter
                .account(source_metadata.len().saturating_add(decompressed_bytes))
                .await;
            decompressed_path.as_path()
        } else {
            rollout_path
        };
        let source_file = with_failure_reason(
            File::open(source_path).await.map_err(migration_error),
            RolloutMigrationFailureReason::RolloutReadFailed,
        )?;
        let mut source = BufReader::with_capacity(PROJECTION_BATCH_BYTES as usize, source_file);
        let mut bytes = Vec::new();
        let mut cursor = RolloutRecordCursor::default();

        // Paginated rollouts keep canonical SessionMeta first (at zero for roots or the inherited
        // base end for referenced children). Legacy readers tolerate a pre-header prefix, so find
        // that metadata before replaying the source instead of buffering the prefix in memory.
        let canonical_session_meta = loop {
            let record = with_failure_reason(
                read_rollout_record(
                    &mut source,
                    &mut bytes,
                    &mut cursor,
                    TrailingPartialPolicy::DiscardIncomplete,
                )
                .await,
                RolloutMigrationFailureReason::RolloutReadFailed,
            )?
            .ok_or_else(|| {
                RolloutMigrationFailure::new(
                    RolloutMigrationFailureReason::InvalidSessionMetadata,
                    migration_error("rollout contains no session metadata"),
                )
            })?;
            limiter.account(record.byte_count).await;
            let Some(line) = record.line else {
                continue;
            };
            if matches!(&line.item, RolloutItem::SessionMeta(_)) {
                break line;
            }
        };
        drop(source);

        let reserved_response_item_ids = with_failure_reason(
            Self::scan_existing_response_item_ids(source_path, limiter).await,
            RolloutMigrationFailureReason::RolloutReadFailed,
        )?;

        let canonicalization_source = CanonicalizationSource {
            thread_id,
            rollout_id,
            source_path,
            staged_path: &staged_path,
            source_permissions: &source_permissions,
            canonical_session_meta: &canonical_session_meta,
            reserved_response_item_ids,
        };

        // Everything up through a durable staged file is one legacy-to-paginated conversion
        // phase. Keep the individual operations readable and tag the phase once if it fails.
        let conversion_result = async {
            let bounded_subagent_context = if source_history_mode == ThreadHistoryMode::Legacy
                && kind == RolloutMigrationKind::Subagent
            {
                let RolloutItem::SessionMeta(session_meta) = &canonical_session_meta.item else {
                    return Err(migration_error("canonical session metadata is missing"));
                };
                let context = subagent::select_bounded_context(
                    source_path.to_path_buf(),
                    session_meta.clone(),
                )
                .await?;
                limiter.account(source_metadata.len()).await;
                context
            } else {
                None
            };
            let (_, expected_ordinal) = if source_history_mode == ThreadHistoryMode::Paginated {
                Self::write_paginated_refs_upgrade(&canonicalization_source, limiter).await?
            } else if let Some(items) = bounded_subagent_context {
                Self::write_bounded_subagent_rollout(&canonicalization_source, items, limiter)
                    .await?
            } else {
                Self::write_rollout_with_rollback_plan(&canonicalization_source, limiter).await?
            };

            if source_history_mode == ThreadHistoryMode::Legacy
                && kind == RolloutMigrationKind::Subagent
            {
                rewrite_subagent_history_boundary(&staged_path, expected_ordinal).await?;
            }
            let expected_length = tokio::fs::metadata(&staged_path)
                .await
                .map_err(migration_error)?
                .len();

            // SQLite projection only starts after every staged-file mutation is durable.
            let modified_at = source_modified;
            let path = staged_path.clone();
            tokio::task::spawn_blocking(move || {
                let file = std::fs::OpenOptions::new().write(true).open(path)?;
                if let Some(modified_at) = modified_at {
                    file.set_times(std::fs::FileTimes::new().set_modified(modified_at))?;
                }
                file.sync_all()
            })
            .await
            .map_err(migration_error)?
            .map_err(migration_error)?;

            Ok::<_, ThreadStoreError>((expected_length, expected_ordinal))
        }
        .await;
        let (expected_length, expected_ordinal) = with_failure_reason(
            conversion_result,
            RolloutMigrationFailureReason::LegacyRolloutConversionFailed,
        )?;

        // SQLite sees only the final staged bytes, so all projection failures share one reason.
        let projection_result = async {
            // Keep an existing old-Paginated projection live while the strict pass-through
            // conversion is still being validated. Only replace it after staged bytes are
            // complete, so a malformed source cannot destroy otherwise usable projected state.
            thread_history::delete_thread(self, thread_id).await?;
            self.project_rollout_in_batches(thread_id, &staged_path, limiter)
                .await?;
            let projection = thread_history::projection_state(self, thread_id)
                .await?
                .ok_or_else(|| migration_error("completed rollout has no SQLite projection"))?;
            if projection.next_byte_offset != expected_length
                || projection.next_ordinal != expected_ordinal
            {
                return Err(migration_error(
                    "SQLite projection does not cover the complete staged rollout",
                ));
            }
            Ok::<_, ThreadStoreError>(())
        }
        .await;
        with_failure_reason(
            projection_result,
            RolloutMigrationFailureReason::SqliteMaterializationFailed,
        )?;

        // Once projection is verified, the remaining work publishes the replacement and clears
        // the durable pending journal.
        let publish_result = async {
            // Older writers do not know about migration locks. Keep the legacy path visible if
            // its append-only source changed while the replacement rollout was being staged.
            let current_source_metadata = tokio::fs::metadata(rollout_path)
                .await
                .map_err(migration_error)?;
            let current_source_fingerprint =
                PreservedSourceFingerprint::from_metadata(&current_source_metadata)?;
            if current_source_fingerprint != source_fingerprint {
                return Err(ThreadStoreError::Conflict {
                    message: "rollout changed while migration was staging it; close older Codex processes and retry".to_string(),
                });
            }

            publish_staged_rollout(
                rollout_path,
                &published_rollout_path,
                &staged_path,
                &preserved_source_path,
                journal_path,
                &source_fingerprint,
            )
            .await?;
            verify_migration_source_unchanged(
                &self.config.codex_home,
                thread_id,
                journal_path,
                &published_rollout_path,
            )
            .await?;
            if let Some(decompressed_path) = decompressed_path.as_ref() {
                remove_file_if_present(decompressed_path).await?;
            }
            self.finish_published_migration(
                thread_id,
                rollout_path,
                &published_rollout_path,
                journal_path,
                legacy_names,
            )
            .await?;
            verify_migration_source_unchanged(
                &self.config.codex_home,
                thread_id,
                &retained_migration_recovery_path(&self.config.codex_home, thread_id),
                &published_rollout_path,
            )
            .await
        }
        .await;
        with_failure_reason(
            publish_result,
            RolloutMigrationFailureReason::RolloutPublishFailed,
        )?;
        Ok(PublishedMigration {
            rollout_path: published_rollout_path,
            preserved_source_path,
        })
    }

    async fn build_rollback_plan(
        source_path: &Path,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<RollbackPlan> {
        let source_file = File::open(source_path).await.map_err(migration_error)?;
        let mut source = BufReader::with_capacity(PROJECTION_BATCH_BYTES as usize, source_file);
        let mut bytes = Vec::new();
        let mut planner = RollbackPlanner::new();
        let mut cursor = RolloutRecordCursor::default();
        while let Some(record) = read_rollout_record(
            &mut source,
            &mut bytes,
            &mut cursor,
            TrailingPartialPolicy::DiscardIncomplete,
        )
        .await?
        {
            limiter.account(record.byte_count).await;
            if let Some(line) = record.line {
                planner.observe(&line)?;
            }
        }
        Ok(planner.finish())
    }

    /// Collects the bounded ID namespace that deterministic migration IDs must not collide with.
    async fn scan_existing_response_item_ids(
        source_path: &Path,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<HashSet<String>> {
        let source_file = File::open(source_path).await.map_err(migration_error)?;
        let mut source = BufReader::with_capacity(PROJECTION_BATCH_BYTES as usize, source_file);
        let mut bytes = Vec::new();
        let mut cursor = RolloutRecordCursor::default();
        let mut item_ids = HashSet::new();
        while let Some(record) = read_rollout_record(
            &mut source,
            &mut bytes,
            &mut cursor,
            TrailingPartialPolicy::DiscardIncomplete,
        )
        .await?
        {
            limiter.account(record.byte_count).await;
            if let Some(line) = record.line {
                collect_response_item_ids(&line.item, &mut item_ids);
            }
        }
        Ok(item_ids)
    }

    async fn write_rollout_with_rollback_plan(
        input: &CanonicalizationSource<'_>,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<(u64, u64)> {
        let attempt = Self::write_canonical_rollout(input, /*plan*/ None, limiter).await?;
        match attempt {
            CanonicalizationAttempt::Complete {
                expected_length,
                expected_ordinal,
            } => Ok((expected_length, expected_ordinal)),
            CanonicalizationAttempt::NeedsRollbackPlan => {
                remove_file_if_present(input.staged_path).await?;
                let plan = Self::build_rollback_plan(input.source_path, limiter).await?;
                let CanonicalizationAttempt::Complete {
                    expected_length,
                    expected_ordinal,
                } = Self::write_canonical_rollout(input, Some(&plan), limiter).await?
                else {
                    return Err(migration_error(
                        "planned rollout still contains a rollback marker",
                    ));
                };
                Ok((expected_length, expected_ordinal))
            }
        }
    }

    /// Upgrade an already canonical paginated rollout without replaying it through legacy turn
    /// synthesis. Ordinals, timestamps, record order, lineage metadata, and non-compaction items
    /// remain unchanged; only the canonical head's format gate and safely resolvable compaction
    /// encoding change.
    async fn write_paginated_refs_upgrade(
        input: &CanonicalizationSource<'_>,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<(u64, u64)> {
        let RolloutItem::SessionMeta(source_metadata) = &input.canonical_session_meta.item else {
            return Err(migration_error(
                "paginated rollout canonical metadata is missing",
            ));
        };
        let mut next_ordinal = source_metadata
            .meta
            .history_base
            .as_ref()
            .map_or(0, |base| base.end_ordinal_exclusive);
        let source_file = File::open(input.source_path)
            .await
            .map_err(migration_error)?;
        let staged_file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(input.staged_path)
            .await
            .map_err(migration_error)?;
        staged_file
            .set_permissions(input.source_permissions.clone())
            .await
            .map_err(migration_error)?;
        let mut source = BufReader::with_capacity(PROJECTION_BATCH_BYTES as usize, source_file);
        let mut staged = BufWriter::with_capacity(PROJECTION_BATCH_BYTES as usize, staged_file);
        let mut bytes = Vec::new();
        let mut cursor = RolloutRecordCursor::default();
        let mut output_byte_offset = 0_u64;
        let mut saw_canonical_head = false;
        let mut compacted_history = CompactedHistoryResolver::default();

        while let Some(record) =
            read_strict_paginated_record(&mut source, &mut bytes, &mut cursor).await?
        {
            limiter.account(record.byte_count).await;
            let mut line = record
                .line
                .ok_or_else(|| migration_error("paginated rollout contains an empty record"))?;
            if line.ordinal != Some(next_ordinal) {
                return Err(migration_error(format!(
                    "paginated rollout ordinal changed: expected {next_ordinal}, found {:?}",
                    line.ordinal
                )));
            }
            if !saw_canonical_head {
                let RolloutItem::SessionMeta(metadata) = &mut line.item else {
                    return Err(migration_error(
                        "paginated rollout does not start with canonical session metadata",
                    ));
                };
                if metadata.meta.id != input.thread_id
                    || metadata.meta.history_mode != ThreadHistoryMode::Paginated
                {
                    return Err(migration_error(
                        "paginated rollout canonical metadata changed during upgrade",
                    ));
                }
                metadata.meta.history_mode = ThreadHistoryMode::PaginatedRefsV1;
                saw_canonical_head = true;
            } else {
                compacted_history
                    .reencode_item_with_backward_references(&mut line.item)
                    .map_err(|missing| {
                        migration_error(format!(
                            "compacted history references could not be resolved during paginated upgrade: {}",
                            missing.join(", ")
                        ))
                    })?;
            }

            let mut encoded = serde_json::to_vec(&line).map_err(migration_error)?;
            encoded.push(b'\n');
            staged.write_all(&encoded).await.map_err(migration_error)?;
            let encoded_len = u64::try_from(encoded.len())
                .map_err(|_| migration_error("rollout record exceeds addressable size"))?;
            output_byte_offset = output_byte_offset
                .checked_add(encoded_len)
                .ok_or_else(|| migration_error("paginated rollout byte offset overflow"))?;
            limiter.account(encoded_len).await;
            next_ordinal = next_ordinal
                .checked_add(1)
                .ok_or_else(|| migration_error("paginated rollout ordinal overflow"))?;
        }
        if !saw_canonical_head {
            return Err(migration_error(
                "paginated rollout contains no canonical session metadata",
            ));
        }
        staged.flush().await.map_err(migration_error)?;
        Ok((output_byte_offset, next_ordinal))
    }

    async fn write_bounded_subagent_rollout(
        input: &CanonicalizationSource<'_>,
        items: Vec<RolloutItem>,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<(u64, u64)> {
        let staged_file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(input.staged_path)
            .await
            .map_err(migration_error)?;
        staged_file
            .set_permissions(input.source_permissions.clone())
            .await
            .map_err(migration_error)?;
        let mut staged = BufWriter::with_capacity(PROJECTION_BATCH_BYTES as usize, staged_file);
        let mut canonicalizer = LegacyRolloutCanonicalizer::new(
            input.thread_id,
            input.rollout_id,
            input.reserved_response_item_ids.clone(),
        );
        let written = canonicalizer
            .write_head_session_meta(input.canonical_session_meta.clone(), &mut staged)
            .await?;
        limiter.account(written).await;
        for item in items {
            let line = RolloutLine {
                timestamp: input.canonical_session_meta.timestamp.clone(),
                ordinal: None,
                item,
            };
            let written = canonicalizer
                .process_line(line, &mut staged, CompactionResolution::InferSelected)
                .await?;
            limiter.account(written).await;
        }
        let written = canonicalizer
            .finish(&mut staged, &input.canonical_session_meta.timestamp)
            .await?;
        limiter.account(written).await;
        staged.flush().await.map_err(migration_error)?;
        Ok((
            canonicalizer.output_byte_offset(),
            canonicalizer.next_ordinal(),
        ))
    }

    async fn write_canonical_rollout(
        input: &CanonicalizationSource<'_>,
        plan: Option<&RollbackPlan>,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<CanonicalizationAttempt> {
        let source_file = File::open(input.source_path)
            .await
            .map_err(migration_error)?;
        let staged_file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(input.staged_path)
            .await
            .map_err(migration_error)?;
        staged_file
            .set_permissions(input.source_permissions.clone())
            .await
            .map_err(migration_error)?;
        let mut source = BufReader::with_capacity(PROJECTION_BATCH_BYTES as usize, source_file);
        let mut staged = BufWriter::with_capacity(PROJECTION_BATCH_BYTES as usize, staged_file);
        let mut bytes = Vec::new();
        let mut cursor = RolloutRecordCursor::default();
        let mut parsed_record_index = 0_usize;
        let mut canonicalizer = LegacyRolloutCanonicalizer::new(
            input.thread_id,
            input.rollout_id,
            input.reserved_response_item_ids.clone(),
        );
        let written = canonicalizer
            .write_head_session_meta(input.canonical_session_meta.clone(), &mut staged)
            .await?;
        limiter.account(written).await;
        let mut last_timestamp = input.canonical_session_meta.timestamp.clone();

        while let Some(record) = read_rollout_record(
            &mut source,
            &mut bytes,
            &mut cursor,
            TrailingPartialPolicy::DiscardIncomplete,
        )
        .await?
        {
            limiter.account(record.byte_count).await;
            let Some(line) = record.line else {
                continue;
            };
            if plan.is_none()
                && matches!(
                    &line.item,
                    RolloutItem::EventMsg(codex_protocol::protocol::EventMsg::ThreadRolledBack(_))
                )
            {
                return Ok(CanonicalizationAttempt::NeedsRollbackPlan);
            }
            let compaction_resolution = plan
                .map(|plan| plan.compaction_resolution(parsed_record_index))
                .unwrap_or(CompactionResolution::InferSelected);
            let line = if let Some(plan) = plan {
                let planned = plan.apply(parsed_record_index, line)?;
                parsed_record_index = parsed_record_index
                    .checked_add(1)
                    .ok_or_else(|| migration_error("legacy rollout record index overflow"))?;
                let Some(line) = planned else {
                    continue;
                };
                line
            } else {
                line
            };
            last_timestamp = line.timestamp.clone();
            let written = canonicalizer
                .process_line(line, &mut staged, compaction_resolution)
                .await?;
            limiter.account(written).await;
        }
        if let Some(plan) = plan
            && parsed_record_index != plan.record_count()
        {
            return Err(migration_error(
                "rollback plan source length changed during replay",
            ));
        }

        let written = canonicalizer.finish(&mut staged, &last_timestamp).await?;
        limiter.account(written).await;
        staged.flush().await.map_err(migration_error)?;
        Ok(CanonicalizationAttempt::Complete {
            expected_length: canonicalizer.output_byte_offset(),
            expected_ordinal: canonicalizer.next_ordinal(),
        })
    }

    async fn recover_published_migration(
        &self,
        thread_id: ThreadId,
        rollout_path: &Path,
        journal_path: &Path,
        legacy_names: &HashMap<ThreadId, String>,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<PathBuf> {
        let recovery = read_migration_journal_recovery(journal_path).await?;
        if let Some(recovery) = recovery.as_ref() {
            validate_migration_recovery(
                &self.config.codex_home,
                thread_id,
                journal_path,
                recovery,
                Some(rollout_path),
                /*require_publication_fingerprint*/ true,
            )
            .await?;
            if !tokio::fs::try_exists(&recovery.preserved_source_path)
                .await
                .map_err(migration_error)?
            {
                return Err(ThreadStoreError::Conflict {
                    message: format!(
                        "published rollout is missing its preserved legacy recovery source: {}",
                        recovery.preserved_source_path.display()
                    ),
                });
            }
            verify_migration_source_unchanged(
                &self.config.codex_home,
                thread_id,
                journal_path,
                rollout_path,
            )
            .await?;
        }
        let published_metadata = codex_rollout::read_session_meta_line(rollout_path)
            .await
            .map_err(migration_error)?;
        if published_metadata.meta.id != thread_id
            || published_metadata.meta.history_mode != ThreadHistoryMode::PaginatedRefsV1
        {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "pending migration does not point to a visible paginated_refs_v1 rollout: {}",
                    rollout_path.display()
                ),
            });
        }
        let decompressed_path = rollout_path_is_compressed(rollout_path)
            .then(|| decompressed_staged_rollout_path(rollout_path))
            .transpose()?;
        let projection_path = if let Some(decompressed_path) = decompressed_path.as_ref() {
            decompress_rollout_to_path(rollout_path, decompressed_path).await?;
            let decompressed_bytes = tokio::fs::metadata(decompressed_path)
                .await
                .map_err(migration_error)?
                .len();
            limiter
                .account(
                    tokio::fs::metadata(rollout_path)
                        .await
                        .map_err(migration_error)?
                        .len()
                        .saturating_add(decompressed_bytes),
                )
                .await;
            decompressed_path.as_path()
        } else {
            rollout_path
        };
        let expected_length = tokio::fs::metadata(projection_path)
            .await
            .map_err(migration_error)?
            .len();
        let projection = thread_history::projection_state(self, thread_id).await?;
        if projection.is_none_or(|state| state.next_byte_offset != expected_length) {
            thread_history::delete_thread(self, thread_id).await?;
            self.project_rollout_in_batches(thread_id, projection_path, limiter)
                .await?;
        }
        let source_rollout_path = recovery
            .as_ref()
            .map_or(rollout_path, |recovery| recovery.rollout_path.as_path());
        remove_file_if_present(&staged_rollout_path(source_rollout_path)?).await?;
        remove_file_if_present(&compressed_staged_rollout_path(source_rollout_path)?).await?;
        if let Some(decompressed_path) = decompressed_path.as_ref() {
            remove_file_if_present(decompressed_path).await?;
        }
        self.finish_published_migration(
            thread_id,
            source_rollout_path,
            rollout_path,
            journal_path,
            legacy_names,
        )
        .await?;
        Ok(rollout_path.to_path_buf())
    }

    async fn cleanup_failed_unpublished_migration(
        &self,
        thread_id: ThreadId,
        source_history_mode: ThreadHistoryMode,
        projection_was_replaced: bool,
        rollout_path: &Path,
        journal_path: &Path,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<()> {
        let recorded_recovery = read_migration_journal_recovery(journal_path).await?;
        if let Some(recovery) = recorded_recovery.as_ref() {
            validate_migration_recovery(
                &self.config.codex_home,
                thread_id,
                journal_path,
                recovery,
                /*selected_published_path*/ None,
                /*require_publication_fingerprint*/ false,
            )
            .await?;
        }
        if let Some(recovery) = recorded_recovery.as_ref()
            && tokio::fs::try_exists(recovery.published_rollout_path())
                .await
                .map_err(migration_error)?
            && codex_rollout::read_session_meta_line(recovery.published_rollout_path())
                .await
                .is_ok_and(|metadata| {
                    metadata.meta.id == thread_id
                        && metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1
                })
        {
            // Publication already crossed the visible format gate. Keep the journal and preserved
            // source for normal recovery instead of treating the old compressed/source name as
            // an unpublished migration.
            return Ok(());
        }
        let Ok(metadata) = codex_rollout::read_session_meta_line(rollout_path).await else {
            return Ok(());
        };
        if metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1 {
            return Ok(());
        }

        if source_history_mode != ThreadHistoryMode::Paginated || projection_was_replaced {
            thread_history::delete_thread(self, thread_id).await?;
        }
        let staged_path = staged_rollout_path(rollout_path)?;
        remove_file_if_present(&staged_path).await?;
        remove_file_if_present(&rewritten_staged_rollout_path(&staged_path)?).await?;
        remove_file_if_present(&compressed_staged_rollout_path(rollout_path)?).await?;
        remove_file_if_present(&decompressed_staged_rollout_path(rollout_path)?).await?;
        if source_history_mode == ThreadHistoryMode::Paginated && projection_was_replaced {
            // Restore the visible old paginated projection before discarding its durable retry
            // marker. A crash or projection failure leaves the journal/alias in place so startup
            // cannot silently advance past an empty SQLite history.
            self.project_rollout_in_batches(thread_id, rollout_path, limiter)
                .await?;
        }
        if let Some(recovery) = recorded_recovery {
            remove_preserved_source_alias_if_present(rollout_path, &recovery.preserved_source_path)
                .await?;
        }
        remove_migration_journal(journal_path).await?;
        Ok(())
    }

    async fn finish_published_migration(
        &self,
        thread_id: ThreadId,
        source_rollout_path: &Path,
        published_rollout_path: &Path,
        journal_path: &Path,
        legacy_names: &HashMap<ThreadId, String>,
    ) -> ThreadStoreResult<()> {
        if source_rollout_path != published_rollout_path
            && let Some(state_db) = &self.state_db
        {
            let current_path = state_db
                .get_thread(thread_id)
                .await
                .map_err(migration_error)?
                .ok_or_else(|| {
                    migration_error(format!("thread {thread_id} is missing its SQLite metadata"))
                })?
                .rollout_path;
            if current_path != published_rollout_path {
                let expected_current_path = if current_path == source_rollout_path {
                    current_path.clone()
                } else if !tokio::fs::try_exists(&current_path)
                    .await
                    .map_err(migration_error)?
                    && same_rollout_filename(&current_path, source_rollout_path)
                {
                    // Archive/unarchive and background compression may move the same rollout
                    // while startup's busy retry is parked. The old indexed path is safe as an
                    // atomic compare-and-swap expectation only when it no longer exists and its
                    // canonical plain filename identifies the source now being published.
                    current_path.clone()
                } else {
                    return Err(ThreadStoreError::Conflict {
                        message: format!(
                            "thread rollout path changed while publishing plain reference history: {}",
                            current_path.display()
                        ),
                    });
                };
                if !state_db
                    .replace_rollout_path_if_current(
                        thread_id,
                        &expected_current_path,
                        published_rollout_path,
                    )
                    .await
                    .map_err(migration_error)?
                {
                    return Err(ThreadStoreError::Conflict {
                        message: format!(
                            "thread rollout path changed while publishing plain reference history: {}",
                            current_path.display()
                        ),
                    });
                }
            }
        }
        self.promote_legacy_name(thread_id, ThreadHistoryMode::PaginatedRefsV1, legacy_names)
            .await?;
        if source_rollout_path != published_rollout_path {
            remove_file_if_present(source_rollout_path).await?;
            sync_parent_directory(source_rollout_path).await?;
        }
        retain_migration_recovery(journal_path).await?;
        Ok(())
    }

    async fn promote_legacy_name(
        &self,
        thread_id: ThreadId,
        target_history_mode: ThreadHistoryMode,
        legacy_names: &HashMap<ThreadId, String>,
    ) -> ThreadStoreResult<()> {
        if let Some(state_db) = &self.state_db {
            let metadata = state_db
                .get_thread(thread_id)
                .await
                .map_err(migration_error)?
                .ok_or_else(|| {
                    migration_error(format!("thread {thread_id} is missing its SQLite metadata"))
                })?;
            // SQLite deliberately exposes the public paginated family to older clients while the
            // rollout head retains the internal RefsV1 gate. Treat either paginated member as the
            // target family here; exact format validation remains on SessionMeta and journals.
            if metadata.history_mode.is_paginated()
                && target_history_mode.is_paginated()
                && metadata
                    .name
                    .as_deref()
                    .is_some_and(|name| !name.trim().is_empty())
            {
                return Ok(());
            }
            let legacy_name = distinct_thread_metadata_title(&metadata)
                .or_else(|| legacy_names.get(&thread_id).cloned())
                .filter(|name| !name.trim().is_empty());
            if !state_db
                .mark_thread_paginated(thread_id, target_history_mode, legacy_name.as_deref())
                .await
                .map_err(migration_error)?
            {
                return Err(migration_error(format!(
                    "thread {thread_id} is missing its SQLite metadata"
                )));
            }
        }
        Ok(())
    }

    async fn project_rollout_in_batches(
        &self,
        thread_id: ThreadId,
        rollout_path: &Path,
        limiter: &mut RolloutMigrationRateLimiter,
    ) -> ThreadStoreResult<()> {
        let session_meta = codex_rollout::read_session_meta_line(rollout_path)
            .await
            .map_err(migration_error)?;
        let subagent_history_start_ordinal = session_meta.meta.subagent_history_start_ordinal;
        let initial_ordinal = session_meta
            .meta
            .history_base
            .as_ref()
            .map_or(0, |base| base.end_ordinal_exclusive);
        let file = File::open(rollout_path).await.map_err(migration_error)?;
        let mut reader = BufReader::with_capacity(PROJECTION_BATCH_BYTES as usize, file);
        let mut line_bytes = Vec::new();
        let mut batch = Vec::new();
        let mut batch_start = 0_u64;
        let mut offset = 0_u64;
        let mut cursor = RolloutRecordCursor::default();

        while let Some(record) = read_rollout_record(
            &mut reader,
            &mut line_bytes,
            &mut cursor,
            TrailingPartialPolicy::Reject,
        )
        .await?
        {
            let next_offset = offset
                .checked_add(record.byte_count)
                .ok_or_else(|| migration_error("staged rollout byte offset overflow"))?;
            limiter.account(record.byte_count).await;
            let Some(line) = record.line else {
                offset = next_offset;
                continue;
            };
            let ordinal = line
                .ordinal
                .ok_or_else(|| migration_error("staged rollout line is missing its ordinal"))?;
            let fallback_created_at_ms = DateTime::parse_from_rfc3339(&line.timestamp)
                .map_err(migration_error)?
                .timestamp_millis();
            let is_inherited_subagent_history =
                subagent_history_start_ordinal.is_some_and(|start| ordinal < start);
            batch.push(RolloutProjectionStep::Line(Box::new(
                ProjectedRolloutLine {
                    ordinal,
                    start_byte_offset: offset,
                    end_byte_offset: next_offset,
                    fallback_created_at_ms: Some(fallback_created_at_ms),
                    changes: if is_inherited_subagent_history {
                        Default::default()
                    } else {
                        project_rollout_line(&line)
                    },
                    realtime_item: match line.item {
                        RolloutItem::RealtimeItem(item) if !is_inherited_subagent_history => {
                            Some(item)
                        }
                        _ => None,
                    },
                },
            )));
            offset = next_offset;

            if offset.saturating_sub(batch_start) >= PROJECTION_BATCH_BYTES {
                thread_history::apply_projection(
                    self,
                    thread_id,
                    batch_start,
                    offset,
                    initial_ordinal,
                    std::mem::take(&mut batch),
                )
                .await?;
                batch_start = offset;
            }
        }

        if batch_start != offset {
            thread_history::apply_projection(
                self,
                thread_id,
                batch_start,
                offset,
                initial_ordinal,
                batch,
            )
            .await?;
        }
        Ok(())
    }
}

async fn read_rollout_record(
    reader: &mut BufReader<File>,
    bytes: &mut Vec<u8>,
    cursor: &mut RolloutRecordCursor,
    trailing_partial: TrailingPartialPolicy,
) -> ThreadStoreResult<Option<RolloutRecord>> {
    let mut record = LogicalRolloutRecord::default();
    loop {
        bytes.clear();
        let chunk_bytes = reader
            .take(record.read_limit())
            .read_until(b'\n', bytes)
            .await
            .map_err(migration_error)?;
        if chunk_bytes == 0 {
            break;
        }
        if record.append_chunk(bytes).map_err(migration_error)? {
            break;
        }
    }
    if record.source_bytes == 0 {
        return Ok(None);
    }
    let record = record
        .finish(*cursor, trailing_partial)
        .map_err(migration_error)?;
    cursor.advance(record.byte_count).map_err(migration_error)?;
    Ok(Some(record))
}

async fn read_strict_paginated_record(
    reader: &mut BufReader<File>,
    bytes: &mut Vec<u8>,
    cursor: &mut RolloutRecordCursor,
) -> ThreadStoreResult<Option<RolloutRecord>> {
    let mut record = LogicalRolloutRecord::default();
    loop {
        bytes.clear();
        let chunk_bytes = reader
            .take(record.read_limit())
            .read_until(b'\n', bytes)
            .await
            .map_err(migration_error)?;
        if chunk_bytes == 0 {
            break;
        }
        if record.append_chunk(bytes).map_err(migration_error)? {
            break;
        }
    }
    if record.source_bytes == 0 {
        return Ok(None);
    }
    let record = record
        .finish_strict_paginated(*cursor)
        .map_err(migration_error)?;
    cursor.advance(record.byte_count).map_err(migration_error)?;
    Ok(Some(record))
}

#[cfg(test)]
fn parse_rollout_record(
    bytes: &[u8],
    terminated: bool,
    source_bytes: u64,
    cursor: RolloutRecordCursor,
    trailing_partial: TrailingPartialPolicy,
    require_rollout_line: bool,
) -> Result<Option<RolloutLine>, RolloutRecordValidationError> {
    parse_rollout_record_with_mode(
        bytes,
        terminated,
        source_bytes,
        cursor,
        trailing_partial,
        require_rollout_line,
        /*strict_paginated*/ false,
    )
}

fn parse_rollout_record_with_mode(
    bytes: &[u8],
    terminated: bool,
    source_bytes: u64,
    cursor: RolloutRecordCursor,
    trailing_partial: TrailingPartialPolicy,
    require_rollout_line: bool,
    strict_paginated: bool,
) -> Result<Option<RolloutLine>, RolloutRecordValidationError> {
    if strict_paginated {
        if !terminated || require_rollout_line {
            return Err(RolloutRecordValidationError::Malformed {
                ordinal: cursor.ordinal,
                byte_offset: cursor.byte_offset,
                observed_bytes: bytes.len() as u64,
                source_bytes,
                parser_error: "paginated rollout record is not one complete terminated JSONL line"
                    .to_string(),
            });
        }
        return serde_json::from_slice::<RolloutLine>(bytes)
            .map(Some)
            .map_err(|error| RolloutRecordValidationError::Malformed {
                ordinal: cursor.ordinal,
                byte_offset: cursor.byte_offset,
                observed_bytes: bytes.len() as u64,
                source_bytes,
                parser_error: error.to_string(),
            });
    }
    match line_parser::parse_legacy_rollout_line(bytes) {
        Ok(None) if require_rollout_line => Err(RolloutRecordValidationError::Malformed {
            ordinal: cursor.ordinal,
            byte_offset: cursor.byte_offset,
            observed_bytes: bytes.len() as u64,
            source_bytes,
            parser_error: "newline-repaired record is not a supported rollout line".to_string(),
        }),
        Ok(line) => Ok(line),
        // A writer can stop between bytes. Preserve the existing recovery behavior only for a
        // source tail that serde identifies as truncated JSON, not merely any invalid EOF record.
        Err(_)
            if !terminated
                && trailing_partial == TrailingPartialPolicy::DiscardIncomplete
                && serde_json::from_slice::<serde_json::Value>(bytes)
                    .is_err_and(|error| error.is_eof()) =>
        {
            Ok(None)
        }
        Err(parser_error) => Err(RolloutRecordValidationError::Malformed {
            ordinal: cursor.ordinal,
            byte_offset: cursor.byte_offset,
            observed_bytes: bytes.len() as u64,
            source_bytes,
            parser_error,
        }),
    }
}

async fn inspect_rollout_records(
    rollout_path: &Path,
    depth: RolloutInspectionDepth,
) -> Result<RolloutInspection, RolloutValidationFailure> {
    let rollout_path = rollout_path.to_path_buf();
    let compressed = rollout_path_is_compressed(&rollout_path);
    let result = tokio::task::spawn_blocking(move || {
        inspect_rollout_records_blocking(&rollout_path, compressed, depth)
    })
    .await
    .map_err(|error| RolloutValidationFailure {
        failure: RolloutMigrationFailure::new(
            RolloutMigrationFailureReason::RolloutReadFailed,
            migration_error(error),
        ),
        bytes_processed: 0,
        read_error_kind: None,
    })?;

    result.map_err(|error| match error {
        RolloutScanFailure::Read {
            error,
            bytes_processed,
        } => {
            let read_error_kind = Some(error.kind());
            RolloutValidationFailure {
                failure: RolloutMigrationFailure::new(
                    RolloutMigrationFailureReason::RolloutReadFailed,
                    migration_error(error),
                ),
                bytes_processed,
                read_error_kind,
            }
        }
        RolloutScanFailure::InvalidSessionMetadata {
            message,
            bytes_processed,
        } => RolloutValidationFailure {
            failure: RolloutMigrationFailure::new(
                RolloutMigrationFailureReason::InvalidSessionMetadata,
                migration_error(message),
            ),
            bytes_processed,
            read_error_kind: None,
        },
        RolloutScanFailure::Record(error) => {
            let reason = error.failure_reason();
            let bytes_processed = error.bytes_processed();
            RolloutValidationFailure {
                failure: RolloutMigrationFailure::new(reason, migration_error(error)),
                bytes_processed,
                read_error_kind: None,
            }
        }
    })
}

fn inspect_rollout_records_blocking(
    rollout_path: &Path,
    compressed: bool,
    depth: RolloutInspectionDepth,
) -> Result<RolloutInspection, RolloutScanFailure> {
    let source = std::fs::File::open(rollout_path).map_err(|error| RolloutScanFailure::Read {
        error,
        bytes_processed: 0,
    })?;
    if compressed {
        let decoder =
            zstd::stream::read::Decoder::new(source).map_err(|error| RolloutScanFailure::Read {
                error,
                bytes_processed: 0,
            })?;
        inspect_rollout_reader(
            std::io::BufReader::with_capacity(PROJECTION_BATCH_BYTES as usize, decoder),
            depth,
        )
    } else {
        inspect_rollout_reader(
            std::io::BufReader::with_capacity(PROJECTION_BATCH_BYTES as usize, source),
            depth,
        )
    }
}

fn inspect_rollout_reader<R: StdBufRead>(
    mut reader: R,
    depth: RolloutInspectionDepth,
) -> Result<RolloutInspection, RolloutScanFailure> {
    let mut bytes = Vec::new();
    let mut cursor = RolloutRecordCursor::default();
    let mut metadata = None;
    while let Some(record) = read_rollout_record_blocking(
        &mut reader,
        &mut bytes,
        &mut cursor,
        TrailingPartialPolicy::DiscardIncomplete,
    )? {
        let Some(line) = record.line else {
            continue;
        };
        match line.item {
            RolloutItem::SessionMeta(candidate) => {
                if metadata.is_none()
                    && (candidate.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1
                        || depth == RolloutInspectionDepth::MetadataOnly)
                {
                    return Ok(RolloutInspection {
                        metadata: candidate,
                        bytes_processed: cursor.byte_offset,
                    });
                }
                if metadata.is_none() {
                    metadata = Some(candidate);
                }
            }
            RolloutItem::ResponseItem(_) | RolloutItem::InterAgentCommunication(_)
                if metadata.is_none() =>
            {
                return Err(RolloutScanFailure::InvalidSessionMetadata {
                    message: "rollout does not start with session metadata".to_string(),
                    bytes_processed: cursor.byte_offset,
                });
            }
            RolloutItem::ResponseItem(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::Compacted(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::RealtimeItem(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_) => {}
        }
    }
    metadata
        .map(|metadata| RolloutInspection {
            metadata,
            bytes_processed: cursor.byte_offset,
        })
        .ok_or_else(|| RolloutScanFailure::InvalidSessionMetadata {
            message: "rollout contains no session metadata".to_string(),
            bytes_processed: cursor.byte_offset,
        })
}

fn read_rollout_record_blocking<R: StdBufRead>(
    reader: &mut R,
    bytes: &mut Vec<u8>,
    cursor: &mut RolloutRecordCursor,
    trailing_partial: TrailingPartialPolicy,
) -> Result<Option<RolloutRecord>, RolloutScanFailure> {
    let mut record = LogicalRolloutRecord::default();
    loop {
        bytes.clear();
        let chunk_bytes = reader
            .take(record.read_limit())
            .read_until(b'\n', bytes)
            .map_err(|error| RolloutScanFailure::Read {
                error,
                bytes_processed: cursor.byte_offset.saturating_add(record.source_bytes),
            })?;
        if chunk_bytes == 0 {
            break;
        }
        if record
            .append_chunk(bytes)
            .map_err(|message| RolloutScanFailure::Read {
                error: io::Error::other(message),
                bytes_processed: cursor.byte_offset.saturating_add(record.source_bytes),
            })?
        {
            break;
        }
    }
    if record.source_bytes == 0 {
        return Ok(None);
    }
    let record = record
        .finish(*cursor, trailing_partial)
        .map_err(RolloutScanFailure::Record)?;
    cursor
        .advance(record.byte_count)
        .map_err(|message| RolloutScanFailure::Read {
            error: io::Error::other(message),
            bytes_processed: cursor.byte_offset,
        })?;
    Ok(Some(record))
}

async fn find_rollout_paths(root: &Path) -> ThreadStoreResult<Vec<PathBuf>> {
    let mut directories = vec![root.to_path_buf()];
    let mut paths = Vec::new();

    while let Some(directory) = directories.pop() {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(migration_error(error)),
        };
        while let Some(entry) = entries.next_entry().await.map_err(migration_error)? {
            let kind = entry.file_type().await.map_err(migration_error)?;
            if kind.is_dir() {
                directories.push(entry.path());
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with("rollout-")
                && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
            {
                let path = entry.path();
                if name.ends_with(".jsonl.zst")
                    && tokio::fs::try_exists(codex_rollout::plain_rollout_path(&path))
                        .await
                        .map_err(migration_error)?
                {
                    continue;
                }
                paths.push(path);
            }
        }
    }

    paths.sort_by(|left, right| right.cmp(left));
    Ok(paths)
}

async fn find_all_rollout_paths(codex_home: &Path) -> ThreadStoreResult<Vec<PathBuf>> {
    let mut paths = find_rollout_paths(&codex_home.join(codex_rollout::SESSIONS_SUBDIR)).await?;
    paths.extend(
        find_rollout_paths(&codex_home.join(codex_rollout::ARCHIVED_SESSIONS_SUBDIR)).await?,
    );
    Ok(paths)
}

async fn find_current_rollout_path(
    codex_home: &Path,
    stale_path: &Path,
) -> ThreadStoreResult<Option<PathBuf>> {
    let plain_path = codex_rollout::plain_rollout_path(stale_path);
    let Some(file_name) = plain_path.file_name() else {
        return Ok(None);
    };
    Ok(find_all_rollout_paths(codex_home)
        .await?
        .into_iter()
        .find(|candidate| {
            codex_rollout::plain_rollout_path(candidate).file_name() == Some(file_name)
        }))
}

fn matches_selection(selected: &[ThreadId], actual: Option<ThreadId>) -> bool {
    selected.is_empty() || actual.is_some_and(|thread_id| selected.contains(&thread_id))
}

fn requires_reference_migration(history_mode: ThreadHistoryMode) -> bool {
    history_mode == ThreadHistoryMode::Legacy
}

fn collect_response_item_ids(item: &RolloutItem, item_ids: &mut HashSet<String>) {
    match item {
        RolloutItem::ResponseItem(envelope) => {
            collect_response_item_envelope_id(envelope, item_ids)
        }
        RolloutItem::Compacted(compacted) => {
            if let Some(history) = &compacted.replacement_history {
                for envelope in history {
                    collect_response_item_envelope_id(envelope, item_ids);
                }
            }
            if let Some(entries) = &compacted.replacement_history_entries {
                for entry in entries {
                    match entry {
                        codex_rollout::CompactedHistoryEntry::Inline { item, .. } => {
                            if let Some(item_id) =
                                item.id().filter(|item_id| !item_id.as_str().is_empty())
                            {
                                item_ids.insert(item_id.as_str().to_string());
                            }
                        }
                        codex_rollout::CompactedHistoryEntry::Reference { item_id }
                        | codex_rollout::CompactedHistoryEntry::ReferenceV2 { item_id, .. }
                            if !item_id.is_empty() =>
                        {
                            item_ids.insert(item_id.clone());
                        }
                        codex_rollout::CompactedHistoryEntry::Reference { .. }
                        | codex_rollout::CompactedHistoryEntry::ReferenceV2 { .. } => {}
                    }
                }
            }
        }
        RolloutItem::SessionMeta(_)
        | RolloutItem::InterAgentCommunication(_)
        | RolloutItem::InterAgentCommunicationMetadata { .. }
        | RolloutItem::TurnContext(_)
        | RolloutItem::WorldState(_)
        | RolloutItem::SecurityRiskScore(_)
        | RolloutItem::EventMsg(_)
        | RolloutItem::RealtimeItem(_) => {}
    }
}

fn collect_response_item_envelope_id(
    envelope: &codex_rollout::ResponseItemEnvelope,
    item_ids: &mut HashSet<String>,
) {
    if let Some(item_id) = envelope
        .item
        .id()
        .filter(|item_id| !item_id.as_str().is_empty())
    {
        item_ids.insert(item_id.as_str().to_string());
    }
}

fn same_rollout_filename(left: &Path, right: &Path) -> bool {
    codex_rollout::plain_rollout_path(left).file_name()
        == codex_rollout::plain_rollout_path(right).file_name()
}

fn retirement_refused_outcome(
    thread_id: ThreadId,
    rollout_path: PathBuf,
    preserved_source_path: PathBuf,
    message: String,
) -> PreservedSourceRetirementOutcome {
    PreservedSourceRetirementOutcome {
        thread_id,
        rollout_path,
        preserved_source_path,
        status: PreservedSourceRetirementStatus::Refused,
        message: Some(message),
    }
}

fn rollout_path_is_compressed(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".jsonl.zst"))
}

fn thread_id_from_rollout_filename(path: &Path) -> Option<ThreadId> {
    let name = path.file_name()?.to_str()?;
    let stem = name
        .strip_suffix(".jsonl.zst")
        .or_else(|| name.strip_suffix(".jsonl"))?;
    let start = stem.len().checked_sub(36)?;
    ThreadId::from_string(stem.get(start..)?).ok()
}

fn migration_outcome(
    thread_id: ThreadId,
    rollout_path: PathBuf,
    result: ClassifiedMigrationResult<RolloutMigrationStatus>,
    bytes_processed: u64,
) -> RolloutMigrationOutcome {
    match result {
        Ok(status) => RolloutMigrationOutcome {
            thread_id: Some(thread_id),
            rollout_path,
            status,
            failure_reason: None,
            preserved_source_path: None,
            bytes_processed,
            message: None,
        },
        Err(failure) => RolloutMigrationOutcome {
            thread_id: Some(thread_id),
            rollout_path,
            status: RolloutMigrationStatus::Failed,
            failure_reason: Some(failure.reason),
            preserved_source_path: None,
            bytes_processed,
            message: Some(failure.error.to_string()),
        },
    }
}

fn recovery_conflict_outcome(
    thread_id: ThreadId,
    rollout_path: PathBuf,
    message: String,
    bytes_processed: u64,
) -> RolloutMigrationOutcome {
    RolloutMigrationOutcome {
        thread_id: Some(thread_id),
        rollout_path,
        status: RolloutMigrationStatus::Failed,
        failure_reason: Some(RolloutMigrationFailureReason::RecoveryStateConflict),
        preserved_source_path: None,
        bytes_processed,
        message: Some(message),
    }
}

fn skipped_busy_outcome(
    thread_id: ThreadId,
    rollout_path: PathBuf,
    message: String,
    bytes_processed: u64,
) -> RolloutMigrationOutcome {
    RolloutMigrationOutcome {
        thread_id: Some(thread_id),
        rollout_path,
        status: RolloutMigrationStatus::SkippedBusy,
        failure_reason: None,
        preserved_source_path: None,
        bytes_processed,
        message: Some(message),
    }
}

fn migration_error(error: impl std::fmt::Display) -> ThreadStoreError {
    ThreadStoreError::Internal {
        message: format!("rollout migration failed: {error}"),
    }
}

#[cfg(test)]
#[path = "rollout_migration_tests.rs"]
mod tests;
