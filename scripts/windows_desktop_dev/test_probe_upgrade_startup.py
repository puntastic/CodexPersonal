"""Synthetic probe checks: no user databases, executable startup, or model calls."""

from argparse import Namespace
from copy import deepcopy
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
from unittest.mock import Mock

import probe_upgrade_startup as probe


def execute(path, sql, parameters=()):
    db = sqlite3.connect(path)
    try:
        with db:
            db.execute(sql, parameters)
    finally:
        db.close()


def create_database(path):
    statements = [
        "CREATE TABLE _sqlx_migrations (version INTEGER PRIMARY KEY, description TEXT, "
        "installed_on TEXT, success BOOLEAN, checksum BLOB, execution_time BIGINT)",
        "CREATE TABLE threads (id TEXT PRIMARY KEY, source TEXT, title TEXT, "
        "first_user_message TEXT, creator_user_id TEXT, creator_account_id TEXT, "
        "archived INTEGER, created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER)",
        "CREATE TABLE thread_attachments (id TEXT PRIMARY KEY, thread_id TEXT, payload BLOB)",
        "INSERT INTO threads VALUES ('b', 'cli', 'second', 'hello', NULL, NULL, 1, 20, 30, 40)",
        "INSERT INTO threads VALUES ('a', 'cli', 'first', 'world', NULL, NULL, 0, 10, 20, 30)",
        "INSERT INTO thread_attachments VALUES ('attachment', 'a', X'00FF')",
    ]
    for statement in statements:
        execute(path, statement)
    for version in (55, 56, 57):
        execute(
            path,
            "INSERT INTO _sqlx_migrations VALUES (?, ?, ?, 1, ?, ?)",
            (
                version,
                f"migration {version}",
                "2026-01-01T00:00:00Z",
                bytes([version]),
                version * 10,
            ),
        )


def migrate(path):
    for key in probe.ARCHIVE_SORT_KEYS:
        execute(
            path,
            f"CREATE INDEX IF NOT EXISTS idx_threads_archive_{key}_ms "
            f"ON threads(archived, {key}_ms DESC, id DESC) WHERE archived = 1",
        )
    execute(
        path,
        "INSERT OR IGNORE INTO _sqlx_migrations VALUES "
        "(58, 'archive indexes', '2026-10-09T00:00:00Z', 1, X'58', 580)",
    )


class ProbeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.resolve_windows_system_config_directory = (
            probe.windows_system_config_directory
        )
        # Never query this machine's ProgramData or read any live policy/config.
        self.system_config_directory = self.root / "synthetic-system-config"
        patcher = patch.object(
            probe,
            "windows_system_config_directory",
            return_value=self.system_config_directory,
        )
        patcher.start()
        self.addCleanup(patcher.stop)
        self.source = self.root / "state.sqlite"
        create_database(self.source)
        self.before = probe.schema(self.source)

    def test_schema_preserves_full_ledger_and_logical_rows_across_index_migration(self):
        self.assertEqual(
            self.before["migration_ledger"][0],
            {
                "version": 55,
                "description": "migration 55",
                "installed_on": "2026-01-01T00:00:00Z",
                "success": 1,
                "checksum": {"blob_hex": "37"},
                "execution_time": 550,
            },
        )
        migrate(self.source)
        after = probe.schema(self.source)
        probe.validate_readback(self.before, after, primary=True)
        self.assertEqual(self.before["row_evidence"], after["row_evidence"])
        # Physical insertion order cannot change the logical digest.
        execute(self.source, "DELETE FROM threads WHERE id='b'")
        execute(
            self.source,
            "INSERT INTO threads VALUES ('b', 'cli', 'second', 'hello', NULL, NULL, 1, 20, 30, 40)",
        )
        self.assertEqual(
            self.before["row_evidence"], probe.schema(self.source)["row_evidence"]
        )

    def test_changed_or_missing_existing_migration_rows_are_rejected(self):
        migrate(self.source)
        after = probe.schema(self.source)
        for field in (
            "description",
            "installed_on",
            "success",
            "checksum",
            "execution_time",
        ):
            with self.subTest(field=field):
                changed = deepcopy(after)
                changed["migration_ledger"][0][field] = "changed"
                with self.assertRaisesRegex(RuntimeError, "ledger rows changed"):
                    probe.validate_readback(self.before, changed, primary=True)
        changed = deepcopy(after)
        changed["migration_ledger"].pop(0)
        with self.assertRaisesRegex(RuntimeError, "ledger rows changed"):
            probe.validate_readback(self.before, changed, primary=True)

    def test_equal_counts_do_not_hide_changed_thread_or_attachment_values(self):
        migrate(self.source)
        for statement in (
            "UPDATE threads SET title='rewritten' WHERE id='a'",
            "UPDATE thread_attachments SET payload=X'0011'",
        ):
            with self.subTest(statement=statement):
                before = probe.schema(self.source)
                execute(self.source, statement)
                after = probe.schema(self.source)
                self.assertEqual(before["attachment_rows"], after["attachment_rows"])
                with self.assertRaisesRegex(RuntimeError, "evidence changed"):
                    probe.validate_readback(before, after, primary=True)

    def test_archive_index_sql_and_order_are_checked(self):
        migrate(self.source)
        for definition in (
            "ON threads(archived, created_at_ms ASC, id DESC) WHERE archived = 1",
            "ON threads(archived, created_at_ms DESC, id DESC) WHERE archived = 0",
        ):
            execute(self.source, "DROP INDEX idx_threads_archive_created_at_ms")
            execute(
                self.source,
                "CREATE INDEX idx_threads_archive_created_at_ms " + definition,
            )
            with self.assertRaisesRegex(RuntimeError, "unexpected SQL"):
                probe.validate_readback(
                    self.before, probe.schema(self.source), primary=True
                )
        migrate_after = probe.schema(self.source)
        migrate_after["archive_indexes"]["idx_threads_archive_created_at_ms"]["sql"] = (
            "CREATE INDEX idx_threads_archive_created_at_ms "
            "ON threads(archived, created_at_ms DESC, id DESC) WHERE archived = 1"
        )
        migrate_after["archive_indexes"]["idx_threads_archive_created_at_ms"][
            "keys"
        ].reverse()
        with self.assertRaisesRegex(RuntimeError, "column order"):
            probe.validate_readback(self.before, migrate_after, primary=True)

    def test_integrity_failure_replacement_and_recovery_roots_are_detected(self):
        migrate(self.source)
        after = probe.schema(self.source)
        changed = deepcopy(after)
        changed["quick_check"] = "corrupt"
        with self.assertRaisesRegex(RuntimeError, "quick_check"):
            probe.validate_readback(self.before, changed, primary=True)
        changed = deepcopy(after)
        changed["file_identity"][1] += 1
        with self.assertRaisesRegex(RuntimeError, "was replaced"):
            probe.validate_readback(self.before, changed, primary=True)
        home = self.root / "isolated-home"
        home.mkdir()
        self.assertEqual(probe.recovery_backups(home), [])
        (home / "db-backups").mkdir()
        home.with_name("isolated-home.db-backups").mkdir()
        self.assertEqual(len(probe.recovery_backups(home)), 2)

    def test_metadata_rpc_calls_are_bounded_and_db_only(self):
        calls = []

        def request(identifier, method, params):
            calls.append((identifier, method, params))
            return {
                "data": []
                if method == "thread/attachment/list"
                else [{"id": "synthetic"}]
            }

        result = probe.list_metadata(request, check_attachments=True)
        self.assertEqual(
            result["db_only_archived_threads_returned"],
            dict.fromkeys(probe.ARCHIVE_SORT_KEYS, 1),
        )
        self.assertEqual(
            [call[1] for call in calls],
            [
                "thread/list",
                "thread/attachment/list",
                "thread/list",
                "thread/list",
                "thread/list",
            ],
        )
        for _, method, params in calls:
            self.assertLessEqual(params["limit"], 2)
            if method == "thread/list":
                self.assertTrue(params["useStateDbOnly"])
        self.assertEqual(
            [call[2]["sortKey"] for call in calls[2:]], list(probe.ARCHIVE_SORT_KEYS)
        )
        self.assertTrue(all(call[2]["archived"] for call in calls[2:]))
        calls.clear()
        previous = probe.list_metadata(request, check_attachments=False)
        self.assertEqual(
            previous["db_only_archived_threads_returned"],
            result["db_only_archived_threads_returned"],
        )
        self.assertEqual([call[1] for call in calls], ["thread/list"] * 4)
        with self.assertRaisesRegex(RuntimeError, "bounded thread-list"):
            probe.list_metadata(lambda *_: {"data": [{}, {}]}, check_attachments=True)

    def run_synthetic_probe(
        self,
        *,
        damage_final=False,
        damage_backup=False,
        recover=False,
        fail_startup=False,
        damage_companion=False,
        expected_added_migrations=None,
        candidate_added_migrations=(),
    ):
        executable = self.root / "not-an-executable"
        executable.touch()
        count = 0
        companion = self.root / "companion.sqlite"
        create_database(companion)

        def startup(_executable, home, *, check_attachments=False):
            nonlocal count
            count += 1
            if check_attachments:
                migrate(home / self.source.name)
                migrate(home / companion.name)
                for version in candidate_added_migrations:
                    execute(
                        home / self.source.name,
                        "INSERT OR IGNORE INTO _sqlx_migrations VALUES (?, ?, ?, 1, ?, ?)",
                        (
                            version,
                            f"migration {version}",
                            "2026-10-16T00:00:00Z",
                            bytes([version]),
                            version * 10,
                        ),
                    )
            if count == 2 and damage_companion:
                execute(
                    home / companion.name,
                    "DELETE FROM _sqlx_migrations WHERE version=58",
                )
            if count == 2 and fail_startup:
                raise RuntimeError("synthetic startup failure")
            if count == 3 and damage_final:
                execute(home / self.source.name, "DELETE FROM thread_attachments")
            if count == 3 and damage_backup:
                execute(
                    home.parent / self.source.name,
                    "UPDATE threads SET title='tampered'",
                )
            if recover:
                (home / "db-backups").mkdir(exist_ok=True)
            return {"synthetic": True}

        args = Namespace(
            candidate=executable,
            previous=executable,
            source_db=self.source,
            source_companion_db=[companion],
            output=self.root / "output",
            expected_added_migration=expected_added_migrations,
        )
        with patch.object(probe, "startup", side_effect=startup) as mocked:
            report = probe.run_probe(args)
        self.assertEqual(mocked.call_count, 3)
        return report

    def test_all_three_startups_have_final_readbacks_and_untouched_hashes(self):
        original_hash = probe.file_hash(self.source)
        report = self.run_synthetic_probe()
        self.assertTrue(report["passed"])
        self.assertEqual(report["expected_added_migrations"], [58])
        for key in (
            "after_candidate",
            "after_previous",
            "after_candidate_after_previous",
        ):
            self.assertEqual(report[key]["quick_check"], "ok")
            self.assertEqual(
                report[key]["row_evidence"], report["before"]["row_evidence"]
            )
            self.assertEqual(
                report["companions"]["companion.sqlite"][key]["quick_check"], "ok"
            )
        self.assertEqual(report["backup_sha256"], report["backup_sha256_final"])
        self.assertEqual(original_hash, probe.file_hash(self.source))

    def test_explicit_added_migrations_are_recorded_and_accepted(self):
        migrate(self.source)
        report = self.run_synthetic_probe(
            expected_added_migrations=(59, 60, 61),
            candidate_added_migrations=(59, 60, 61),
        )
        self.assertTrue(report["passed"])
        self.assertTrue(report["untouched_backups_verified"])
        self.assertEqual(report["expected_added_migrations"], [59, 60, 61])
        self.assertEqual(
            report["after_candidate_after_previous"]["applied_migrations"],
            [55, 56, 57, 58, 59, 60, 61],
        )
        receipt = json.loads(
            (self.root / "output" / "receipt.json").read_text(encoding="utf-8")
        )
        self.assertEqual(receipt["expected_added_migrations"], [59, 60, 61])

    def test_explicit_added_migrations_reject_unexpected_62(self):
        migrate(self.source)
        with self.assertRaisesRegex(RuntimeError, "unexpected added state migrations"):
            self.run_synthetic_probe(
                expected_added_migrations=(59, 60, 61),
                candidate_added_migrations=(59, 60, 61, 62),
            )
        receipt = json.loads(
            (self.root / "output" / "receipt.json").read_text(encoding="utf-8")
        )
        self.assertFalse(receipt["passed"])
        self.assertEqual(receipt["expected_added_migrations"], [59, 60, 61])
        self.assertTrue(receipt["untouched_backups_verified"])

    def test_explicit_added_migrations_do_not_also_allow_default_58(self):
        with self.assertRaisesRegex(RuntimeError, "unexpected added state migrations"):
            self.run_synthetic_probe(
                expected_added_migrations=(59, 60, 61),
                candidate_added_migrations=(59, 60, 61),
            )

    def test_explicit_added_migrations_require_every_qualified_addition(self):
        migrate(self.source)
        with self.assertRaisesRegex(RuntimeError, "unexpected added state migrations"):
            self.run_synthetic_probe(
                expected_added_migrations=(59, 60, 61),
                candidate_added_migrations=(59, 60),
            )

    def test_cli_repeated_added_migrations_replace_the_legacy_default(self):
        required = [
            "probe_upgrade_startup.py",
            "--candidate",
            "candidate.exe",
            "--previous",
            "previous.exe",
            "--source-db",
            "state.sqlite",
            "--output",
            "fresh-output",
        ]
        for flags, expected in (
            ([], None),
            (
                [
                    "--expected-added-migration",
                    "59",
                    "--expected-added-migration",
                    "60",
                    "--expected-added-migration",
                    "61",
                ],
                [59, 60, 61],
            ),
        ):
            with self.subTest(flags=flags):
                with (
                    patch("sys.argv", required + flags),
                    patch.object(probe, "run_probe", return_value={}) as run,
                    patch("builtins.print"),
                ):
                    probe.main()
                self.assertEqual(
                    run.call_args.args[0].expected_added_migration, expected
                )

    def test_third_startup_data_loss_is_a_failure_with_readback_receipt(self):
        with self.assertRaisesRegex(RuntimeError, "evidence changed"):
            self.run_synthetic_probe(damage_final=True)
        report = json.loads(
            (self.root / "output" / "receipt.json").read_text(encoding="utf-8")
        )
        self.assertFalse(report["passed"])
        self.assertEqual(report["after_candidate_after_previous"]["attachment_rows"], 0)
        self.assertTrue(report["untouched_backups_verified"])

    def test_untouched_backup_tampering_fails_final_receipt(self):
        with self.assertRaisesRegex(RuntimeError, "untouched backup changed"):
            self.run_synthetic_probe(damage_backup=True)
        report = json.loads(
            (self.root / "output" / "receipt.json").read_text(encoding="utf-8")
        )
        self.assertFalse(report["passed"])
        self.assertFalse(report["untouched_backups_verified"])

    def test_recovery_backup_fails_even_with_preserved_rows_and_good_quick_check(self):
        with self.assertRaisesRegex(RuntimeError, "recovery backup"):
            self.run_synthetic_probe(recover=True)

    def test_candidate_added_companion_migration_must_survive_previous_startup(self):
        with self.assertRaisesRegex(RuntimeError, "ledger changed after initial"):
            self.run_synthetic_probe(damage_companion=True)

    def test_failed_startup_still_reads_primary_and_companion_copies(self):
        with self.assertRaisesRegex(RuntimeError, "synthetic startup failure"):
            self.run_synthetic_probe(fail_startup=True)
        report = json.loads(
            (self.root / "output" / "receipt.json").read_text(encoding="utf-8")
        )
        self.assertFalse(report["passed"])
        self.assertEqual(report["after_previous"]["quick_check"], "ok")
        self.assertEqual(
            report["companions"]["companion.sqlite"]["after_previous"]["quick_check"],
            "ok",
        )
        self.assertTrue(report["untouched_backups_verified"])

    def test_backup_fingerprint_detects_wal_changes_while_main_file_is_unchanged(self):
        db = sqlite3.connect(self.source)
        try:
            db.execute("PRAGMA journal_mode=WAL")
            before = probe.backup_fingerprint(self.source)
            with db:
                db.execute("UPDATE threads SET title='wal-only change'")
            after = probe.backup_fingerprint(self.source)
            self.assertEqual(before[self.source.name], after[self.source.name])
            self.assertNotEqual(before, after)
            self.assertIn(self.source.name + "-wal", after)
        finally:
            db.close()

    def test_child_environment_removes_auth_and_test_config_markers(self):
        home = self.root / "isolated-home"
        excluded_names = (
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "OPENAI_FEDERATION_RULE_ID",
            "OPENAI_IDENTITY_TOKEN_FILE",
            "OPENAI_WORKLOAD_IDENTITY_CONTEXT",
            "CODEX_APP_SERVER_TEST_USER_CONFIG_FILE",
            "CODEX_APP_SERVER_MANAGED_CONFIG_PATH",
            "CODEX_APP_SERVER_DISABLE_MANAGED_CONFIG",
            "CODEX_CLI_PATH",
        )
        parent = {name.lower(): "synthetic-secret" for name in excluded_names}
        parent.update(
            PATH="synthetic-path",
            codex_home="outside",
            codex_sqlite_home="outside",
            otel_sdk_disabled="false",
            codex_internal_app_server_remote_control_disabled="0",
        )
        before = dict(parent)

        self.assertEqual(
            probe.child_environment(home, parent),
            {
                "PATH": "synthetic-path",
                "CODEX_HOME": str(home),
                "CODEX_SQLITE_HOME": str(home),
                "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED": "1",
                "OTEL_SDK_DISABLED": "true",
            },
        )
        self.assertEqual(parent, before)

    @unittest.skipUnless(os.name == "nt", "Windows Known Folder ABI fixture")
    def test_windows_config_directory_uses_known_folder_and_frees_result(self):
        program_data = self.root / "synthetic-program-data"
        buffer = probe.ctypes.create_unicode_buffer(str(program_data))

        for status in (0, 1):
            with self.subTest(status=status):
                shell, ole = Mock(), Mock()

                def query(folder_id, flags, token, returned):
                    self.assertEqual(
                        probe.ctypes.string_at(folder_id, 16),
                        probe.UUID("62ab5d82-fdc1-4dc3-a9dd-070d1d495d97").bytes_le,
                    )
                    self.assertEqual((flags, token), (0, None))
                    probe.ctypes.cast(
                        returned, probe.ctypes.POINTER(probe.ctypes.c_void_p)
                    ).contents.value = probe.ctypes.addressof(buffer)
                    return status

                shell.SHGetKnownFolderPath.side_effect = query
                with (
                    patch.object(probe.ctypes, "WinDLL", side_effect=[shell, ole]),
                    patch.object(
                        probe.os, "environ", {"ProgramData": "must-not-be-used"}
                    ),
                ):
                    if status == 0:
                        self.assertEqual(
                            self.resolve_windows_system_config_directory(),
                            program_data / "OpenAI" / "Codex",
                        )
                    else:
                        with self.assertRaisesRegex(
                            RuntimeError, "cannot resolve Windows ProgramData"
                        ):
                            self.resolve_windows_system_config_directory()
                ole.CoTaskMemFree.assert_called_once()
                self.assertEqual(
                    ole.CoTaskMemFree.call_args.args[0].value,
                    probe.ctypes.addressof(buffer),
                )

    def test_absent_system_policy_allows_preflight_without_creating_files(self):
        probe.preflight_system_configuration()
        self.assertFalse(self.system_config_directory.exists())

    def test_system_policy_presence_refuses_before_backup_or_process(self):
        executable = self.root / "not-an-executable"
        executable.touch()
        cases = (
            ("requirements.toml", 'sqlite_home="C:/outside"\n'),
            ("requirements.toml", 'cli_auth_credentials_store="auto"\n'),
            ("requirements.toml", 'log_dir="C:/outside"\n'),
            (
                "config.toml",
                'model_provider="custom"\n[model_providers.custom]\n'
                'env_key="SYNTHETIC_PROVIDER_SECRET"\n',
            ),
            # Conservative cost: even harmless settings require separate qualification.
            ("requirements.toml", 'allowed_approval_policies=["on-request"]\n'),
            ("config.toml", 'model_provider="openai"\n'),
        )
        for index, (name, contents) in enumerate(cases):
            with self.subTest(name=name, index=index):
                directory = self.root / f"policy-{index}"
                directory.mkdir()
                (directory / name).write_text(contents, encoding="utf-8")
                args = Namespace(
                    candidate=executable,
                    previous=executable,
                    source_db=self.source,
                    source_companion_db=[],
                    output=self.root / f"refused-output-{index}",
                )
                with (
                    patch.object(
                        probe, "windows_system_config_directory", return_value=directory
                    ),
                    patch.object(probe, "backup") as backup,
                    patch.object(probe, "startup") as startup,
                    patch.object(probe.subprocess, "Popen") as popen,
                ):
                    with self.assertRaisesRegex(RuntimeError, f"Windows system {name}"):
                        probe.run_probe(args)
                backup.assert_not_called()
                startup.assert_not_called()
                popen.assert_not_called()
                self.assertFalse(args.output.exists())

    def test_unknown_system_policy_presence_refuses(self):
        with patch.object(Path, "lstat", side_effect=PermissionError("synthetic")):
            with self.assertRaisesRegex(RuntimeError, "cannot establish absence"):
                probe.preflight_system_configuration()

    def test_startup_rechecks_system_policy_before_process(self):
        self.system_config_directory.mkdir()
        (self.system_config_directory / "requirements.toml").write_text(
            "", encoding="utf-8"
        )
        with patch.object(probe.subprocess, "Popen") as popen:
            with self.assertRaisesRegex(
                RuntimeError, "Windows system requirements.toml"
            ):
                probe.startup(self.root / "not-an-executable", self.root / "home")
        popen.assert_not_called()

    def test_startup_refuses_existing_trial_auth_without_reading_it(self):
        home = self.root / "isolated-home"
        home.mkdir()
        (home / "auth.json").write_text('{"OPENAI_API_KEY":null,"tokens":null}')
        with patch.object(probe.subprocess, "Popen") as popen:
            with self.assertRaisesRegex(RuntimeError, "trial auth.json"):
                probe.startup(self.root / "not-an-executable", home)
        popen.assert_not_called()

    def test_startup_contains_paths_and_leaves_auth_absent(self):
        home = self.root / "isolated-home"
        home.mkdir()
        executable = self.root / "not-an-executable"
        with (
            patch.object(
                probe.os, "environ", {"CODEX_ACCESS_TOKEN": "synthetic-secret"}
            ),
            patch.object(probe.subprocess, "CREATE_NO_WINDOW", 0, create=True),
            patch.object(
                probe.subprocess,
                "Popen",
                side_effect=RuntimeError("synthetic launch boundary"),
            ) as popen,
        ):
            with self.assertRaisesRegex(RuntimeError, "synthetic launch boundary"):
                probe.startup(executable, home)
        self.assertEqual(
            popen.call_args.args[0],
            [
                str(executable),
                "-c",
                'cli_auth_credentials_store="file"',
                "-c",
                f"sqlite_home={json.dumps(str(home))}",
                "-c",
                f"log_dir={json.dumps(str(home / 'log'))}",
                "app-server",
            ],
        )
        self.assertEqual(popen.call_args.kwargs["cwd"], home)
        self.assertNotIn("CODEX_ACCESS_TOKEN", popen.call_args.kwargs["env"])
        self.assertFalse((home / "auth.json").exists())

    def test_auth_material_created_by_startup_is_a_failed_rehearsal(self):
        executable = self.root / "not-an-executable"
        executable.touch()
        args = Namespace(
            candidate=executable,
            previous=executable,
            source_db=self.source,
            source_companion_db=[],
            output=self.root / "output",
        )

        def startup(_executable, home, *, check_attachments=False):
            migrate(home / self.source.name)
            (home / "auth.json").write_text("null", encoding="utf-8")
            return {"synthetic": True}

        with patch.object(probe, "startup", side_effect=startup) as mocked:
            with self.assertRaisesRegex(RuntimeError, "trial auth.json after startup"):
                probe.run_probe(args)
        self.assertEqual(mocked.call_count, 1)
        receipt = json.loads((args.output / "receipt.json").read_text(encoding="utf-8"))
        self.assertFalse(receipt["passed"])
        self.assertTrue(receipt["untouched_backups_verified"])


if __name__ == "__main__":
    unittest.main()
