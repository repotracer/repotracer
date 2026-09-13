//! Interactive benchmark entry point.
//!
//! The benchmark runner is deliberately kept in Python.  This module owns the
//! terminal presentation and talks to it through small JSON requests.  Calls
//! are made on worker threads so a slow provider, grader, or filesystem never
//! stops the TUI from repainting or accepting navigation.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap},
    Frame, Terminal,
};
use serde_json::{json, Map, Value};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const MIN_WIDTH: u16 = 48;
const MIN_HEIGHT: u16 = 12;

struct Theme {
    unicode: bool,
    color: bool,
}

impl Theme {
    fn plain(&self) -> Style {
        Style::default()
    }
    fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }
    fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    fn accent(&self) -> Style {
        if self.color {
            Style::default().fg(Color::Cyan)
        } else {
            self.bold()
        }
    }
    fn warn(&self) -> Style {
        if self.color {
            Style::default().fg(Color::Yellow)
        } else {
            self.bold()
        }
    }
    fn highlight(&self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD);
        if self.color {
            style.fg(Color::Black).bg(Color::Cyan)
        } else {
            style.add_modifier(Modifier::REVERSED)
        }
    }
    fn pointer(&self) -> &'static str {
        if self.unicode {
            "❯"
        } else {
            ">"
        }
    }
}

#[derive(Clone, Debug)]
pub struct BenchmarkOptions {
    pub state_dir: Option<PathBuf>,
    pub engine: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Config,
    Task,
    Models,
    Run,
    Results,
    Investigations,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditField {
    Project,
    Prompt,
    DatasetPath,
    InstanceId,
    Parent,
    Model,
    Effort,
    ScoutModel,
    ScoutEffort,
    Seed,
    Preset,
    RepoTracerBinary,
    RepoTracerSource,
    ChangedBinary,
    ChangedSource,
    Acceptance,
    RateCards,
}

impl EditField {
    fn multiline(self) -> bool {
        matches!(self, Self::Prompt | Self::RateCards)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Project => "project path",
            Self::Prompt => "prompt",
            Self::DatasetPath => "dataset path",
            Self::InstanceId => "dataset instance",
            Self::Parent => "parent",
            Self::Model => "parent model",
            Self::Effort => "parent effort",
            Self::ScoutModel => "scout model",
            Self::ScoutEffort => "scout effort",
            Self::Seed => "seed",
            Self::Preset => "daily manual preset arms",
            Self::RepoTracerBinary => "RepoTracer binary",
            Self::RepoTracerSource => "source path",
            Self::ChangedBinary => "changed-arm binary",
            Self::ChangedSource => "changed-arm source path",
            Self::Acceptance => "acceptance command",
            Self::RateCards => "rate cards JSON",
        }
    }
}

#[derive(Clone, Debug)]
struct TextEditor {
    lines: Vec<String>,
    line: usize,
    column: usize,
}

impl TextEditor {
    fn new(value: &str) -> Self {
        let mut lines = value.split('\n').map(ToOwned::to_owned).collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let line = lines.len() - 1;
        let column = lines[line].chars().count();
        Self {
            lines,
            line,
            column,
        }
    }

    fn value(&self) -> String {
        self.lines.join("\n")
    }

    fn insert(&mut self, ch: char) {
        let chars = self.lines[self.line].chars().collect::<Vec<_>>();
        let at = self.column.min(chars.len());
        let mut out = String::with_capacity(self.lines[self.line].len() + ch.len_utf8());
        for (index, value) in chars.into_iter().enumerate() {
            if index == at {
                out.push(ch);
            }
            out.push(value);
        }
        if at == out.chars().count() {
            out.push(ch);
        }
        self.lines[self.line] = out;
        self.column += 1;
    }

    fn newline(&mut self) {
        let chars = self.lines[self.line].chars().collect::<Vec<_>>();
        let at = self.column.min(chars.len());
        let left = chars[..at].iter().collect::<String>();
        let right = chars[at..].iter().collect::<String>();
        self.lines[self.line] = left;
        self.lines.insert(self.line + 1, right);
        self.line += 1;
        self.column = 0;
    }

    fn backspace(&mut self) {
        if self.column > 0 {
            let mut chars = self.lines[self.line].chars().collect::<Vec<_>>();
            let at = self.column.min(chars.len());
            chars.remove(at - 1);
            self.lines[self.line] = chars.into_iter().collect();
            self.column -= 1;
        } else if self.line > 0 {
            let current = self.lines.remove(self.line);
            self.line -= 1;
            self.column = self.lines[self.line].chars().count();
            self.lines[self.line].push_str(&current);
        }
    }

    fn delete(&mut self) {
        let mut chars = self.lines[self.line].chars().collect::<Vec<_>>();
        if self.column < chars.len() {
            chars.remove(self.column);
            self.lines[self.line] = chars.into_iter().collect();
        } else if self.line + 1 < self.lines.len() {
            let next = self.lines.remove(self.line + 1);
            self.lines[self.line].push_str(&next);
        }
    }

    fn left(&mut self) {
        if self.column > 0 {
            self.column -= 1;
        } else if self.line > 0 {
            self.line -= 1;
            self.column = self.lines[self.line].chars().count();
        }
    }

    fn right(&mut self) {
        let width = self.lines[self.line].chars().count();
        if self.column < width {
            self.column += 1;
        } else if self.line + 1 < self.lines.len() {
            self.line += 1;
            self.column = 0;
        }
    }

    fn up(&mut self) {
        if self.line > 0 {
            self.line -= 1;
            self.column = self.column.min(self.lines[self.line].chars().count());
        }
    }

    fn down(&mut self) {
        if self.line + 1 < self.lines.len() {
            self.line += 1;
            self.column = self.column.min(self.lines[self.line].chars().count());
        }
    }
}

#[derive(Clone, Debug)]
struct BackendClient {
    engine: PathBuf,
    state_dir: PathBuf,
}

#[derive(Debug)]
enum Request {
    Init,
    Get,
    Save(Value),
    Start,
    Investigate {
        run: String,
        task: String,
        group: String,
    },
    Apply {
        run: String,
        task: String,
        group: String,
    },
}

#[derive(Debug)]
enum BackendMessage {
    Snapshot(Result<Value, String>),
    Action(Result<Value, String>),
}

impl BackendClient {
    fn call(&self, request: Request) -> Result<Value, String> {
        if !self.engine.is_file() {
            return Err(format!(
                "Benchmark workflow backend not found at {}. Reinstall RepoTracer or pass --engine PATH to a compatible workflow.py.",
                self.engine.display()
            ));
        }
        let (command, args, input) = match request {
            Request::Init => ("init", Vec::new(), None),
            Request::Get => ("get", Vec::new(), None),
            Request::Save(value) => ("save", Vec::new(), Some(value.to_string())),
            Request::Start => ("start", Vec::new(), None),
            Request::Investigate { run, task, group } => (
                "investigate",
                vec![
                    "--run".into(),
                    run,
                    "--task".into(),
                    task,
                    "--group".into(),
                    group,
                ],
                None,
            ),
            Request::Apply { run, task, group } => (
                "apply",
                vec![
                    "--run".into(),
                    run,
                    "--task".into(),
                    task,
                    "--group".into(),
                    group,
                ],
                None,
            ),
        };
        let mut child = None;
        let mut launch_error = None;
        for (python, python_args) in python_launchers() {
            let mut process = Command::new(python);
            process
                .args(*python_args)
                .arg(&self.engine)
                .arg("--state-dir")
                .arg(&self.state_dir)
                .arg(command)
                .args(&args)
                .stdin(if input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Ok(binary) = std::env::current_exe() {
                process.env("REPOTRACER_BENCH_BINARY", binary);
            }
            match process.spawn() {
                Ok(process) => {
                    child = Some(process);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    launch_error = Some(error);
                }
                Err(error) => {
                    return Err(format!("Could not start benchmark backend: {error}"));
                }
            }
        }
        let Some(mut child) = child else {
            let detail = launch_error
                .map(|error| format!(" ({error})"))
                .unwrap_or_default();
            return Err(format!(
                "Python 3.10+ is required for benchmarks. Install it and make `python3` or `python` available on PATH{detail}."
            ));
        };
        if let Some(input) = input {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                stdin.write_all(input.as_bytes()).map_err(|error| {
                    format!("Could not send config to benchmark backend: {error}")
                })?;
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("Benchmark backend did not finish: {error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let detail = serde_json::from_str::<Value>(&detail)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .filter(|value| !value.is_empty())
                .unwrap_or(detail);
            return Err(if detail.is_empty() {
                format!("Benchmark backend exited with {}", output.status)
            } else {
                detail
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(stdout.trim())
            .map_err(|error| format!("Benchmark backend returned invalid JSON: {error}"))
    }
}

#[cfg(windows)]
fn python_launchers() -> &'static [(&'static str, &'static [&'static str])] {
    &[("py", &["-3"]), ("python3", &[]), ("python", &[])]
}

#[cfg(not(windows))]
fn python_launchers() -> &'static [(&'static str, &'static [&'static str])] {
    &[("python3", &[]), ("python", &[])]
}

fn default_state_dir() -> PathBuf {
    dirs::data_local_dir()
        .map(|path| path.join("repotracer").join("benchmarks"))
        .unwrap_or_else(|| PathBuf::from(".benchmark-runs"))
}

fn spawn_request(client: BackendClient, request: Request, sender: mpsc::Sender<BackendMessage>) {
    thread::spawn(move || {
        let is_snapshot = matches!(request, Request::Init | Request::Get);
        let result = client.call(request);
        let message = if is_snapshot {
            BackendMessage::Snapshot(result)
        } else {
            BackendMessage::Action(result)
        };
        let _ = sender.send(message);
    });
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn text_or(value: &Value, first: &str, second: &str) -> String {
    let primary = text(value, first);
    if primary.is_empty() {
        text(value, second)
    } else {
        primary
    }
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn bool_value(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn array<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|values| values.iter().collect())
        .unwrap_or_default()
}

fn config_array_mut<'a>(config: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    let map = config.as_object_mut().expect("config must be an object");
    let value = map
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    value
        .as_array_mut()
        .expect("config array field must be an array")
}

fn nested_text(config: &Value, parent: &str, key: &str, fallback: &str) -> String {
    config
        .get(parent)
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn set_nested_text(config: &mut Value, parent: &str, key: &str, value: String) {
    let root = config.as_object_mut().expect("config must be an object");
    let child = root
        .entry(parent.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    child
        .as_object_mut()
        .expect("config nested field must be an object")
        .insert(key.to_owned(), Value::String(value));
}

fn arm_text(config: &Value, arm_name: &str, key: &str) -> String {
    array(config, "arms")
        .into_iter()
        .find(|arm| text(arm, "name") == arm_name)
        .map(|arm| text(arm, key))
        .unwrap_or_default()
}

fn set_arm_text(config: &mut Value, arm_name: &str, key: &str, value: String) {
    if let Some(arm) = config_array_mut(config, "arms")
        .iter_mut()
        .find(|arm| text(arm, "name") == arm_name)
    {
        arm[key] = Value::String(value);
    }
}

fn parse_rate_cards(value: &str) -> Result<Value, String> {
    let parsed: Value = serde_json::from_str(value)
        .map_err(|error| format!("Rate cards must be valid JSON: {error}"))?;
    let cards = parsed
        .as_object()
        .ok_or_else(|| "Rate cards must be a JSON object.".to_owned())?;
    for (name, card) in cards {
        if name.trim().is_empty() {
            return Err("Rate card names cannot be empty.".into());
        }
        let card = card
            .as_object()
            .ok_or_else(|| format!("Rate card {name:?} must be an object."))?;
        for field in ["model", "source"] {
            if !card
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Err(format!("Rate card {name:?} needs a non-empty {field}."));
            }
        }
        for field in ["uncached_input", "cache_read", "cache_write", "output"] {
            if !card
                .get(field)
                .and_then(Value::as_f64)
                .is_some_and(|value| value.is_finite() && value >= 0.0)
            {
                return Err(format!(
                    "Rate card {name:?} needs a non-negative numeric {field} rate."
                ));
            }
        }
    }
    Ok(parsed)
}

struct App {
    page: Page,
    config: Value,
    runs: Vec<Value>,
    investigations: Vec<Value>,
    results: Vec<Value>,
    selected: usize,
    task: usize,
    arm: usize,
    edit: Option<EditField>,
    editor: Option<TextEditor>,
    selected_run: usize,
    selected_investigation: usize,
    selected_result: usize,
    confirm_apply: Option<ApplyTarget>,
    pending_start: bool,
    message: String,
    loading: bool,
    config_loaded: bool,
    receiver: mpsc::Receiver<BackendMessage>,
    busy: bool,
    sender: mpsc::Sender<BackendMessage>,
    client: BackendClient,
    last_poll: Instant,
    unicode: bool,
    color: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ApplyTarget {
    run: String,
    task: String,
    group: String,
}

impl App {
    fn new(
        client: BackendClient,
        sender: mpsc::Sender<BackendMessage>,
        receiver: mpsc::Receiver<BackendMessage>,
    ) -> Self {
        Self {
            page: Page::Config,
            config: default_config(),
            runs: Vec::new(),
            investigations: Vec::new(),
            results: Vec::new(),
            selected: 0,
            task: 0,
            arm: 0,
            edit: None,
            editor: None,
            selected_run: 0,
            selected_investigation: 0,
            selected_result: 0,
            confirm_apply: None,
            pending_start: false,
            message: String::new(),
            loading: true,
            config_loaded: false,
            receiver,
            busy: false,
            sender,
            client,
            last_poll: Instant::now(),
            unicode: unicode_enabled(),
            color: std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn apply_snapshot(&mut self, value: Value) {
        self.config = value.get("config").cloned().unwrap_or_else(default_config);
        if self.config.get("presets").is_none() {
            if let Some(config) = self.config.as_object_mut() {
                config.insert(
                    "presets".into(),
                    json!([{"name":"daily","schedule":"manual","arms":["baseline","current"]}]),
                );
            }
        }
        self.runs = array(&value, "runs").into_iter().cloned().collect();
        self.investigations = array(&value, "investigations")
            .into_iter()
            .cloned()
            .collect();
        self.results = array(&value, "results").into_iter().cloned().collect();
        if self.investigations.is_empty() {
            self.investigations = self
                .result_rows()
                .into_iter()
                .filter(is_loss)
                .map(|result| {
                    json!({
                        "run_id": text(&result, "run_id"),
                        "task_id": text_or(&result, "task_id", "task"),
                        "group": text(&result, "group"),
                        "state": "queued",
                        "diagnosis": "diagnosis required"
                    })
                })
                .collect();
        }
        self.loading = false;
        self.config_loaded = true;
        self.selected = self.selected.min(self.task_count().saturating_sub(1));
        self.selected_run = self.selected_run.min(self.runs.len().saturating_sub(1));
        self.selected_investigation = self
            .selected_investigation
            .min(self.investigations.len().saturating_sub(1));
        self.selected_result = self
            .selected_result
            .min(self.result_rows().len().saturating_sub(1));
    }

    fn poll_backend(&mut self) {
        let mut messages = Vec::new();
        while let Ok(message) = self.receiver.try_recv() {
            messages.push(message);
        }
        if messages.is_empty() {
            return;
        }
        for message in messages {
            self.busy = false;
            match message {
                BackendMessage::Snapshot(result) => match result {
                    Ok(value) => self.apply_snapshot(value),
                    Err(error) => {
                        self.pending_start = false;
                        self.loading = false;
                        self.message = error;
                    }
                },
                BackendMessage::Action(result) => match result {
                    Ok(value) => {
                        self.loading = false;
                        if let Some(config) = value.get("config") {
                            self.config = config.clone();
                        }
                        self.message = text(&value, "message");
                        if self.pending_start {
                            self.pending_start = false;
                            self.submit_request(Request::Start);
                        } else {
                            // Start/investigate/apply return an id/state object.
                            // A fresh get immediately makes that job visible.
                            self.submit_request(Request::Get);
                        }
                    }
                    Err(error) => {
                        self.pending_start = false;
                        self.loading = false;
                        self.message = error;
                    }
                },
            }
        }
    }

    fn task_count(&self) -> usize {
        array(&self.config, "tasks").len()
    }

    fn tasks(&self) -> Vec<&Value> {
        array(&self.config, "tasks")
    }

    fn selected_task(&self) -> Option<&Value> {
        self.tasks().get(self.task).copied()
    }

    fn selected_task_mut(&mut self) -> Option<&mut Value> {
        config_array_mut(&mut self.config, "tasks").get_mut(self.task)
    }

    fn selected_run(&self) -> Option<&Value> {
        self.runs.get(self.selected_run)
    }

    fn selected_investigation(&self) -> Option<&Value> {
        self.investigations.get(self.selected_investigation)
    }

    /// Flatten the runner's report summaries.  `results/*.json` intentionally
    /// stores `pairs`; the TUI turns those into rows while retaining arm usage
    /// from the saved run when it is available.
    fn result_rows(&self) -> Vec<Value> {
        let mut rows = Vec::new();
        for result in &self.results {
            let pairs = result.get("pairs").and_then(Value::as_array);
            if let Some(pairs) = pairs {
                for pair in pairs {
                    let mut row = pair.clone();
                    let run_id = {
                        let value = text(&row, "run_id");
                        if value.is_empty() {
                            text(result, "run_id")
                        } else {
                            value
                        }
                    };
                    let task_id = text_or(&row, "task_id", "task");
                    let group = text(&row, "group");
                    if let Some(map) = row.as_object_mut() {
                        map.entry("run_id")
                            .or_insert_with(|| Value::String(run_id.clone()));
                        if let Some(run) = self.runs.iter().find(|run| text(run, "id") == run_id) {
                            if let Some(task) =
                                run.get("tasks")
                                    .and_then(Value::as_array)
                                    .and_then(|tasks| {
                                        tasks.iter().find(|task| text(task, "id") == task_id)
                                    })
                            {
                                if let Some(arm) =
                                    task.get("arms").and_then(Value::as_array).and_then(|arms| {
                                        arms.iter().find(|arm| text(arm, "group") == group)
                                    })
                                {
                                    for key in [
                                        "cost_usd",
                                        "seconds",
                                        "tokens",
                                        "usage_complete",
                                        "usage_missing_reason",
                                        "state",
                                    ] {
                                        if !map.contains_key(key) {
                                            if let Some(value) = arm.get(key) {
                                                map.insert(key.into(), value.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    rows.push(row);
                }
            } else {
                rows.push(result.clone());
            }
        }
        rows
    }

    fn is_active_run(&self) -> bool {
        self.selected_run().is_some_and(|run| {
            matches!(
                text(run, "state").as_str(),
                "queued" | "running" | "grading"
            )
        })
    }

    fn poll_active(&mut self) {
        if self.is_active_run()
            && self.last_poll.elapsed() >= Duration::from_millis(700)
            && !self.busy
        {
            self.last_poll = Instant::now();
            // The event-loop receiver is supplied by `run_loop`; this request
            // is sent through the shared channel and the next get is handled by
            // the same path as every other operation.
            spawn_request(self.client.clone(), Request::Get, self.sender.clone());
            self.loading = true;
            self.busy = true;
        }
    }

    fn start_edit(&mut self, field: EditField) {
        let value = match field {
            EditField::Project => self
                .selected_task()
                .map(|task| text(task, "project_path"))
                .unwrap_or_default(),
            EditField::Prompt => self
                .selected_task()
                .map(|task| text(task, "prompt"))
                .unwrap_or_default(),
            EditField::DatasetPath => self
                .selected_task()
                .map(|task| text(task, "dataset_path"))
                .unwrap_or_default(),
            EditField::InstanceId => self
                .selected_task()
                .map(|task| text(task, "instance_id"))
                .unwrap_or_default(),
            EditField::Parent => self
                .selected_task()
                .map(|task| text(task, "parent"))
                .unwrap_or_else(|| "codex".into()),
            EditField::Model => self.model_field("parent_model"),
            EditField::Effort => self.model_field("parent_effort"),
            EditField::ScoutModel => self.model_field("scout_model"),
            EditField::ScoutEffort => self.model_field("scout_effort"),
            EditField::Seed => self
                .config
                .get("seed")
                .map(|seed| match seed {
                    Value::String(value) => value.clone(),
                    Value::Number(value) => value.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default(),
            EditField::Preset => self
                .config
                .get("presets")
                .and_then(Value::as_array)
                .and_then(|presets| presets.first())
                .and_then(|preset| preset.get("arms"))
                .and_then(Value::as_array)
                .map(|arms| {
                    arms.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_else(|| "baseline,current".into()),
            EditField::RepoTracerBinary => nested_text(&self.config, "repotracer", "binary", ""),
            EditField::RepoTracerSource => {
                nested_text(&self.config, "repotracer", "source_path", "")
            }
            EditField::ChangedBinary => arm_text(&self.config, "changed", "binary"),
            EditField::ChangedSource => arm_text(&self.config, "changed", "source_path"),
            EditField::Acceptance => nested_text(&self.config, "acceptance", "command", ""),
            EditField::RateCards => serde_json::to_string_pretty(
                self.config
                    .get("rate_cards")
                    .unwrap_or(&Value::Object(Map::new())),
            )
            .unwrap_or_else(|_| "{}".into()),
        };
        self.edit = Some(field);
        self.editor = Some(TextEditor::new(&value));
    }

    fn model_field(&self, key: &str) -> String {
        let parent = if self.arm == 1 { "claude" } else { "codex" };
        nested_text(&self.config, "models", &format!("{parent}_{key}"), "")
    }

    fn finish_edit(&mut self) {
        let Some(field) = self.edit.take() else {
            return;
        };
        let Some(editor) = self.editor.take() else {
            return;
        };
        let value = editor.value();
        if matches!(
            field,
            EditField::Project
                | EditField::Prompt
                | EditField::DatasetPath
                | EditField::InstanceId
                | EditField::Parent
        ) {
            if let Some(task) = self.selected_task_mut() {
                let key = match field {
                    EditField::Project => "project_path",
                    EditField::Prompt => "prompt",
                    EditField::DatasetPath => "dataset_path",
                    EditField::InstanceId => "instance_id",
                    EditField::Parent => "parent",
                    _ => unreachable!(),
                };
                if key == "prompt" && text(task, "origin") == "external" {
                    self.message = "External SWE-bench prompts come from the dataset; choose a dataset instead.".into();
                    return;
                }
                task[key] = Value::String(value);
            }
            return;
        }
        let parent = if self.arm == 1 { "claude" } else { "codex" };
        match field {
            EditField::Model => set_nested_text(
                &mut self.config,
                "models",
                &format!("{parent}_parent_model"),
                value,
            ),
            EditField::Effort => set_nested_text(
                &mut self.config,
                "models",
                &format!("{parent}_parent_effort"),
                value,
            ),
            EditField::ScoutModel => set_nested_text(
                &mut self.config,
                "models",
                &format!("{parent}_scout_model"),
                value,
            ),
            EditField::ScoutEffort => set_nested_text(
                &mut self.config,
                "models",
                &format!("{parent}_scout_effort"),
                value,
            ),
            EditField::Seed => {
                if let Some(root) = self.config.as_object_mut() {
                    root.insert("seed".into(), Value::String(value));
                }
            }
            EditField::Preset => {
                let arms = value
                    .split([',', ' ', '\n'])
                    .map(str::trim)
                    .filter(|arm| !arm.is_empty())
                    .map(|arm| Value::String(arm.to_owned()))
                    .collect::<Vec<_>>();
                let presets = config_array_mut(&mut self.config, "presets");
                if presets.is_empty() {
                    presets.push(json!({"name":"daily","schedule":"manual","arms":arms}));
                } else if let Some(preset) = presets[0].as_object_mut() {
                    preset.insert("name".into(), Value::String("daily".into()));
                    preset.insert("schedule".into(), Value::String("manual".into()));
                    preset.insert("arms".into(), Value::Array(arms));
                }
            }
            EditField::RepoTracerBinary => {
                set_nested_text(&mut self.config, "repotracer", "binary", value)
            }
            EditField::RepoTracerSource => {
                set_nested_text(&mut self.config, "repotracer", "source_path", value)
            }
            EditField::ChangedBinary => set_arm_text(&mut self.config, "changed", "binary", value),
            EditField::ChangedSource => {
                set_arm_text(&mut self.config, "changed", "source_path", value)
            }
            EditField::Acceptance => {
                set_nested_text(&mut self.config, "acceptance", "command", value)
            }
            EditField::RateCards => match parse_rate_cards(&value) {
                Ok(cards) => {
                    if let Some(root) = self.config.as_object_mut() {
                        root.insert("rate_cards".into(), cards);
                    }
                }
                Err(error) => self.message = error,
            },
            _ => {}
        }
    }

    fn add_task(&mut self, external: bool) {
        let id = format!(
            "{}-{}",
            if external { "external" } else { "custom" },
            self.task_count() + 1
        );
        let task = if external {
            json!({"id": id, "origin":"external", "source":"swebench-lite", "dataset_path":"", "instance_id":"", "project_path":"", "prompt":"", "apply_allowed":false, "parent":"claude"})
        } else {
            json!({"id": id, "origin":"custom", "project_path":".", "prompt":"Describe the change and how it should be verified.", "apply_allowed":false, "parent":"codex"})
        };
        config_array_mut(&mut self.config, "tasks").push(task);
        self.task = self.task_count().saturating_sub(1);
        self.selected = self.task;
        self.message = if external {
            "Added external SWE-bench Lite task."
        } else {
            "Added custom Codex task."
        }
        .into();
    }

    fn remove_task(&mut self) {
        if self.task_count() <= 1 {
            self.message = "Keep at least one benchmark task.".into();
            return;
        }
        config_array_mut(&mut self.config, "tasks").remove(self.task);
        self.task = self.task.min(self.task_count().saturating_sub(1));
        self.selected = self.task;
    }

    fn toggle_changed_arm(&mut self) {
        let arms = self.config.get_mut("arms").and_then(Value::as_array_mut);
        if let Some(arms) = arms {
            if let Some(changed) = arms.iter_mut().find(|arm| text(arm, "name") == "changed") {
                let enabled = bool_value(changed, "enabled");
                changed["enabled"] = Value::Bool(!enabled);
                self.message = if enabled {
                    "Changed arm disabled; baseline/current remain the default comparison.".into()
                } else {
                    "Changed arm enabled for this manual run.".into()
                };
            }
        }
    }

    fn save(&mut self) {
        if !self.config_loaded {
            self.message =
                "Wait for the saved benchmark configuration to load before saving.".into();
            return;
        }
        self.submit_request(Request::Save(self.config.clone()));
    }

    fn submit_request(&mut self, request: Request) {
        if self.busy {
            self.message =
                "Benchmark backend is still working; navigation remains available.".into();
            return;
        }
        self.loading = true;
        self.busy = true;
        spawn_request(self.client.clone(), request, self.sender.clone());
    }

    fn handle(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return false;
        }
        if self.edit.is_some() {
            return self.handle_editor(key);
        }
        if self.confirm_apply.is_some() {
            match key.code {
                KeyCode::Enter => self.apply_confirmed(),
                KeyCode::Esc => self.confirm_apply = None,
                _ => {}
            }
            return true;
        }
        self.message.clear();
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return false,
            KeyCode::Char('1') => self.show_page(Page::Config),
            KeyCode::Char('2') => self.show_page(Page::Models),
            KeyCode::Char('3') => self.show_page(Page::Run),
            KeyCode::Char('4') => self.show_page(Page::Results),
            KeyCode::Char('5') => self.show_page(Page::Investigations),
            KeyCode::Char('6') => self.show_page(Page::History),
            KeyCode::Esc if self.page != Page::Config => {
                self.page = Page::Config;
                self.selected = self.task;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(true),
            KeyCode::Tab => self.next_page(),
            KeyCode::Char('a') if self.page == Page::Config => self.add_task(false),
            KeyCode::Char('x') if self.page == Page::Config => self.add_task(true),
            KeyCode::Char('d') if self.page == Page::Config => self.remove_task(),
            KeyCode::Char('c') | KeyCode::Char('C') if self.page == Page::Config => {
                self.toggle_changed_arm()
            }
            KeyCode::Enter if self.page == Page::Config => {
                self.task = self.selected;
                self.selected = 0;
                self.page = Page::Task;
            }
            KeyCode::Char('e')
                if self.page == Page::Config
                    && self
                        .selected_task()
                        .is_some_and(|task| text(task, "origin") != "external") =>
            {
                self.task = self.selected;
                self.selected = 1;
                self.page = Page::Task;
                self.start_edit(EditField::Prompt);
            }
            KeyCode::Char('m') if self.page == Page::Task => self.start_edit(EditField::Parent),
            KeyCode::Char('p') if self.page == Page::Task => self.start_edit(EditField::Project),
            KeyCode::Char(' ') if self.page == Page::Task => {
                let external = self
                    .selected_task()
                    .is_some_and(|task| text(task, "origin") == "external");
                let parent_index = if external { 3 } else { 2 };
                if self.selected == parent_index {
                    if let Some(task) = self.selected_task_mut() {
                        let parent = text(task, "parent");
                        task["parent"] = Value::String(if parent == "claude" {
                            "codex".into()
                        } else {
                            "claude".into()
                        });
                    }
                } else if !external && self.selected == 3 {
                    if let Some(task) = self.selected_task_mut() {
                        let allowed = task
                            .get("apply_allowed")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        task["apply_allowed"] = Value::Bool(!allowed);
                    }
                }
            }
            KeyCode::Char('r') if self.page == Page::Config || self.page == Page::Models => {
                self.start_run()
            }
            KeyCode::Char('s') if self.page == Page::Config || self.page == Page::Models => {
                self.save()
            }
            KeyCode::Enter if self.page == Page::Task => self.edit_task_enter(),
            KeyCode::Enter if self.page == Page::Models => self.edit_models_enter(),
            KeyCode::Enter if self.page == Page::History => self.page = Page::Run,
            KeyCode::Char('i') | KeyCode::Enter if self.page == Page::Investigations => {
                self.investigate()
            }
            KeyCode::Char('a') if matches!(self.page, Page::Results | Page::Investigations) => {
                self.begin_apply()
            }
            KeyCode::Char('<') if self.page == Page::Models => self.arm = 0,
            KeyCode::Char('>') if self.page == Page::Models => self.arm = 1,
            _ => {}
        }
        true
    }

    fn handle_editor(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.finish_edit();
            return true;
        }
        let editor = self.editor.as_mut().expect("edit has editor");
        match key.code {
            KeyCode::Esc => self.finish_edit(),
            KeyCode::Enter if self.edit.is_some_and(EditField::multiline) => editor.newline(),
            KeyCode::Enter => self.finish_edit(),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Left => editor.left(),
            KeyCode::Right => editor.right(),
            KeyCode::Up => editor.up(),
            KeyCode::Down => editor.down(),
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::ALT) => editor.insert(ch),
            _ => {}
        }
        true
    }

    fn move_selection(&mut self, down: bool) {
        let count = match self.page {
            Page::Config => self.task_count(),
            Page::Task => 4,
            Page::Models => 12,
            Page::Run => self.runs.len(),
            Page::Investigations => self.investigations.len(),
            Page::Results => self.result_rows().len(),
            Page::History => self.runs.len(),
        };
        if count == 0 {
            self.selected = 0;
            return;
        }
        self.selected = if down {
            (self.selected + 1) % count
        } else {
            (self.selected + count - 1) % count
        };
        match self.page {
            Page::Config => self.task = self.selected,
            Page::Run | Page::History => self.selected_run = self.selected,
            Page::Investigations => self.selected_investigation = self.selected,
            Page::Results => self.selected_result = self.selected,
            _ => {}
        }
    }

    fn show_page(&mut self, page: Page) {
        self.page = page;
        self.selected = match page {
            Page::Config | Page::Task => self.task,
            Page::Run | Page::History => self.selected_run,
            Page::Results => self.selected_result,
            Page::Investigations => self.selected_investigation,
            Page::Models => 0,
        };
    }

    fn next_page(&mut self) {
        self.page = match self.page {
            Page::Config => Page::Models,
            Page::Models => Page::Run,
            Page::Run => Page::Results,
            Page::Results => Page::Investigations,
            Page::Investigations => Page::History,
            Page::History => Page::Config,
            Page::Task => Page::Config,
        };
        self.selected = 0;
        match self.page {
            Page::Config => self.task = 0,
            Page::Run | Page::History => self.selected_run = 0,
            Page::Results => self.selected_result = 0,
            Page::Investigations => self.selected_investigation = 0,
            _ => {}
        }
    }

    fn edit_task_enter(&mut self) {
        let external = self
            .selected_task()
            .is_some_and(|task| text(task, "origin") == "external");
        let field = match self.selected {
            0 => EditField::Project,
            1 if external => EditField::DatasetPath,
            2 if external => EditField::InstanceId,
            1 => EditField::Prompt,
            _ => EditField::Parent,
        };
        if self
            .selected_task()
            .is_some_and(|task| text(task, "origin") == "external")
            && field == EditField::Prompt
        {
            self.message =
                "External prompt is dataset-owned; set dataset/instance fields instead.".into();
        } else {
            self.start_edit(field);
        }
    }

    fn edit_models_enter(&mut self) {
        let field = match self.selected {
            0 => EditField::Model,
            1 => EditField::Effort,
            2 => EditField::ScoutModel,
            3 => EditField::ScoutEffort,
            4 => EditField::Seed,
            5 => EditField::RepoTracerBinary,
            6 => EditField::RepoTracerSource,
            7 => EditField::ChangedBinary,
            8 => EditField::ChangedSource,
            9 => EditField::Acceptance,
            10 => EditField::RateCards,
            _ => EditField::Preset,
        };
        self.start_edit(field);
    }

    fn start_run(&mut self) {
        if !self.config_loaded {
            self.message =
                "Wait for the saved benchmark configuration to load before starting a run.".into();
            return;
        }
        if self.busy {
            self.message =
                "Benchmark backend is still working; wait for it before starting a run.".into();
            return;
        }
        self.page = Page::Run;
        self.message = "Starting a detached benchmark run…".into();
        self.pending_start = true;
        self.submit_request(Request::Save(self.config.clone()));
    }

    fn investigate(&mut self) {
        let Some(row) = self.selected_investigation() else {
            self.message = "No worse comparison is selected.".into();
            return;
        };
        let run = text(row, "run_id");
        let task = text(row, "task_id");
        let group = text(row, "group");
        if run.is_empty() || task.is_empty() || group.is_empty() {
            self.message = "This comparison has no complete run/task/group identity.".into();
            return;
        }
        self.submit_request(Request::Investigate { run, task, group });
    }

    fn selected_result_row(&self) -> Option<Value> {
        self.result_rows().get(self.selected_result).cloned()
    }

    fn saved_task(&self, run_id: &str, task_id: &str) -> Option<&Value> {
        let run = self.runs.iter().find(|run| text(run, "id") == run_id)?;
        run.get("selected_tasks")
            .and_then(Value::as_array)
            .or_else(|| {
                run.get("config")
                    .and_then(|config| config.get("tasks"))
                    .and_then(Value::as_array)
            })
            .or_else(|| run.get("tasks").and_then(Value::as_array))?
            .iter()
            .find(|task| text(task, "id") == task_id)
    }

    fn begin_apply(&mut self) {
        let row = match self.page {
            Page::Results => self.selected_result_row(),
            Page::Investigations => self.selected_investigation().cloned(),
            _ => None,
        };
        let Some(row) = row else {
            self.message = "No candidate is selected.".into();
            return;
        };
        if self.page == Page::Results && !is_win(&row) {
            self.message = "Select a completed winning comparison before applying it.".into();
            return;
        }
        let target = ApplyTarget {
            run: text(&row, "run_id"),
            task: text_or(&row, "task_id", "task"),
            group: text(&row, "group"),
        };
        if target.run.is_empty() || target.task.is_empty() || target.group.is_empty() {
            self.message = "This candidate cannot be applied without its run identity.".into();
            return;
        }
        let Some(saved_task) = self.saved_task(&target.run, &target.task) else {
            self.message =
                "The saved run does not contain permission metadata for this task.".into();
            return;
        };
        if text(saved_task, "origin") != "custom" {
            self.message =
                "Only custom tasks may apply a candidate; external tasks stay immutable.".into();
            return;
        }
        if !bool_value(saved_task, "apply_allowed") {
            self.message =
                "This saved run did not authorize applying the selected custom task.".into();
            return;
        }
        self.confirm_apply = Some(target);
    }

    fn apply_confirmed(&mut self) {
        let Some(target) = self.confirm_apply.take() else {
            return;
        };
        let Some(saved_task) = self.saved_task(&target.run, &target.task) else {
            self.message =
                "The saved run no longer contains permission metadata for this task.".into();
            return;
        };
        if text(saved_task, "origin") != "custom" || !bool_value(saved_task, "apply_allowed") {
            self.message = "The saved run does not authorize applying this candidate.".into();
            return;
        }
        self.submit_request(Request::Apply {
            run: target.run,
            task: target.task,
            group: target.group,
        });
    }

    fn theme(&self) -> Theme {
        Theme {
            unicode: self.unicode,
            color: self.color,
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            frame.render_widget(
                Paragraph::new(format!(
                    "Enlarge this window to {MIN_WIDTH} x {MIN_HEIGHT}.  Ctrl+C quits."
                )),
                area,
            );
            return;
        }
        let theme = self.theme();
        let compact = area.height < 21;
        let card = centered(area, 112, area.height.min(40));
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(card);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(") )", theme.accent()),
                    Span::styled(" RepoTracer benchmarks", theme.bold()),
                ]),
                Line::from(Span::styled(
                    "compare cost, time, tokens, and quality in native runs",
                    theme.dim(),
                )),
            ]),
            rows[0],
        );
        self.draw_tabs(frame, rows[1], &theme);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if theme.unicode {
                BorderType::Rounded
            } else {
                BorderType::Plain
            })
            .border_style(theme.dim())
            .padding(Padding::horizontal(1))
            .title(Span::styled(self.title(), theme.accent()));
        let inner = block.inner(rows[2]);
        frame.render_widget(block, rows[2]);
        match self.page {
            Page::Config => self.draw_config(frame, inner, compact, &theme),
            Page::Task => self.draw_task(frame, inner, &theme),
            Page::Models => self.draw_models(frame, inner, &theme),
            Page::Run => self.draw_run(frame, inner, &theme),
            Page::Results => self.draw_results(frame, inner, &theme),
            Page::Investigations => self.draw_investigations(frame, inner, &theme),
            Page::History => self.draw_history(frame, inner, &theme),
        }
        let footer = if self.confirm_apply.is_some() {
            "Apply this custom-task candidate to the working checkout? Enter confirms · Esc cancels"
        } else if !self.message.is_empty() {
            &self.message
        } else {
            self.footer()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                footer,
                if self.confirm_apply.is_some() {
                    theme.warn()
                } else {
                    theme.dim()
                },
            ))
            .wrap(Wrap { trim: false }),
            rows[3],
        );
    }

    fn title(&self) -> String {
        match self.page {
            Page::Config => " Tasks / configuration ".into(),
            Page::Task => format!(" Edit task {} ", self.task + 1),
            Page::Models => " Models / run preset ".into(),
            Page::Run => " Live run progress ".into(),
            Page::Results => " Results ".into(),
            Page::Investigations => " Investigations ".into(),
            Page::History => " Saved run history ".into(),
        }
    }

    fn draw_tabs(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let tabs = [
            "1 Tasks",
            "2 Models",
            "3 Run",
            "4 Results",
            "5 Investigate",
            "6 History",
        ];
        let mut spans = Vec::new();
        for (index, tab) in tabs.iter().enumerate() {
            let page = match index {
                0 => Page::Config,
                1 => Page::Models,
                2 => Page::Run,
                3 => Page::Results,
                4 => Page::Investigations,
                _ => Page::History,
            };
            spans.push(Span::styled(
                format!(" {tab} "),
                if self.page == page {
                    theme.highlight()
                } else {
                    theme.dim()
                },
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_config(&self, frame: &mut Frame, area: Rect, _compact: bool, theme: &Theme) {
        let tasks = self.tasks();
        let rows = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| {
                let origin = text(task, "origin");
                let path = text(task, "project_path");
                let parent = text(task, "parent");
                let label = if origin == "external" {
                    "external · SWE-bench Lite"
                } else {
                    "custom · Codex"
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(
                            "{} ",
                            if index == self.task {
                                theme.pointer()
                            } else {
                                "  "
                            }
                        ),
                        if index == self.task {
                            theme.accent()
                        } else {
                            theme.dim()
                        },
                    ),
                    Span::styled(
                        format!("{}  ", text(task, "id")),
                        if index == self.task {
                            theme.bold()
                        } else {
                            theme.plain()
                        },
                    ),
                    Span::raw(label),
                    Span::styled(format!("  {parent}  {path}"), theme.dim()),
                ]))
            })
            .collect::<Vec<_>>();
        let mut state =
            ListState::default().with_selected((!tasks.is_empty()).then_some(self.task));
        let list_area = Rect {
            height: area.height.saturating_sub(4),
            ..area
        };
        frame.render_stateful_widget(
            List::new(rows).highlight_style(theme.highlight()),
            list_area,
            &mut state,
        );
        let note = format!(
            "{} tasks  ·  enabled arms: {}  ·  c toggles changed\na add Codex  ·  x add SWE-bench  ·  d remove  ·  Enter edit",
            tasks.len(),
            self.enabled_arms()
        );
        frame.render_widget(
            Paragraph::new(note).wrap(Wrap { trim: false }),
            Rect {
                y: area.y + area.height.saturating_sub(3),
                height: 3,
                ..area
            },
        );
    }

    fn enabled_arms(&self) -> String {
        let names = array(&self.config, "arms")
            .into_iter()
            .filter(|arm| bool_value(arm, "enabled"))
            .map(|arm| text(arm, "name"))
            .collect::<Vec<_>>();
        if names.is_empty() {
            "none".into()
        } else {
            names.join(", ")
        }
    }

    fn draw_task(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if let (Some(field), Some(editor)) = (self.edit, self.editor.as_ref()) {
            self.draw_editor(
                frame,
                area,
                editor,
                format!("Editing {}", field.label()),
                theme,
            );
            return;
        }
        let task = self.selected_task().cloned().unwrap_or_else(|| json!({}));
        let external = text(&task, "origin") == "external";
        let mut lines = vec![
            Line::from(vec![
                Span::styled("id       ", theme.dim()),
                Span::raw(text(&task, "id")),
            ]),
            Line::from(vec![
                Span::styled("origin   ", theme.dim()),
                Span::raw(if external {
                    "external · SWE-bench Lite (dataset prompt)"
                } else {
                    "custom · Codex"
                }),
            ]),
            Line::from(vec![
                Span::styled(
                    format!(
                        "{}project  ",
                        if self.selected == 0 {
                            theme.pointer()
                        } else {
                            "  "
                        }
                    ),
                    if self.selected == 0 {
                        theme.accent()
                    } else {
                        theme.dim()
                    },
                ),
                Span::raw(text(&task, "project_path")),
            ]),
        ];
        if external {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "{}dataset  ",
                        if self.selected == 1 {
                            theme.pointer()
                        } else {
                            "  "
                        }
                    ),
                    if self.selected == 1 {
                        theme.accent()
                    } else {
                        theme.dim()
                    },
                ),
                Span::raw(format!(
                    "{}  {}",
                    text(&task, "source"),
                    text(&task, "dataset_path")
                )),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "{}instance ",
                        if self.selected == 2 {
                            theme.pointer()
                        } else {
                            "  "
                        }
                    ),
                    if self.selected == 2 {
                        theme.accent()
                    } else {
                        theme.dim()
                    },
                ),
                Span::raw(text(&task, "instance_id")),
            ]));
            lines.push(Line::from(Span::styled(
                "Prompt is resolved from the external dataset; it is not editable here.",
                theme.warn(),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "{}prompt   ",
                        if self.selected == 1 {
                            theme.pointer()
                        } else {
                            "  "
                        }
                    ),
                    if self.selected == 1 {
                        theme.accent()
                    } else {
                        theme.dim()
                    },
                ),
                Span::raw(truncate(&text(&task, "prompt"), 90)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "{}apply    ",
                        if self.selected == 3 {
                            theme.pointer()
                        } else {
                            "  "
                        }
                    ),
                    if self.selected == 3 {
                        theme.accent()
                    } else {
                        theme.dim()
                    },
                ),
                Span::raw(if bool_value(&task, "apply_allowed") {
                    "allowed after explicit candidate choice"
                } else {
                    "off (Space toggles)"
                }),
            ]));
        }
        let parent_index = if external { 3 } else { 2 };
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "{}parent   ",
                    if self.selected == parent_index {
                        theme.pointer()
                    } else {
                        "  "
                    }
                ),
                if self.selected == parent_index {
                    theme.accent()
                } else {
                    theme.dim()
                },
            ),
            Span::raw(text(&task, "parent")),
        ]));
        lines.push(Line::from(Span::styled(
            "Enter a field to edit · prompt is dataset-owned · Space toggles apply on custom tasks · Esc back",
            theme.dim(),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }

    fn draw_models(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if let (Some(field), Some(editor)) = (self.edit, self.editor.as_ref()) {
            self.draw_editor(
                frame,
                area,
                editor,
                format!("Editing {}", field.label()),
                theme,
            );
            return;
        }
        let parent = if self.arm == 1 { "claude" } else { "codex" };
        let rows = vec![
            format!(
                "{} parent model   {}",
                parent,
                nested_text(
                    &self.config,
                    "models",
                    &format!("{parent}_parent_model"),
                    if parent == "claude" {
                        "opus"
                    } else {
                        "gpt-5.6-sol"
                    }
                )
            ),
            format!(
                "{} parent effort  {}",
                parent,
                nested_text(
                    &self.config,
                    "models",
                    &format!("{parent}_parent_effort"),
                    if parent == "claude" {
                        "natural"
                    } else {
                        "medium"
                    }
                )
            ),
            format!(
                "{} scout model    {}",
                parent,
                nested_text(
                    &self.config,
                    "models",
                    &format!("{parent}_scout_model"),
                    if parent == "claude" {
                        "opus"
                    } else {
                        "gpt-5.6-luna"
                    }
                )
            ),
            format!(
                "{} scout effort   {}",
                parent,
                nested_text(
                    &self.config,
                    "models",
                    &format!("{parent}_scout_effort"),
                    "auto"
                )
            ),
            format!(
                "seed             {}",
                self.config
                    .get("seed")
                    .map(|seed| match seed {
                        Value::String(value) if !value.is_empty() => value.clone(),
                        Value::Number(value) => value.to_string(),
                        _ => "(loading)".into(),
                    })
                    .unwrap_or_else(|| "(loading)".into())
            ),
            format!(
                "RepoTracer binary {}",
                nested_text(&self.config, "repotracer", "binary", "(auto)")
            ),
            format!(
                "source path      {}",
                nested_text(&self.config, "repotracer", "source_path", "(working tree)")
            ),
            format!("changed binary   {}", {
                let value = arm_text(&self.config, "changed", "binary");
                if value.is_empty() {
                    "(unset)".into()
                } else {
                    value
                }
            }),
            format!("changed source   {}", {
                let value = arm_text(&self.config, "changed", "source_path");
                if value.is_empty() {
                    "(unset)".into()
                } else {
                    value
                }
            }),
            format!(
                "acceptance       {}",
                nested_text(&self.config, "acceptance", "command", "(none)")
            ),
            format!(
                "rate cards       {} configured",
                self.config
                    .get("rate_cards")
                    .and_then(Value::as_object)
                    .map_or(0, Map::len)
            ),
            "preset            daily · manual start only".into(),
        ];
        let items = rows
            .iter()
            .enumerate()
            .map(|(index, value)| {
                ListItem::new(Span::styled(
                    format!(
                        "{}{}",
                        if index == self.selected {
                            theme.pointer()
                        } else {
                            "  "
                        },
                        value
                    ),
                    if index == self.selected {
                        theme.highlight()
                    } else {
                        theme.plain()
                    },
                ))
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default()
            .with_selected(Some(self.selected.min(items.len().saturating_sub(1))));
        frame.render_stateful_widget(List::new(items), area, &mut state);
    }

    fn draw_run(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let Some(run) = self.selected_run() else {
            frame.render_widget(
                Paragraph::new("No runs yet. Press r on Tasks or Models to start one."),
                area,
            );
            return;
        };
        let progress = run.get("progress").cloned().unwrap_or_else(|| json!({}));
        let (done, total) = if let Some(tasks) = run.get("tasks").and_then(Value::as_array) {
            (
                tasks
                    .iter()
                    .filter(|task| {
                        matches!(
                            text(task, "state").as_str(),
                            "completed" | "failed" | "interrupted"
                        )
                    })
                    .count() as u64,
                tasks.len() as u64,
            )
        } else {
            (
                progress.get("done").and_then(Value::as_u64).unwrap_or(0),
                progress.get("total").and_then(Value::as_u64).unwrap_or(0),
            )
        };
        let state = text(run, "state");
        let mut lines = vec![
            Line::from(vec![
                Span::styled("run      ", theme.dim()),
                Span::raw(format!("{}  {state}", text(run, "id"))),
            ]),
            Line::from(vec![
                Span::styled("progress ", theme.dim()),
                Span::raw(format!("{done}/{total} tasks")),
            ]),
        ];
        if let Some(tasks) = run.get("tasks").and_then(Value::as_array) {
            for task in tasks {
                let mut status = format!("{}  {}", text(task, "id"), text(task, "state"));
                if let Some(arms) = task.get("arms").and_then(Value::as_array) {
                    for arm in arms {
                        let cost = number(arm, "cost_usd")
                            .map(|value| format!("${value:.4}"))
                            .unwrap_or_else(|| "cost ?".into());
                        let seconds = elapsed_text(arm);
                        let tokens = arm
                            .get("tokens")
                            .map(format_tokens)
                            .unwrap_or_else(|| "tokens ?".into());
                        let usage = if bool_value(arm, "usage_complete") {
                            ""
                        } else {
                            " · usage incomplete"
                        };
                        status.push_str(&format!(
                            "\n  {} {} {cost} {seconds} {tokens}{usage}",
                            text(arm, "group"),
                            text(arm, "state")
                        ));
                    }
                }
                lines.push(Line::from(status));
            }
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }

    fn draw_results(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let result_rows = self.result_rows();
        if result_rows.is_empty() {
            frame.render_widget(Paragraph::new("No completed result summaries yet. Incomplete runs remain visible on Run and History."), area);
            return;
        }
        let mut lines = vec![Line::from(Span::styled(
            "  group             cost       time       tokens       quality / tests",
            theme.bold(),
        ))];
        for (index, result) in result_rows.iter().enumerate() {
            let tokens = result
                .get("tokens")
                .map(format_tokens)
                .unwrap_or_else(|| "tokens ?".into());
            let quality = format_quality(result);
            let tests = result
                .get("tests")
                .map(format_tests)
                .unwrap_or_else(|| "tests ?".into());
            let incomplete = if result
                .get("usage_complete")
                .is_some_and(|value| !value.as_bool().unwrap_or(false))
            {
                " · incomplete"
            } else {
                ""
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "{}{:<17} {:<10} {:<10} {:<12} {} / {}{}",
                    if index == self.selected_result {
                        theme.pointer()
                    } else {
                        " "
                    },
                    text(result, "group"),
                    result
                        .get("cost_usd")
                        .map(format_number)
                        .unwrap_or_else(|| "cost ?".into()),
                    result
                        .get("seconds")
                        .map(format_number)
                        .unwrap_or_else(|| "time ?".into()),
                    tokens,
                    quality,
                    tests,
                    incomplete
                ),
                if index == self.selected_result {
                    theme.highlight()
                } else {
                    theme.plain()
                },
            )));
        }
        let mut run_ids = Vec::new();
        for row in &result_rows {
            let id = text(row, "run_id");
            if !id.is_empty() && !run_ids.contains(&id) {
                run_ids.push(id);
            }
        }
        if run_ids.len() < 2 {
            lines.push(Line::from(Span::styled(
                format!(
                    "\nNot enough runs to establish a reliable median (n={}).",
                    run_ids.len()
                ),
                theme.warn(),
            )));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }

    fn draw_investigations(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut rows = Vec::new();
        for (index, item) in self.investigations.iter().enumerate() {
            let diagnosis = text(item, "diagnosis");
            let suffix = if diagnosis.is_empty() {
                "diagnosis required"
            } else {
                &diagnosis
            };
            rows.push(ListItem::new(Span::styled(
                format!(
                    "{}{}  {} / {}  {}",
                    if index == self.selected_investigation {
                        theme.pointer()
                    } else {
                        "  "
                    },
                    text(item, "state"),
                    text(item, "task_id"),
                    text(item, "group"),
                    suffix
                ),
                if index == self.selected_investigation {
                    theme.highlight()
                } else {
                    theme.plain()
                },
            )));
        }
        if rows.is_empty() {
            frame.render_widget(Paragraph::new("Worse cost/time/quality comparisons appear here after a report. Enter starts a detached investigation."), area);
        } else {
            let mut state = ListState::default()
                .with_selected(Some(self.selected_investigation.min(rows.len() - 1)));
            frame.render_stateful_widget(List::new(rows), area, &mut state);
        }
    }

    fn draw_history(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let rows = self
            .runs
            .iter()
            .enumerate()
            .map(|(index, run)| {
                ListItem::new(Span::styled(
                    format!(
                        "{}{}  {}  {}  {}/{}",
                        if index == self.selected_run {
                            theme.pointer()
                        } else {
                            "  "
                        },
                        text(run, "state"),
                        text(run, "id"),
                        text(run, "created_at"),
                        run.get("progress")
                            .and_then(|v| v.get("done"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        run.get("progress")
                            .and_then(|v| v.get("total"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                    ),
                    if index == self.selected_run {
                        theme.highlight()
                    } else {
                        theme.plain()
                    },
                ))
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            frame.render_widget(Paragraph::new("No saved runs yet."), area);
        } else {
            let mut state =
                ListState::default().with_selected(Some(self.selected_run.min(rows.len() - 1)));
            frame.render_stateful_widget(List::new(rows), area, &mut state);
        }
    }

    fn draw_editor(
        &self,
        frame: &mut Frame,
        area: Rect,
        editor: &TextEditor,
        title: String,
        theme: &Theme,
    ) {
        let mut lines = vec![Line::from(Span::styled(
            format!("{title}  ·  Ctrl+S or Esc saves",),
            theme.accent(),
        ))];
        for (index, value) in editor.lines.iter().enumerate() {
            let mut line = value.clone();
            if index == editor.line {
                let chars = line.chars().collect::<Vec<_>>();
                let cursor = editor.column.min(chars.len());
                line = chars[..cursor].iter().collect::<String>()
                    + if self.unicode { "▌" } else { "|" }
                    + &chars[cursor..].iter().collect::<String>();
            }
            lines.push(Line::from(line));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }

    fn footer(&self) -> &'static str {
        match self.page {
            Page::Config => "↑↓ select · Enter edit · a/x add · d remove · s save · r start · Tab next · q quit (jobs continue)",
            Page::Task => "↑↓ fields · Enter edit · p project · m parent · Esc back",
            Page::Models => "↑↓ field · Enter edit · </> Codex/Claude · s save · r start · Tab next",
            Page::Run => "↑↓ run · Tab next · jobs continue when you navigate or quit",
            Page::Results => "↑↓ candidate · a then Enter apply winning custom candidate · Tab next",
            Page::Investigations => "↑↓ comparison · Enter investigate · a then Enter apply custom candidate · Tab next",
            Page::History => "↑↓ run · Tab next · q quit; detached jobs are never cancelled",
        }
    }
}

fn default_config() -> Value {
    json!({
        "schema_version": 1,
        "seed": 0,
        "tasks": [
            {"id":"custom-1","origin":"custom","project_path":".","prompt":"","apply_allowed":false,"parent":"codex"},
            {"id":"custom-2","origin":"custom","project_path":".","prompt":"","apply_allowed":false,"parent":"codex"},
            {"id":"custom-3","origin":"custom","project_path":".","prompt":"","apply_allowed":false,"parent":"codex"},
            {"id":"external-1","origin":"external","source":"swebench-lite","dataset_path":"","instance_id":"","project_path":"","prompt":"","apply_allowed":false,"parent":"claude"}
        ],
        "arms": [
            {"name":"baseline","enabled":true,"repotracer":false,"version":"parent-only"},
            {"name":"current","enabled":true,"repotracer":true,"version":"working"},
            {"name":"changed","enabled":false,"repotracer":true,"version":"changed"}
        ],
        "models": {
            "codex_parent_model":"gpt-5.6-sol","codex_parent_effort":"medium","codex_scout_model":"gpt-5.6-luna","codex_scout_effort":"auto",
            "claude_parent_model":"opus","claude_parent_effort":"natural","claude_scout_model":"opus","claude_scout_effort":"auto-low-medium"
        },
        "repotracer":{"binary":"","source_path":""},
        "acceptance":{"command":""},
        "rate_cards":{},
        "presets":[{"name":"daily","schedule":"manual","arms":["baseline","current"]}]
    })
}

fn format_number(value: &Value) -> String {
    value
        .as_f64()
        .map(|number| format!("{number:.3}"))
        .unwrap_or_else(|| "?".into())
}

fn elapsed_text(value: &Value) -> String {
    if let Some(seconds) = number(value, "seconds") {
        return format!("{seconds:.0}s");
    }
    if let Some(started) = value.get("started_at").and_then(Value::as_str) {
        if let Ok(started) = DateTime::parse_from_rfc3339(started) {
            let seconds = (Utc::now() - started.with_timezone(&Utc))
                .num_milliseconds()
                .max(0) as f64
                / 1000.0;
            return format!("{seconds:.0}s");
        }
    }
    "time ?".into()
}

fn format_tokens(value: &Value) -> String {
    if let Some(total) = value.as_u64() {
        return total.to_string();
    }
    if let Some(object) = value.as_object() {
        let parent = object.get("parent").and_then(Value::as_u64);
        let scout = object.get("scout").and_then(Value::as_u64);
        return match (parent, scout) {
            (Some(parent), Some(scout)) => format!("{}+{}", parent, scout),
            _ => "incomplete".into(),
        };
    }
    "?".into()
}

fn format_tests(value: &Value) -> String {
    value
        .as_array()
        .map(|tests| format!("{} outcomes", tests.len()))
        .unwrap_or_else(|| "tests ?".into())
}

fn format_quality(value: &Value) -> String {
    for key in ["quality", "grade"] {
        let Some(candidate) = value.get(key) else {
            continue;
        };
        if let Some(label) = candidate.as_str() {
            return label.to_owned();
        }
        if let Some(score) = candidate.as_f64() {
            return format!("grade {score:.1}");
        }
        if let Some(score) = candidate.get("score").and_then(Value::as_f64) {
            return format!("grade {score:.1}");
        }
    }
    value
        .get("quality_delta")
        .and_then(Value::as_f64)
        .map(|value| format!("delta {value:+.1}"))
        .unwrap_or_else(|| "quality ?".into())
}

fn is_loss(value: &Value) -> bool {
    value
        .get("diagnosis_required")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.is_empty())
        || bool_value(value, "diagnosis_required")
}

fn is_win(value: &Value) -> bool {
    if bool_value(value, "winner") {
        return true;
    }
    if value
        .get("excluded")
        .and_then(Value::as_array)
        .is_some_and(|reasons| !reasons.is_empty())
        || is_loss(value)
        || value
            .get("state")
            .and_then(Value::as_str)
            .is_some_and(|state| state != "completed")
    {
        return false;
    }
    value
        .get("quality_delta")
        .and_then(Value::as_f64)
        .is_some_and(|delta| delta > 0.0)
        || ["cost_ratio", "time_ratio"].iter().any(|key| {
            value
                .get(*key)
                .and_then(Value::as_f64)
                .is_some_and(|ratio| ratio < 1.0)
        })
}

fn truncate(value: &str, max: usize) -> String {
    let mut output = value.chars().take(max).collect::<String>();
    if value.chars().count() > max {
        output.push('…');
    }
    output.replace('\n', " ↵ ")
}

fn centered(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = area.width.min(max_width);
    let height = area.height.min(max_height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn unicode_enabled() -> bool {
    if std::env::var_os("REPOTRACER_ASCII").is_some() {
        return false;
    }
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(std::env::var_os)
        .map(|value| value.to_string_lossy().to_lowercase().replace('-', ""))
        .is_some_and(|value| value.contains("utf8"))
}

struct TerminalSession;
impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stderr(), EnterAlternateScreen)?;
        Ok(Self)
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stderr(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

pub fn run(options: BenchmarkOptions) -> Result<()> {
    if !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        return Err(anyhow!("benchmarks is interactive and requires a terminal; use tools/benchmarks/workflow.py directly for redirected or automated runs"));
    }
    let client = BackendClient {
        engine: match options.engine {
            Some(engine) => engine,
            None => crate::benchmark_assets::install()?,
        },
        state_dir: options.state_dir.unwrap_or_else(default_state_dir),
    };
    let session = TerminalSession::enter().context("enter benchmark terminal")?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stderr())).context("create benchmark terminal")?;
    let (sender, receiver) = mpsc::channel();
    let mut app = App::new(client, sender.clone(), receiver);
    spawn_request(app.client.clone(), Request::Init, sender);
    app.busy = true;
    let result = loop {
        app.poll_backend();
        app.poll_active();
        terminal
            .draw(|frame| app.draw(frame))
            .context("draw benchmark terminal")?;
        if event::poll(Duration::from_millis(100)).context("read benchmark terminal")? {
            if let Event::Key(key) = event::read().context("read benchmark key")? {
                if !app.handle(key) {
                    break Ok(());
                }
            }
        }
    };
    drop(terminal);
    drop(session);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        let (sender, receiver) = mpsc::channel();
        let mut app = App::new(
            BackendClient {
                engine: PathBuf::from("workflow.py"),
                state_dir: PathBuf::from(".benchmark-runs"),
            },
            sender,
            receiver,
        );
        app.config = default_config();
        app.loading = false;
        app.config_loaded = true;
        app.unicode = false;
        app
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn defaults_have_three_custom_and_one_external_task() {
        let app = app();
        assert_eq!(app.task_count(), 4);
        assert_eq!(text(app.tasks()[3], "origin"), "external");
        assert!(text(app.tasks()[3], "prompt").is_empty());
    }

    #[test]
    fn multiline_editor_round_trips_and_moves() {
        let mut editor = TextEditor::new("one\ntwo");
        editor.up();
        editor.newline();
        editor.insert('x');
        assert_eq!(editor.value(), "one\nx\ntwo");
    }

    #[test]
    fn narrow_terminal_has_actionable_message() {
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut app = app();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Enlarge this window"));
    }

    #[test]
    fn normal_terminal_renders_tabs_and_task_count() {
        let mut app = app();
        let rendered = screen(&mut app, 100, 24);
        assert!(rendered.contains("RepoTracer benchmarks"));
        assert!(rendered.contains("custom-1"));
        assert!(rendered.contains("SWE-bench"));
    }

    #[test]
    fn external_prompt_cannot_be_edited() {
        let mut app = app();
        app.task = 3;
        app.selected = 1;
        app.start_edit(EditField::Prompt);
        app.finish_edit();
        assert!(app.message.contains("dataset"));
    }

    #[test]
    fn placeholder_config_cannot_overwrite_saved_config_before_init() {
        let mut app = app();
        app.config_loaded = false;
        app.save();
        assert!(!app.busy);
        assert!(app.message.contains("configuration to load"));
        assert_eq!(app.config["seed"], json!(0));
    }

    #[test]
    fn snapshot_keeps_backend_seed() {
        let mut app = app();
        app.config_loaded = false;
        app.apply_snapshot(json!({
            "config": {"schema_version": 1, "seed": 4242, "tasks": [], "arms": []},
            "runs": [],
            "investigations": [],
            "results": []
        }));
        assert!(app.config_loaded);
        assert_eq!(app.config["seed"], json!(4242));
    }

    #[test]
    fn zero_grade_is_visible() {
        assert_eq!(format_quality(&json!({"grade": {"score": 0}})), "grade 0.0");
        assert_eq!(format_quality(&json!({"quality_delta": 0})), "delta +0.0");
    }

    #[test]
    fn result_apply_uses_saved_run_permission() {
        let mut app = app();
        app.page = Page::Results;
        app.runs = vec![json!({
            "id": "run-1",
            "tasks": [{"id":"custom-1", "origin":"custom", "apply_allowed":true}]
        })];
        app.results = vec![json!({"pairs": [{
            "run_id":"run-1", "task":"custom-1", "group":"current",
            "state":"completed", "quality_delta":1
        }]})];
        assert!(!bool_value(app.tasks()[0], "apply_allowed"));
        app.begin_apply();
        assert_eq!(
            app.confirm_apply,
            Some(ApplyTarget {
                run: "run-1".into(),
                task: "custom-1".into(),
                group: "current".into()
            })
        );
    }

    #[test]
    fn current_config_cannot_enable_apply_for_an_old_run() {
        let mut app = app();
        app.page = Page::Results;
        app.config["tasks"][0]["apply_allowed"] = Value::Bool(true);
        app.runs = vec![json!({
            "id": "run-1",
            "tasks": [{"id":"custom-1", "origin":"custom", "apply_allowed":false}]
        })];
        app.results = vec![json!({"pairs": [{
            "run_id":"run-1", "task":"custom-1", "group":"current",
            "state":"completed", "quality_delta":1
        }]})];
        app.begin_apply();
        assert!(app.confirm_apply.is_none());
        assert!(app.message.contains("saved run did not authorize"));
    }

    #[test]
    fn rate_cards_require_the_pricing_shape() {
        let valid = r#"{
            "gpt": {
                "model": "gpt-5.6-sol", "source": "vendor pricing",
                "uncached_input": 1.0, "cache_read": 0.1,
                "cache_write": 0, "output": 5
            }
        }"#;
        assert!(parse_rate_cards(valid).is_ok());
        assert!(parse_rate_cards("[]").unwrap_err().contains("JSON object"));
        assert!(parse_rate_cards(r#"{"gpt":{"model":"gpt"}}"#)
            .unwrap_err()
            .contains("source"));
    }

    #[test]
    fn invalid_rate_card_edit_does_not_replace_config() {
        let mut app = app();
        app.config["rate_cards"] = json!({"kept": {
            "model": "gpt", "source": "source", "uncached_input": 1,
            "cache_read": 1, "cache_write": 1, "output": 1
        }});
        app.edit = Some(EditField::RateCards);
        app.editor = Some(TextEditor::new(r#"{"broken": []}"#));
        app.finish_edit();
        assert!(app.config["rate_cards"].get("kept").is_some());
        assert!(app.message.contains("must be an object"));
    }

    #[test]
    fn changed_arm_paths_are_editable() {
        let mut app = app();
        app.edit = Some(EditField::ChangedBinary);
        app.editor = Some(TextEditor::new("/tmp/repotracer-next"));
        app.finish_edit();
        assert_eq!(
            arm_text(&app.config, "changed", "binary"),
            "/tmp/repotracer-next"
        );
    }
}
