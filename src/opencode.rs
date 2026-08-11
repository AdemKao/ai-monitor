use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Duration, Local, LocalResult, NaiveDate, TimeZone};
use directories::BaseDirs;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::model::{BreakdownUsage, Usage, UsageReport, UsageRow};

pub const INDEX_NAME: &str = "ai_monitor_message_time_created_idx";
const DB_ENV: &str = "AI_MONITOR_OPENCODE_DB";
const SOURCE: &str = "opencode";
const AGENT_PATHS: &[&[&str]] = &[&["agent"]];

#[derive(Debug, Error)]
pub enum Error {
    #[error("days must be greater than zero")]
    InvalidDays,
    #[error("could not determine the OpenCode database path")]
    DatabasePath,
    #[error("the opencode db path command failed")]
    DatabasePathCommand(#[source] std::io::Error),
    #[error("the opencode db path command returned no path")]
    EmptyDatabasePath,
    #[error("could not open the OpenCode database")]
    OpenDatabase(#[source] rusqlite::Error),
    #[error("the OpenCode database has an unsupported schema")]
    InvalidSchema,
    #[error("the local date range is outside the supported date range")]
    InvalidDateRange,
    #[error("the requested project could not be resolved")]
    ProjectResolution(#[source] rusqlite::Error),
    #[error("the OpenCode database query failed")]
    Query(#[source] rusqlite::Error),
    #[error("the OpenCode database index operation failed")]
    Index(#[source] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize)]
pub struct DashboardReport {
    pub source: String,
    pub start_day: String,
    pub end_day: String,
    pub scope: String,
    pub totals: BreakdownUsage,
    pub projects: Vec<DashboardProject>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DashboardProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub usage: BreakdownUsage,
    pub agents: Vec<DashboardAgent>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DashboardAgent {
    pub name: String,
    pub kind: String,
    pub usage: BreakdownUsage,
}

#[derive(Clone, Debug, Default)]
pub struct OpenCodeProvider {
    explicit_db_path: Option<PathBuf>,
}

impl OpenCodeProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_db_path(path: impl Into<PathBuf>) -> Self {
        Self {
            explicit_db_path: Some(path.into()),
        }
    }

    pub fn usage(
        &self,
        days: u32,
        all_projects: bool,
        project: Option<&Path>,
    ) -> Result<UsageReport> {
        let range = DateRange::for_days(days, Local::now())?;
        let path = discover_db_path(self.explicit_db_path.as_deref())?;
        let connection = open_database(&path, true)?;
        validate_schema(&connection)?;

        let (project_id, project_path) = if all_projects {
            (None, None)
        } else {
            let target = project
                .map(normalize_path)
                .transpose()
                .map_err(|_| Error::DatabasePath)?
                .unwrap_or(
                    normalize_path(&env::current_dir().map_err(|_| Error::DatabasePath)?)
                        .map_err(|_| Error::DatabasePath)?,
                );
            (resolve_project_id(&connection, &target)?, Some(target))
        };

        let mut aggregates = HashMap::<AggregateKey, Usage>::new();
        if all_projects || project_id.is_some() {
            stream_usage_rows(&connection, &range, project_id.as_deref(), &mut aggregates)?;
        }

        let mut rows = aggregates
            .into_iter()
            .map(|(key, usage)| UsageRow {
                day: key.day,
                provider: key.provider,
                model: key.model,
                usage,
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            left.day
                .cmp(&right.day)
                .then_with(|| left.provider.cmp(&right.provider))
                .then_with(|| left.model.cmp(&right.model))
        });

        let scope = if all_projects {
            "all-projects".to_owned()
        } else {
            project_path
                .expect("project path is set for project-scoped reports")
                .to_string_lossy()
                .into_owned()
        };

        Ok(UsageReport {
            source: SOURCE.to_owned(),
            start_day: range.start_day,
            end_day: range.end_day,
            scope,
            rows,
        })
    }

    pub fn dashboard(
        &self,
        days: u32,
        all_projects: bool,
        project: Option<&Path>,
    ) -> Result<DashboardReport> {
        let range = DateRange::for_days(days, Local::now())?;
        let path = discover_db_path(self.explicit_db_path.as_deref())?;
        let connection = open_database(&path, true)?;
        validate_schema(&connection)?;

        let (project_id, project_path) = if all_projects {
            (None, None)
        } else {
            let target = project
                .map(normalize_path)
                .transpose()
                .map_err(|_| Error::DatabasePath)?
                .unwrap_or(
                    normalize_path(&env::current_dir().map_err(|_| Error::DatabasePath)?)
                        .map_err(|_| Error::DatabasePath)?,
                );
            (resolve_project_id(&connection, &target)?, Some(target))
        };

        let project_metadata = load_project_metadata(&connection)?;
        let session_columns = table_columns(&connection, "session")?;
        let mut totals = DashboardAggregate::default();
        let mut projects = HashMap::<String, DashboardAggregateProject>::new();
        stream_dashboard_rows(
            &connection,
            &range,
            project_id.as_deref(),
            &session_columns,
            &project_metadata,
            &mut totals,
            &mut projects,
        )?;

        let mut projects = projects
            .into_iter()
            .map(|(id, project)| project.finish(id))
            .collect::<Vec<_>>();
        let include_path = project_path
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "all-projects".to_owned());
        projects.sort_by(|left, right| {
            right
                .usage
                .active_tokens()
                .cmp(&left.usage.active_tokens())
                .then_with(|| left.name.cmp(&right.name))
        });

        Ok(DashboardReport {
            source: SOURCE.to_owned(),
            start_day: range.start_day,
            end_day: range.end_day,
            scope: include_path,
            totals: totals.finish(),
            projects,
        })
    }

    pub fn index_status(&self) -> Result<bool> {
        let path = discover_db_path(self.explicit_db_path.as_deref())?;
        let connection = open_database(&path, true)?;
        validate_schema(&connection)?;
        index_exists(&connection)
    }

    pub fn create_index(&self) -> Result<()> {
        let path = discover_db_path(self.explicit_db_path.as_deref())?;
        let connection = open_database(&path, false)?;
        validate_schema(&connection)?;
        connection
            .execute(
                "CREATE INDEX IF NOT EXISTS ai_monitor_message_time_created_idx ON message(time_created)",
                [],
            )
            .map_err(Error::Index)?;
        Ok(())
    }

    pub fn remove_index(&self) -> Result<()> {
        let path = discover_db_path(self.explicit_db_path.as_deref())?;
        let connection = open_database(&path, false)?;
        validate_schema(&connection)?;
        connection
            .execute(
                "DROP INDEX IF EXISTS ai_monitor_message_time_created_idx",
                [],
            )
            .map_err(Error::Index)?;
        Ok(())
    }
}

pub fn discover_db_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }

    if let Some(path) = env::var_os(DB_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    if let Some(home) = BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
        let standard = home.join(".local/share/opencode/opencode.db");
        if standard.is_file() {
            return Ok(standard);
        }
    }

    let output = Command::new("opencode")
        .args(["db", "path"])
        .output()
        .map_err(Error::DatabasePathCommand)?;
    if !output.status.success() {
        return Err(Error::DatabasePath);
    }

    output
        .stdout
        .split(|byte| *byte == b'\n' || *byte == b'\r')
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(PathBuf::from)
        .ok_or(Error::EmptyDatabasePath)
}

fn open_database(path: &Path, read_only: bool) -> Result<Connection> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    Connection::open_with_flags(path, flags).map_err(Error::OpenDatabase)
}

fn validate_schema(connection: &Connection) -> Result<()> {
    let required = [
        ("session", ["id", "project_id", "directory"]),
        ("message", ["session_id", "time_created", "data"]),
    ];

    for (table, columns) in required {
        let query = match table {
            "session" => "PRAGMA table_info(session)",
            "message" => "PRAGMA table_info(message)",
            _ => return Err(Error::InvalidSchema),
        };
        let mut statement = connection.prepare(query).map_err(Error::Query)?;
        let mut rows = statement.query([]).map_err(Error::Query)?;
        let mut found = [false; 3];
        while let Some(row) = rows.next().map_err(Error::Query)? {
            let name = row.get::<_, String>(1).map_err(Error::Query)?;
            for (index, required_column) in columns.iter().enumerate() {
                if name == *required_column {
                    found[index] = true;
                }
            }
        }
        if found.iter().any(|present| !present) {
            return Err(Error::InvalidSchema);
        }
    }

    Ok(())
}

fn index_exists(connection: &Connection) -> Result<bool> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1",
            [INDEX_NAME],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(Error::Query)
}

#[derive(Clone, Debug)]
struct DateRange {
    start_day: String,
    end_day: String,
    start_millis: i64,
    end_millis: i64,
}

impl DateRange {
    fn for_days(days: u32, now: DateTime<Local>) -> Result<Self> {
        if days == 0 {
            return Err(Error::InvalidDays);
        }

        let end = now.date_naive();
        let start = end
            .checked_sub_signed(Duration::days(i64::from(days - 1)))
            .ok_or(Error::InvalidDateRange)?;
        let end_exclusive = end
            .checked_add_signed(Duration::days(1))
            .ok_or(Error::InvalidDateRange)?;

        Ok(Self {
            start_day: start.to_string(),
            end_day: end.to_string(),
            start_millis: local_midnight(start)?.timestamp_millis(),
            end_millis: local_midnight(end_exclusive)?.timestamp_millis(),
        })
    }
}

fn local_midnight(date: NaiveDate) -> Result<DateTime<Local>> {
    let value = date.and_hms_opt(0, 0, 0).ok_or(Error::InvalidDateRange)?;
    match Local.from_local_datetime(&value) {
        LocalResult::Single(datetime) | LocalResult::Ambiguous(datetime, _) => Ok(datetime),
        LocalResult::None => Err(Error::InvalidDateRange),
    }
}

fn resolve_project_id(connection: &Connection, target: &Path) -> Result<Option<String>> {
    let mut statement = connection
        .prepare("SELECT project_id, directory FROM session")
        .map_err(Error::ProjectResolution)?;
    let mut rows = statement.query([]).map_err(Error::ProjectResolution)?;
    let mut exact = None;
    let mut longest_parent = None::<(usize, String)>;

    while let Some(row) = rows.next().map_err(Error::ProjectResolution)? {
        let project_id = text_value(row.get_ref(0).map_err(Error::ProjectResolution)?);
        let directory = text_value(row.get_ref(1).map_err(Error::ProjectResolution)?);
        let (Some(project_id), Some(directory)) = (project_id, directory) else {
            continue;
        };
        let directory = normalize_path(Path::new(&directory)).map_err(|_| Error::DatabasePath)?;

        if directory == target {
            if exact.is_none() {
                exact = Some(project_id);
            }
            continue;
        }

        if target.starts_with(&directory) {
            let depth = directory.components().count();
            if longest_parent
                .as_ref()
                .is_none_or(|(current_depth, _)| depth > *current_depth)
            {
                longest_parent = Some((depth, project_id));
            }
        }
    }

    Ok(exact.or_else(|| longest_parent.map(|(_, project_id)| project_id)))
}

fn normalize_path(path: &Path) -> std::io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }

    // Resolve symlinks in the nearest existing ancestor while preserving a
    // possibly non-existent project child path.
    let mut ancestor = path.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            break;
        };
        missing.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            break;
        };
        ancestor = parent;
    }
    if let Ok(mut canonical) = ancestor.canonicalize() {
        for component in missing.into_iter().rev() {
            canonical.push(component);
        }
        return Ok(canonical);
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct AggregateKey {
    day: String,
    provider: String,
    model: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct DashboardAgentKey {
    name: String,
    kind: String,
}

#[derive(Default)]
struct DashboardAggregate {
    usage: BreakdownUsage,
    sessions: HashSet<String>,
}

impl DashboardAggregate {
    fn add_call(&mut self, session_id: &str, usage: &Usage) {
        self.usage.add_usage(usage);
        if self.sessions.insert(session_id.to_owned()) {
            self.usage.sessions = self.usage.sessions.saturating_add(1);
        }
    }

    fn finish(self) -> BreakdownUsage {
        self.usage
    }
}

struct DashboardAggregateProject {
    name: String,
    path: String,
    usage: DashboardAggregate,
    agents: HashMap<DashboardAgentKey, DashboardAggregate>,
}

impl DashboardAggregateProject {
    fn new(name: String, path: String) -> Self {
        Self {
            name,
            path,
            usage: DashboardAggregate::default(),
            agents: HashMap::new(),
        }
    }

    fn finish(self, id: String) -> DashboardProject {
        let mut agents = self
            .agents
            .into_iter()
            .map(|(key, aggregate)| DashboardAgent {
                name: key.name,
                kind: key.kind,
                usage: aggregate.finish(),
            })
            .collect::<Vec<_>>();
        agents.sort_by(|left, right| {
            right
                .usage
                .active_tokens()
                .cmp(&left.usage.active_tokens())
                .then_with(|| left.name.cmp(&right.name))
        });
        DashboardProject {
            id,
            name: self.name,
            path: self.path,
            usage: self.usage.finish(),
            agents,
        }
    }
}

type ProjectMetadata = HashMap<String, (String, String)>;

fn stream_dashboard_rows(
    connection: &Connection,
    range: &DateRange,
    project_id: Option<&str>,
    session_columns: &HashSet<String>,
    project_metadata: &ProjectMetadata,
    totals: &mut DashboardAggregate,
    projects: &mut HashMap<String, DashboardAggregateProject>,
) -> Result<()> {
    let parent_expression = if session_columns.contains("parent_id") {
        "s.parent_id"
    } else {
        "NULL"
    };
    let session_agent_expression = if session_columns.contains("agent") {
        "s.agent"
    } else {
        "NULL"
    };
    let (query, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match project_id {
        Some(project_id) => (
            format!(
                "SELECT m.session_id, m.data, s.project_id, s.directory, {parent_expression}, {session_agent_expression} \
                 FROM message AS m \
                 JOIN session AS s ON s.id = m.session_id \
                 WHERE m.time_created >= ?1 \
                   AND m.time_created < ?2 \
                   AND s.project_id = ?3"
            ),
            vec![
                Box::new(range.start_millis),
                Box::new(range.end_millis),
                Box::new(project_id.to_owned()),
            ],
        ),
        None => (
            format!(
                "SELECT m.session_id, m.data, s.project_id, s.directory, {parent_expression}, {session_agent_expression} \
                 FROM message AS m \
                 JOIN session AS s ON s.id = m.session_id \
                 WHERE m.time_created >= ?1 \
                   AND m.time_created < ?2"
            ),
            vec![Box::new(range.start_millis), Box::new(range.end_millis)],
        ),
    };

    let mut statement = connection.prepare(&query).map_err(Error::Query)?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(
            params.iter().map(|param| param.as_ref()),
        ))
        .map_err(Error::Query)?;
    while let Some(row) = rows.next().map_err(Error::Query)? {
        let Some(session_id) = text_value(row.get_ref(0).map_err(Error::Query)?) else {
            continue;
        };
        let data = match row.get_ref(1).map_err(Error::Query)? {
            ValueRef::Text(value) | ValueRef::Blob(value) => value,
            _ => continue,
        };
        let Ok(value) = serde_json::from_slice::<Value>(data) else {
            continue;
        };
        let Some((_, _, usage)) = assistant_usage(&value) else {
            continue;
        };
        let project_id = text_value(row.get_ref(2).map_err(Error::Query)?).unwrap_or_default();
        let directory = text_value(row.get_ref(3).map_err(Error::Query)?).unwrap_or_default();
        let parent_id = text_value(row.get_ref(4).map_err(Error::Query)?);
        let session_agent = text_value(row.get_ref(5).map_err(Error::Query)?);
        let agent = find_string(&value, AGENT_PATHS)
            .or(session_agent)
            .unwrap_or_else(|| "unknown".to_owned());
        if agent == "compaction" {
            continue;
        }
        let kind = if parent_id.is_some() || agent.starts_with("subagents/") {
            "subagent"
        } else {
            "agent"
        };
        let (metadata_name, metadata_path) = project_metadata
            .get(&project_id)
            .cloned()
            .unwrap_or_else(|| (String::new(), directory));
        let path = if metadata_path.is_empty() {
            "unknown".to_owned()
        } else {
            metadata_path
        };
        let name = project_name(&project_id, &path, &metadata_name);
        let project = projects
            .entry(project_id.clone())
            .or_insert_with(|| DashboardAggregateProject::new(name, path));
        project.usage.add_call(&session_id, &usage);
        project
            .agents
            .entry(DashboardAgentKey {
                name: agent,
                kind: kind.to_owned(),
            })
            .or_default()
            .add_call(&session_id, &usage);
        totals.add_call(&session_id, &usage);
    }
    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> Result<HashSet<String>> {
    let query = match table {
        "project" => "PRAGMA table_info(project)",
        "session" => "PRAGMA table_info(session)",
        _ => return Err(Error::InvalidSchema),
    };
    let mut statement = connection.prepare(query).map_err(Error::Query)?;
    let mut rows = statement.query([]).map_err(Error::Query)?;
    let mut columns = HashSet::new();
    while let Some(row) = rows.next().map_err(Error::Query)? {
        columns.insert(row.get::<_, String>(1).map_err(Error::Query)?);
    }
    Ok(columns)
}

fn load_project_metadata(connection: &Connection) -> Result<ProjectMetadata> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'project'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(Error::Query)?
        .is_some();
    if !exists {
        return Ok(HashMap::new());
    }

    let columns = table_columns(connection, "project")?;
    if !columns.contains("id") || !columns.contains("worktree") {
        return Ok(HashMap::new());
    }
    let name_expression = if columns.contains("name") {
        "name"
    } else {
        "NULL"
    };
    let mut statement = connection
        .prepare(&format!(
            "SELECT id, {name_expression}, worktree FROM project"
        ))
        .map_err(Error::Query)?;
    let mut rows = statement.query([]).map_err(Error::Query)?;
    let mut metadata = HashMap::new();
    while let Some(row) = rows.next().map_err(Error::Query)? {
        let id = row.get::<_, String>(0).map_err(Error::Query)?;
        let name = row
            .get::<_, Option<String>>(1)
            .map_err(Error::Query)?
            .unwrap_or_default();
        let path = row.get::<_, String>(2).map_err(Error::Query)?;
        metadata.insert(id, (name, path));
    }
    Ok(metadata)
}

fn project_name(id: &str, path: &str, name: &str) -> String {
    if !name.trim().is_empty() {
        return name.trim().to_owned();
    }
    if id == "global" || path == "/" {
        return "global".to_owned();
    }
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| id.to_owned())
}

fn stream_usage_rows(
    connection: &Connection,
    range: &DateRange,
    project_id: Option<&str>,
    aggregates: &mut HashMap<AggregateKey, Usage>,
) -> Result<()> {
    let (query, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match project_id {
        Some(project_id) => (
            "SELECT m.time_created, m.data \
              FROM message AS m \
              WHERE m.time_created >= ?1 \
                AND m.time_created < ?2 \
                AND m.session_id IN (SELECT id FROM session WHERE project_id = ?3)",
            vec![
                Box::new(range.start_millis),
                Box::new(range.end_millis),
                Box::new(project_id.to_owned()),
            ],
        ),
        None => (
            "SELECT m.time_created, m.data \
              FROM message AS m \
              WHERE m.time_created >= ?1 \
                AND m.time_created < ?2",
            vec![Box::new(range.start_millis), Box::new(range.end_millis)],
        ),
    };

    let mut statement = connection.prepare(query).map_err(Error::Query)?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(
            params.iter().map(|param| param.as_ref()),
        ))
        .map_err(Error::Query)?;
    while let Some(row) = rows.next().map_err(Error::Query)? {
        let timestamp = timestamp_millis(row.get_ref(0).map_err(Error::Query)?);
        let Some(timestamp) = timestamp else {
            continue;
        };
        let day = match Local.timestamp_millis_opt(timestamp) {
            LocalResult::Single(datetime) | LocalResult::Ambiguous(datetime, _) => {
                datetime.date_naive().to_string()
            }
            LocalResult::None => continue,
        };
        let data = match row.get_ref(1).map_err(Error::Query)? {
            ValueRef::Text(value) | ValueRef::Blob(value) => value,
            _ => continue,
        };
        let Ok(value) = serde_json::from_slice::<Value>(data) else {
            continue;
        };
        let Some((provider, model, usage)) = assistant_usage(&value) else {
            continue;
        };

        let key = AggregateKey {
            day,
            provider,
            model,
        };
        let aggregate = aggregates.entry(key).or_default();
        aggregate.messages = aggregate.messages.saturating_add(1);
        aggregate.input_tokens = aggregate.input_tokens.saturating_add(usage.input_tokens);
        aggregate.output_tokens = aggregate.output_tokens.saturating_add(usage.output_tokens);
        aggregate.reasoning_tokens = aggregate
            .reasoning_tokens
            .saturating_add(usage.reasoning_tokens);
        aggregate.cache_read_tokens = aggregate
            .cache_read_tokens
            .saturating_add(usage.cache_read_tokens);
        aggregate.cache_write_tokens = aggregate
            .cache_write_tokens
            .saturating_add(usage.cache_write_tokens);
        aggregate.cost_usd = safe_cost_sum(aggregate.cost_usd, usage.cost_usd);
    }
    Ok(())
}

fn assistant_usage(value: &Value) -> Option<(String, String, Usage)> {
    if value.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }

    let provider = find_string(value, PROVIDER_PATHS).unwrap_or_else(|| "unknown".to_owned());
    let model = find_string(value, MODEL_PATHS).unwrap_or_else(|| "unknown".to_owned());
    let usage = Usage {
        messages: 1,
        input_tokens: find_u64(value, INPUT_PATHS),
        output_tokens: find_u64(value, OUTPUT_PATHS),
        reasoning_tokens: find_u64(value, REASONING_PATHS),
        cache_read_tokens: find_u64(value, CACHE_READ_PATHS),
        cache_write_tokens: find_u64(value, CACHE_WRITE_PATHS),
        cost_usd: find_f64(value, COST_PATHS),
    };
    Some((provider, model, usage))
}

const PROVIDER_PATHS: &[&[&str]] = &[
    &["providerID"],
    &["providerId"],
    &["provider_id"],
    &["provider"],
    &["provider", "id"],
    &["provider", "providerID"],
    &["provider", "providerId"],
    &["provider", "provider_id"],
    &["model", "providerID"],
    &["model", "providerId"],
    &["model", "provider_id"],
    &["model", "provider"],
    &["metadata", "assistant", "providerID"],
    &["metadata", "assistant", "providerId"],
    &["metadata", "assistant", "provider_id"],
    &["metadata", "assistant", "provider"],
    &["metadata", "assistant", "provider", "id"],
    &["metadata", "assistant", "provider", "providerID"],
    &["metadata", "assistant", "provider", "providerId"],
    &["metadata", "assistant", "provider", "provider_id"],
    &["metadata", "assistant", "model", "providerID"],
    &["assistant", "providerID"],
    &["assistant", "providerId"],
    &["assistant", "provider_id"],
    &["assistant", "provider"],
    &["assistant", "provider", "id"],
    &["usage", "providerID"],
    &["usage", "providerId"],
    &["usage", "provider_id"],
    &["usage", "provider"],
];

const MODEL_PATHS: &[&[&str]] = &[
    &["modelID"],
    &["modelId"],
    &["model_id"],
    &["model"],
    &["model", "id"],
    &["model", "modelID"],
    &["model", "modelId"],
    &["model", "model_id"],
    &["metadata", "assistant", "modelID"],
    &["metadata", "assistant", "modelId"],
    &["metadata", "assistant", "model_id"],
    &["metadata", "assistant", "model"],
    &["metadata", "assistant", "model", "id"],
    &["metadata", "assistant", "model", "modelID"],
    &["assistant", "modelID"],
    &["assistant", "modelId"],
    &["assistant", "model_id"],
    &["assistant", "model"],
    &["usage", "modelID"],
    &["usage", "modelId"],
    &["usage", "model_id"],
];

const INPUT_PATHS: &[&[&str]] = &[
    &["tokens", "input"],
    &["tokens", "inputTokens"],
    &["tokens", "input_tokens"],
    &["input"],
    &["inputTokens"],
    &["input_tokens"],
    &["usage", "input"],
    &["usage", "inputTokens"],
    &["usage", "input_tokens"],
    &["metadata", "assistant", "tokens", "input"],
    &["metadata", "assistant", "tokens", "inputTokens"],
    &["metadata", "assistant", "tokens", "input_tokens"],
    &["metadata", "assistant", "input"],
    &["metadata", "assistant", "inputTokens"],
    &["metadata", "assistant", "input_tokens"],
    &["assistant", "tokens", "input"],
    &["assistant", "tokens", "inputTokens"],
    &["assistant", "tokens", "input_tokens"],
];

const OUTPUT_PATHS: &[&[&str]] = &[
    &["tokens", "output"],
    &["tokens", "outputTokens"],
    &["tokens", "output_tokens"],
    &["output"],
    &["outputTokens"],
    &["output_tokens"],
    &["usage", "output"],
    &["usage", "outputTokens"],
    &["usage", "output_tokens"],
    &["metadata", "assistant", "tokens", "output"],
    &["metadata", "assistant", "tokens", "outputTokens"],
    &["metadata", "assistant", "tokens", "output_tokens"],
    &["metadata", "assistant", "output"],
    &["metadata", "assistant", "outputTokens"],
    &["metadata", "assistant", "output_tokens"],
    &["assistant", "tokens", "output"],
    &["assistant", "tokens", "outputTokens"],
    &["assistant", "tokens", "output_tokens"],
];

const REASONING_PATHS: &[&[&str]] = &[
    &["tokens", "reasoning"],
    &["tokens", "reasoningTokens"],
    &["tokens", "reasoning_tokens"],
    &["reasoning"],
    &["reasoningTokens"],
    &["reasoning_tokens"],
    &["usage", "reasoning"],
    &["usage", "reasoningTokens"],
    &["usage", "reasoning_tokens"],
    &["metadata", "assistant", "tokens", "reasoning"],
    &["metadata", "assistant", "tokens", "reasoningTokens"],
    &["metadata", "assistant", "tokens", "reasoning_tokens"],
    &["metadata", "assistant", "reasoning"],
    &["metadata", "assistant", "reasoningTokens"],
    &["metadata", "assistant", "reasoning_tokens"],
    &["assistant", "tokens", "reasoning"],
    &["assistant", "tokens", "reasoningTokens"],
    &["assistant", "tokens", "reasoning_tokens"],
];

const CACHE_READ_PATHS: &[&[&str]] = &[
    &["tokens", "cache", "read"],
    &["tokens", "cache", "readTokens"],
    &["tokens", "cache", "read_tokens"],
    &["tokens", "cacheRead"],
    &["tokens", "cacheReadTokens"],
    &["tokens", "cache_read"],
    &["tokens", "cache_read_tokens"],
    &["cache", "read"],
    &["cache", "readTokens"],
    &["cache", "read_tokens"],
    &["cacheRead"],
    &["cacheReadTokens"],
    &["cache_read"],
    &["cache_read_tokens"],
    &["usage", "cache", "read"],
    &["usage", "cache", "readTokens"],
    &["usage", "cache", "read_tokens"],
    &["usage", "cacheRead"],
    &["usage", "cacheReadTokens"],
    &["usage", "cache_read"],
    &["usage", "cache_read_tokens"],
    &["metadata", "assistant", "tokens", "cache", "read"],
    &["metadata", "assistant", "tokens", "cache", "readTokens"],
    &["metadata", "assistant", "tokens", "cache", "read_tokens"],
    &["metadata", "assistant", "tokens", "cacheRead"],
    &["metadata", "assistant", "tokens", "cacheReadTokens"],
    &["metadata", "assistant", "tokens", "cache_read"],
    &["metadata", "assistant", "tokens", "cache_read_tokens"],
    &["metadata", "assistant", "cache", "read"],
    &["metadata", "assistant", "cacheRead"],
    &["metadata", "assistant", "cache_read"],
    &["assistant", "tokens", "cache", "read"],
    &["assistant", "tokens", "cacheRead"],
    &["assistant", "tokens", "cache_read"],
];

const CACHE_WRITE_PATHS: &[&[&str]] = &[
    &["tokens", "cache", "write"],
    &["tokens", "cache", "writeTokens"],
    &["tokens", "cache", "write_tokens"],
    &["tokens", "cacheWrite"],
    &["tokens", "cacheWriteTokens"],
    &["tokens", "cache_write"],
    &["tokens", "cache_write_tokens"],
    &["cache", "write"],
    &["cache", "writeTokens"],
    &["cache", "write_tokens"],
    &["cacheWrite"],
    &["cacheWriteTokens"],
    &["cache_write"],
    &["cache_write_tokens"],
    &["usage", "cache", "write"],
    &["usage", "cache", "writeTokens"],
    &["usage", "cache", "write_tokens"],
    &["usage", "cacheWrite"],
    &["usage", "cacheWriteTokens"],
    &["usage", "cache_write"],
    &["usage", "cache_write_tokens"],
    &["metadata", "assistant", "tokens", "cache", "write"],
    &["metadata", "assistant", "tokens", "cache", "writeTokens"],
    &["metadata", "assistant", "tokens", "cache", "write_tokens"],
    &["metadata", "assistant", "tokens", "cacheWrite"],
    &["metadata", "assistant", "tokens", "cacheWriteTokens"],
    &["metadata", "assistant", "tokens", "cache_write"],
    &["metadata", "assistant", "tokens", "cache_write_tokens"],
    &["metadata", "assistant", "cache", "write"],
    &["metadata", "assistant", "cacheWrite"],
    &["metadata", "assistant", "cache_write"],
    &["assistant", "tokens", "cache", "write"],
    &["assistant", "tokens", "cacheWrite"],
    &["assistant", "tokens", "cache_write"],
];

const COST_PATHS: &[&[&str]] = &[
    &["cost"],
    &["costUsd"],
    &["costUSD"],
    &["cost_usd"],
    &["usage", "cost"],
    &["usage", "costUsd"],
    &["usage", "costUSD"],
    &["usage", "cost_usd"],
    &["metadata", "assistant", "cost"],
    &["metadata", "assistant", "costUsd"],
    &["metadata", "assistant", "costUSD"],
    &["metadata", "assistant", "cost_usd"],
    &["assistant", "cost"],
    &["assistant", "costUsd"],
    &["assistant", "costUSD"],
    &["assistant", "cost_usd"],
];

fn find_string(value: &Value, paths: &[&[&str]]) -> Option<String> {
    paths
        .iter()
        .filter_map(|path| value_at(value, path))
        .find_map(string_value)
}

fn find_u64(value: &Value, paths: &[&[&str]]) -> u64 {
    paths
        .iter()
        .filter_map(|path| value_at(value, path))
        .find_map(safe_u64)
        .unwrap_or(0)
}

fn find_f64(value: &Value, paths: &[&[&str]]) -> f64 {
    paths
        .iter()
        .filter_map(|path| value_at(value, path))
        .find_map(safe_f64)
        .unwrap_or(0.0)
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn string_value(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_owned());
        }
        return None;
    }
    value.as_object().and_then(|object| {
        ["id", "name", "value"]
            .iter()
            .find_map(|key| object.get(*key).and_then(string_value))
    })
}

fn safe_u64(value: &Value) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return Some(value);
    }
    if let Some(value) = value.as_i64() {
        return u64::try_from(value).ok();
    }
    if let Some(value) = value.as_str() {
        if let Ok(value) = value.trim().parse::<u64>() {
            return Some(value);
        }
    }
    let value = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))?;
    if value.is_finite()
        && value >= 0.0
        && value.fract() == 0.0
        && value < 18_446_744_073_709_551_616.0
    {
        return Some(value as u64);
    }
    None
}

fn safe_f64(value: &Value) -> Option<f64> {
    let value = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))?;
    if value.is_finite() && value >= 0.0 {
        Some(value)
    } else {
        None
    }
}

fn safe_cost_sum(left: f64, right: f64) -> f64 {
    if left >= f64::MAX - right {
        f64::MAX
    } else {
        left + right
    }
}

fn text_value(value: ValueRef<'_>) -> Option<String> {
    match value {
        ValueRef::Text(value) => std::str::from_utf8(value).ok().map(ToOwned::to_owned),
        ValueRef::Blob(value) => std::str::from_utf8(value).ok().map(ToOwned::to_owned),
        _ => None,
    }
}

fn timestamp_millis(value: ValueRef<'_>) -> Option<i64> {
    match value {
        ValueRef::Integer(value) => Some(value),
        ValueRef::Real(value) if value.is_finite() && value.fract() == 0.0 => {
            if value >= i64::MIN as f64 && value < 9_223_372_036_854_775_808.0 {
                Some(value as i64)
            } else {
                None
            }
        }
        ValueRef::Text(value) => std::str::from_utf8(value).ok()?.parse().ok(),
        _ => None,
    }
}
