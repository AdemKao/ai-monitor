use std::fs;
use std::path::{Path, PathBuf};

use ai_monitor::model::Usage;
use ai_monitor::opencode::{INDEX_NAME, OpenCodeProvider, discover_db_path};
use chrono::{Duration, Local, TimeZone};
use rusqlite::{Connection, params};
use serde_json::json;
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    db_path: PathBuf,
    repo: PathBuf,
    nested: PathBuf,
    other: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary directory");
        let repo = dir.path().join("repo");
        let nested = repo.join("nested");
        let other = dir.path().join("other");
        fs::create_dir_all(&nested).expect("nested directory");
        fs::create_dir_all(&other).expect("other directory");

        let db_path = dir.path().join("opencode.db");
        let connection = Connection::open(&db_path).expect("temporary database");
        connection
            .execute_batch(
                "
                CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    directory TEXT NOT NULL
                );
                CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
                ",
            )
            .expect("OpenCode schema");

        Self {
            _dir: dir,
            db_path,
            repo,
            nested,
            other,
        }
    }

    fn provider(&self) -> OpenCodeProvider {
        OpenCodeProvider::with_db_path(self.db_path.clone())
    }

    fn add_session(&self, id: &str, project_id: &str, directory: &Path) {
        let connection = Connection::open(&self.db_path).expect("database connection");
        connection
            .execute(
                "INSERT INTO session (id, project_id, directory) VALUES (?1, ?2, ?3)",
                params![id, project_id, directory.to_string_lossy()],
            )
            .expect("session row");
    }

    fn add_message<T: serde::Serialize>(&self, id: &str, session_id: &str, days_ago: i64, data: T) {
        let connection = Connection::open(&self.db_path).expect("database connection");
        let timestamp = local_millis(days_ago);
        let data = serde_json::to_string(&data).expect("message JSON");
        connection
            .execute(
                "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
                params![id, session_id, timestamp, data],
            )
            .expect("message row");
    }

    fn add_raw_message(&self, id: &str, session_id: &str, days_ago: i64, data: &str) {
        let connection = Connection::open(&self.db_path).expect("database connection");
        connection
            .execute(
                "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
                params![id, session_id, local_millis(days_ago), data],
            )
            .expect("raw message row");
    }
}

fn local_millis(days_ago: i64) -> i64 {
    let date = Local::now().date_naive() - Duration::days(days_ago);
    Local
        .from_local_datetime(&date.and_hms_opt(12, 0, 0).expect("valid time"))
        .single()
        .expect("local timestamp")
        .timestamp_millis()
}

fn current_message(input: u64, output: u64, cost: f64) -> serde_json::Value {
    json!({
        "role": "assistant",
        "providerID": "openai",
        "modelID": "gpt-5",
        "cost": cost,
        "tokens": {
            "input": input,
            "output": output,
            "reasoning": 3,
            "cache": { "read": 4, "write": 5 }
        }
    })
}

#[test]
fn aggregates_assistant_usage_and_filters_local_days() {
    let fixture = Fixture::new();
    fixture.add_session("s1", "project-a", &fixture.repo);
    fixture.add_session("s2", "project-a", &fixture.nested);
    fixture.add_session("s3", "project-b", &fixture.other);

    fixture.add_message("m1", "s1", 0, current_message(10, 20, 1.25));
    fixture.add_message("m2", "s2", 0, current_message(5, 7, 0.75));
    fixture.add_message(
        "m3",
        "s1",
        1,
        json!({
            "role": "assistant",
            "metadata": {
                "assistant": {
                    "providerId": "anthropic",
                    "modelId": "claude-sonnet",
                    "cost": 2.0,
                    "tokens": {
                        "input_tokens": 8,
                        "output_tokens": 9,
                        "reasoning_tokens": 2,
                        "cache": { "read": 1, "write": 6 }
                    }
                }
            }
        }),
    );
    fixture.add_message(
        "m4",
        "s1",
        3,
        json!({
            "role": "assistant",
            "provider": "ignored-out-of-range",
            "model": "ignored-out-of-range",
            "tokens": { "input": 1000 }
        }),
    );
    fixture.add_message(
        "m5",
        "s1",
        0,
        json!({
            "role": "user",
            "providerID": "openai",
            "modelID": "gpt-5",
            "tokens": { "input": 999 }
        }),
    );
    fixture.add_raw_message("m6", "s1", 0, "{ \"secret\": \"must-not-be-an-error\" ");

    let report = fixture
        .provider()
        .usage(2, true, None)
        .expect("usage report");

    assert_eq!(report.source, "opencode");
    assert_eq!(
        report.start_day,
        (Local::now().date_naive() - Duration::days(1)).to_string()
    );
    assert_eq!(report.end_day, Local::now().date_naive().to_string());
    assert_eq!(report.rows.len(), 2);

    let openai = report
        .rows
        .iter()
        .find(|row| row.provider == "openai")
        .expect("openai row");
    assert_eq!(
        openai.usage,
        Usage {
            messages: 2,
            input_tokens: 15,
            output_tokens: 27,
            reasoning_tokens: 6,
            cache_read_tokens: 8,
            cache_write_tokens: 10,
            cost_usd: 2.0,
        }
    );

    let anthropic = report
        .rows
        .iter()
        .find(|row| row.provider == "anthropic")
        .expect("anthropic row");
    assert_eq!(anthropic.usage.messages, 1);
    assert_eq!(anthropic.usage.input_tokens, 8);
    assert_eq!(anthropic.usage.output_tokens, 9);
    assert_eq!(anthropic.usage.reasoning_tokens, 2);
    assert_eq!(anthropic.usage.cache_read_tokens, 1);
    assert_eq!(anthropic.usage.cache_write_tokens, 6);
    assert_eq!(anthropic.usage.cost_usd, 2.0);
}

#[test]
fn resolves_exact_project_then_longest_parent_and_keeps_project_sessions() {
    let fixture = Fixture::new();
    fixture.add_session("root", "project-root", &fixture.repo);
    fixture.add_session("nested", "project-nested", &fixture.nested);
    fixture.add_session("nested-other", "project-nested", &fixture.other);
    fixture.add_message("root-message", "root", 0, current_message(1, 1, 1.0));
    fixture.add_message("nested-message", "nested", 0, current_message(2, 2, 2.0));
    fixture.add_message(
        "nested-other-message",
        "nested-other",
        0,
        current_message(4, 4, 4.0),
    );

    let exact = fixture
        .provider()
        .usage(1, false, Some(&fixture.repo))
        .expect("exact project usage");
    assert_eq!(exact.rows[0].usage.messages, 1);
    assert_eq!(exact.rows[0].usage.input_tokens, 1);

    let child = fixture.nested.join("child");
    let longest_parent = fixture
        .provider()
        .usage(1, false, Some(&child))
        .expect("longest parent usage");
    assert_eq!(longest_parent.rows.len(), 1);
    assert_eq!(longest_parent.rows[0].usage.messages, 2);
    assert_eq!(longest_parent.rows[0].usage.input_tokens, 6);
}

#[test]
fn usage_does_not_create_index_and_index_lifecycle_is_explicit() {
    let fixture = Fixture::new();
    fixture.add_session("s1", "project-a", &fixture.repo);
    fixture.add_message("m1", "s1", 0, current_message(1, 2, 0.5));
    let provider = fixture.provider();

    assert!(!provider.index_status().expect("initial index status"));
    provider.usage(1, true, None).expect("read-only usage");
    assert!(!provider.index_status().expect("index remains absent"));

    provider.create_index().expect("create index");
    assert!(provider.index_status().expect("created index status"));
    let connection = Connection::open(&fixture.db_path).expect("database connection");
    let indexed_columns = connection
        .prepare("PRAGMA index_info(ai_monitor_message_time_created_idx)")
        .expect("index info query")
        .query_map([], |row| row.get::<_, String>(2))
        .expect("index info rows")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("index columns");
    assert_eq!(indexed_columns, vec!["time_created"]);

    provider.remove_index().expect("remove index");
    assert!(!provider.index_status().expect("removed index status"));
    assert_eq!(INDEX_NAME, "ai_monitor_message_time_created_idx");
}

#[test]
fn explicit_database_path_wins_and_invalid_numbers_are_ignored_safely() {
    let fixture = Fixture::new();
    assert_eq!(
        discover_db_path(Some(&fixture.db_path)).expect("explicit path"),
        fixture.db_path
    );

    fixture.add_session("s1", "project-a", &fixture.repo);
    fixture.add_message(
        "m1",
        "s1",
        0,
        json!({
            "role": "assistant",
            "provider": { "id": "provider" },
            "model": { "id": "model" },
            "cost_usd": -1.0,
            "tokens": {
                "input_tokens": -10,
                "output_tokens": "not-a-number",
                "reasoning_tokens": 1.5,
                "cache_read_tokens": 2,
                "cache_write_tokens": "3"
            }
        }),
    );

    let report = fixture
        .provider()
        .usage(1, true, None)
        .expect("safe numeric usage");
    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.rows[0].usage.messages, 1);
    assert_eq!(report.rows[0].usage.input_tokens, 0);
    assert_eq!(report.rows[0].usage.output_tokens, 0);
    assert_eq!(report.rows[0].usage.reasoning_tokens, 0);
    assert_eq!(report.rows[0].usage.cache_read_tokens, 2);
    assert_eq!(report.rows[0].usage.cache_write_tokens, 3);
    assert_eq!(report.rows[0].usage.cost_usd, 0.0);
}
