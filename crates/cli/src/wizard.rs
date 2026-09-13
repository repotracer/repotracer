//! Full-screen setup and settings. State changes are separate from rendering;
//! only the caller writes configuration, after an explicit Save.
use crate::{
    model_catalog::{self, Catalog, ModelChoice},
    select,
};
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
use std::{io, sync::mpsc, time::Duration};

/// Presentation only. Both axes degrade independently: a terminal without
/// colour keeps every shape, and a terminal without UTF-8 keeps every colour.
struct Theme {
    unicode: bool,
    color: bool,
}

impl Theme {
    fn plain(&self) -> Style {
        Style::default()
    }

    fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
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
            self.plain()
        }
    }

    fn danger(&self) -> Style {
        if self.color {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            self.bold()
        }
    }

    /// A filled pill reads as "selected" far faster than reversed body text.
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
            "❯ "
        } else {
            "> "
        }
    }

    fn checkbox(&self, checked: bool) -> &'static str {
        match (self.unicode, checked) {
            (true, true) => "◉",
            (true, false) => "◯",
            (false, true) => "[x]",
            (false, false) => "[ ]",
        }
    }

    fn step_mark(&self, active: bool) -> &'static str {
        match (self.unicode, active) {
            (true, true) => "●",
            (true, false) => "○",
            (false, true) => "*",
            (false, false) => "-",
        }
    }

    fn caret(&self) -> &'static str {
        if self.unicode {
            "▌"
        } else {
            "|"
        }
    }

    fn separator(&self) -> &'static str {
        if self.unicode {
            " · "
        } else {
            " | "
        }
    }

    /// Three tints so the wordmark reads as one shape with depth instead of a
    /// flat slab. Without colour the same gradient is carried by weight.
    fn banner(&self, row: usize) -> Style {
        match (self.color, row) {
            (true, 0) => Style::default().fg(Color::LightCyan),
            (true, 1) => Style::default().fg(Color::Cyan),
            (true, _) => Style::default().fg(Color::DarkGray),
            (false, 0) => self.bold(),
            (false, 1) => self.plain(),
            (false, _) => self.dim(),
        }
    }

    fn border(&self) -> BorderType {
        if self.unicode {
            BorderType::Rounded
        } else {
            BorderType::Plain
        }
    }
}

/// Box-drawing and glyphs are wrong on a terminal that cannot encode them, so
/// the check is the encoding, not the terminal name. `REPOTRACER_ASCII` forces
/// the fallback for terminals that claim UTF-8 and render it badly.
fn unicode_enabled() -> bool {
    if std::env::var_os("REPOTRACER_ASCII").is_some() {
        return false;
    }
    #[cfg(windows)]
    {
        std::env::var_os("WT_SESSION").is_some() || std::env::var_os("TERM_PROGRAM").is_some()
    }
    #[cfg(not(windows))]
    {
        ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(std::env::var_os)
            .map(|value| value.to_string_lossy().to_lowercase().replace('-', ""))
            .is_some_and(|value| value.contains("utf8"))
    }
}

/// Centre the chrome instead of stretching it. A wizard that fills a 200-column
/// terminal edge to edge is harder to read than one held to a column.
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

#[derive(Clone, PartialEq, Eq, Default)]
pub struct CustomApiProfile {
    pub base_url: String,
    pub api_key: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CurrentProfile {
    pub parent: String,
    pub choice: Option<ModelChoice>,
    pub custom: Option<CustomApiProfile>,
    pub reasoning_effort: Option<String>,
    /// Saved Codex service tier, if the profile carries one. `None` means the
    /// profile never set it, so the model's own default applies.
    pub fast_tier: Option<bool>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ParentModelChoice {
    pub parent: String,
    pub model: ModelChoice,
    pub custom: Option<CustomApiProfile>,
    pub reasoning_effort: Option<String>,
    /// Resolved service tier, or `None` where the backend has no such concept.
    /// Only Codex reads a tier; Claude and custom endpoints must not be given
    /// one, so the caller writes nothing for them.
    pub fast_tier: Option<bool>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub chosen: Vec<ParentModelChoice>,
    /// Parents that were installed and have been unchecked. The wizard never
    /// writes; the caller detaches these.
    pub removed: Vec<String>,
}

const PARENTS: [&str; 2] = ["codex", "claude"];
const LABELS: [&str; 2] = ["Codex", "Claude Code"];

const RECOMMENDED_CODEX_MODEL: &str = "gpt-5.6-luna";

fn recommended_model(index: usize) -> ModelChoice {
    let (provider, id, label) = match index {
        0 => ("codex", RECOMMENDED_CODEX_MODEL, "Codex — gpt-5.6-luna"),
        1 => ("claude", "opus", "Claude Code — opus"),
        _ => unreachable!("the parent list has two entries"),
    };
    ModelChoice {
        provider: provider.into(),
        id: id.into(),
        label: label.into(),
    }
}

/// The fast tier is a Codex-only knob: the Claude CLI has no tier or speed
/// setting, and a custom endpoint's tiers are its own business.
fn tier_applies(model: &ModelChoice) -> bool {
    model.provider == "codex"
}

/// Fast tier is worth its cost on the model the wizard recommends and not
/// assumed for the rest, so every other Codex model opens on the normal tier.
/// The non-interactive setup path applies the same rule.
pub fn default_fast_tier_for(model_id: &str) -> bool {
    model_id.trim() == RECOMMENDED_CODEX_MODEL
}

fn default_fast_tier(model: &ModelChoice) -> bool {
    tier_applies(model) && default_fast_tier_for(&model.id)
}

/// Half-block wordmark, 39 columns. Drawn only where there is room for it;
/// every other size falls back to the one-line lockup.
const WORDMARK: [&str; 3] = [
    "█▀▄ █▀▀ █▀▄ █▀█ ▀█▀ █▀▄ ▄▀▄ █▀▀ █▀▀ █▀▄",
    "█▀▄ █▀▀ █▀  █ █  █  █▀▄ █▀█ █   █▀▀ █▀▄",
    "▀ ▀ ▀▀▀ ▀   ▀▀▀  ▀  ▀ ▀ ▀ ▀ ▀▀▀ ▀▀▀ ▀ ▀",
];
const TAGLINE: &str = "small models investigate. big models solve.";

/// The picker leads with "Custom model…" so an endpoint the catalog cannot
/// know about is the first thing offered, not the last thing found. Every
/// catalog index is therefore offset by this one row.
const CUSTOM_ROW_COUNT: usize = 1;

/// One editable setting on the Scout models page. Every setting is a row the
/// reader can arrow onto, so nothing is reachable only through a shortcut key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Model,
    Effort,
    FastTier,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Model => "Model",
            Field::Effort => "Effort",
            Field::FastTier => "Fast tier",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Install,
    Models,
    Picker,
    Custom,
    Effort,
}

struct App {
    page: Page,
    installed: [bool; 2],
    enabled: [bool; 2],
    choices: [Option<ModelChoice>; 2],
    custom: [Option<CustomApiProfile>; 2],
    selected_efforts: [Option<String>; 2],
    /// `None` follows the chosen model's default; a toggle pins it.
    fast_tiers: [Option<bool>; 2],
    catalog: Catalog,
    loading: bool,
    focus: usize,
    editing: usize,
    query: String,
    custom_fields: [String; 3],
    custom_field: usize,
    custom_key_touched: bool,
    custom_loading: bool,
    custom_receiver: Option<mpsc::Receiver<Result<model_catalog::DiscoveredModels, String>>>,
    custom_pending: Option<(usize, CustomApiProfile)>,
    custom_discovered: [Option<(CustomApiProfile, model_catalog::DiscoveredModels)>; 2],
    picker: ListState,
    message: String,
    no_color: bool,
    unicode: bool,
}

enum Outcome {
    Continue,
    Cancel,
    Save(ModelSelection),
}

fn same_origin(left: &str, right: &str) -> bool {
    match (reqwest::Url::parse(left), reqwest::Url::parse(right)) {
        (Ok(left), Ok(right)) => left.origin() == right.origin(),
        _ => false,
    }
}

impl App {
    #[cfg(test)]
    fn new(installed: &[String], current: &[(String, Option<ModelChoice>)]) -> Self {
        let profiles = current
            .iter()
            .map(|(parent, choice)| CurrentProfile {
                parent: parent.clone(),
                choice: choice.clone(),
                custom: None,
                reasoning_effort: None,
                fast_tier: None,
            })
            .collect::<Vec<_>>();
        Self::new_with_profiles(installed, &profiles)
    }

    fn new_with_profiles(installed: &[String], current: &[CurrentProfile]) -> Self {
        let installed = PARENTS.map(|parent| installed.iter().any(|value| value == parent));
        // Nothing installed is a first run, and someone reaching this screen
        // came to install. Offer both agents already checked rather than
        // opening on a choice that does nothing until they turn something on.
        // A rerun keeps mirroring disk, so unchecking still means removal.
        let first_run = !installed.iter().any(|value| *value);
        let enabled = installed.map(|value| value || first_run);
        let custom = PARENTS.map(|parent| {
            current
                .iter()
                .find(|profile| profile.parent == parent)
                .and_then(|profile| profile.custom.clone())
        });
        let selected_efforts = PARENTS.map(|parent| {
            current
                .iter()
                .find(|profile| profile.parent == parent)
                .and_then(|profile| profile.reasoning_effort.clone())
        });
        let fast_tiers = PARENTS.map(|parent| {
            current
                .iter()
                .find(|profile| profile.parent == parent)
                .and_then(|profile| profile.fast_tier)
        });
        Self {
            page: Page::Install,
            installed,
            enabled,
            choices: PARENTS.map(|parent| {
                let index = PARENTS
                    .iter()
                    .position(|candidate| *candidate == parent)
                    .expect("parent index");
                current
                    .iter()
                    .find(|profile| profile.parent == parent)
                    .and_then(|profile| profile.choice.clone())
                    .or_else(|| enabled[index].then(|| recommended_model(index)))
            }),
            custom,
            selected_efforts,
            fast_tiers,
            catalog: Catalog::default(),
            loading: true,
            focus: 0,
            editing: 0,
            query: String::new(),
            custom_fields: [
                "https://api.openai.com/v1".into(),
                String::new(),
                String::new(),
            ],
            custom_field: 0,
            custom_key_touched: false,
            custom_loading: false,
            custom_receiver: None,
            custom_pending: None,
            custom_discovered: [None, None],
            picker: ListState::default(),
            message: String::new(),
            no_color: std::env::var_os("NO_COLOR").is_some(),
            unicode: unicode_enabled(),
        }
    }

    fn theme(&self) -> Theme {
        Theme {
            unicode: self.unicode,
            color: !self.no_color,
        }
    }

    /// Land on Save only when there is something to save. Opening on a Save
    /// button that immediately rejects the press is the worst first keystroke.
    /// Every focusable setting on the Scout models page, in screen order. The
    /// tier row exists only where the backend has a tier, so the shape of this
    /// list follows the chosen models rather than being fixed.
    fn fields(&self) -> Vec<(usize, Field)> {
        let mut fields = Vec::new();
        for index in self.parents() {
            fields.push((index, Field::Model));
            fields.push((index, Field::Effort));
            if self.effective_fast_tier(index).is_some() {
                fields.push((index, Field::FastTier));
            }
        }
        fields
    }

    fn field_focus(&self, parent: usize, field: Field) -> usize {
        self.fields()
            .iter()
            .position(|entry| *entry == (parent, field))
            .unwrap_or(0)
    }

    /// Rows the settings list occupies. Compact drops the per-agent heading
    /// and the gap between agents, so a short viewport spends its rows on
    /// values rather than on structure.
    fn models_rows(&self, compact: bool) -> u16 {
        let fields = self.fields().len() as u16;
        if compact {
            fields
        } else {
            let parents = self.parents().len() as u16;
            fields + parents + parents.saturating_sub(1)
        }
    }

    fn enter_models_page(&mut self) {
        self.page = Page::Models;
        // Open on the first thing that still needs an answer; with everything
        // answered there is nothing to review, so open on Save.
        self.focus = self
            .parents()
            .iter()
            .find(|index| self.choices[**index].is_none())
            .map(|index| self.field_focus(*index, Field::Model))
            .unwrap_or_else(|| self.fields().len());
    }

    fn edit_field(&mut self, position: usize) {
        let Some((parent, field)) = self.fields().get(position).copied() else {
            return;
        };
        match field {
            Field::Model => self.open_picker(parent),
            Field::Effort => self.open_effort(parent),
            Field::FastTier => self.toggle_fast_tier(parent),
        }
    }

    /// Up and down move between rows. The buttons share one row at the foot of
    /// the page, so they count as a single stop: leaving them lands back on a
    /// setting instead of stepping sideways through the other buttons.
    fn move_row(&mut self, rows: usize, forward: bool) {
        if rows == 0 {
            return;
        }
        let on_buttons = self.focus >= rows;
        self.focus = match (forward, on_buttons) {
            (true, true) => 0,
            (true, false) if self.focus + 1 < rows => self.focus + 1,
            (true, false) => rows,
            (false, true) => rows - 1,
            (false, false) if self.focus > 0 => self.focus - 1,
            (false, false) => rows,
        };
    }

    /// Left and right stay inside the button row. The settings above it are a
    /// column, so sideways movement there has nowhere to go, and wrapping out
    /// of Cancel into the top of the page reads as the focus jumping.
    fn move_button(&mut self, rows: usize, buttons: usize, forward: bool) {
        if self.focus < rows || buttons == 0 {
            return;
        }
        let current = self.focus - rows;
        let next = if forward {
            (current + 1) % buttons
        } else {
            (current + buttons - 1) % buttons
        };
        self.focus = rows + next;
    }

    fn set_catalog(&mut self, catalog: Catalog) {
        self.catalog = catalog;
        self.loading = false;
        let row = self.first_model_row();
        self.picker.select(Some(row));
    }

    fn parents(&self) -> Vec<usize> {
        (0..2).filter(|index| self.enabled[*index]).collect()
    }

    /// Unchecking an installed agent is the uninstall gesture: the same box
    /// that put the integration there takes it away.
    fn removals(&self) -> Vec<String> {
        (0..2)
            .filter(|index| self.installed[*index] && !self.enabled[*index])
            .map(|index| PARENTS[index].to_owned())
            .collect()
    }

    /// Installed agents still checked — what the Uninstall button would stage.
    /// The button is offered only while it would actually do something.
    fn removable(&self) -> Vec<usize> {
        (0..2)
            .filter(|index| self.installed[*index] && self.enabled[*index])
            .collect()
    }

    /// Focus stops on the Install page: two checkboxes, the primary action,
    /// Cancel, and an Uninstall button that only exists while something is
    /// installed. The count is therefore not fixed.
    fn install_stops(&self) -> usize {
        if self.removable().is_empty() {
            4
        } else {
            5
        }
    }

    /// Stage every installed agent for removal. Staging rather than removing
    /// keeps the confirmation in the flow: the rows say "will be removed" and
    /// the primary button says "Remove" before anything is touched.
    fn stage_uninstall(&mut self) {
        for index in self.removable() {
            self.enabled[index] = false;
        }
        self.focus = 2;
    }

    fn save(&mut self) -> Outcome {
        let mut chosen = Vec::new();
        for index in self.parents() {
            let Some(model) = self.choices[index].clone() else {
                self.message = format!("Choose a scout for {} first.", LABELS[index]);
                self.focus = self.field_focus(index, Field::Model);
                return Outcome::Continue;
            };
            let fast_tier = self.effective_fast_tier(index);
            chosen.push(ParentModelChoice {
                parent: PARENTS[index].into(),
                model,
                custom: self.custom[index].clone(),
                reasoning_effort: self.selected_efforts[index].clone(),
                fast_tier,
            });
        }
        let removed = self.removals();
        // Removing every integration is a real outcome, not an empty selection.
        if chosen.is_empty() && removed.is_empty() {
            return Outcome::Continue;
        }
        Outcome::Save(ModelSelection { chosen, removed })
    }

    fn candidates(&self) -> Vec<ModelChoice> {
        let mut models = self.catalog.models.clone();
        if let Some((_, (discovered, _))) = &self.custom_discovered[self.editing] {
            models.extend(discovered.clone());
        }
        if let Some(saved) = &self.choices[self.editing] {
            if !models.iter().any(|model| same_model(model, saved)) {
                models.push(saved.clone());
            }
        }
        let terms = self.query.to_lowercase();
        models.retain(|model| {
            format!("{} {} {}", model.provider, model.id, model.label)
                .to_lowercase()
                .contains(&terms)
        });
        models
    }

    fn open_picker(&mut self, index: usize) {
        self.editing = index;
        self.page = Page::Picker;
        self.query.clear();
        let selected = self.choices[index]
            .as_ref()
            .and_then(|current| {
                self.candidates()
                    .iter()
                    .position(|model| same_model(model, current))
                    .map(|position| position + CUSTOM_ROW_COUNT)
            })
            .unwrap_or_else(|| self.first_model_row());
        self.picker = ListState::default().with_selected(Some(selected));
    }

    /// Row zero is the custom-model entry, so the catalog starts one below it.
    /// Filtering and reopening land on a model, never on the escape hatch.
    fn first_model_row(&self) -> usize {
        if self.candidates().is_empty() {
            0
        } else {
            CUSTOM_ROW_COUNT
        }
    }

    fn known_efforts(&self, index: usize) -> Option<&Vec<String>> {
        let model = self.choices[index].as_ref()?;
        let key = model_catalog::model_key(&model.provider, &model.id);
        if model.provider == "openai-compatible" {
            let (connection, (_, efforts)) = self.custom_discovered[index].as_ref()?;
            if self.custom[index].as_ref() != Some(connection) {
                return None;
            }
            efforts.get(&key)
        } else {
            self.catalog.reasoning_efforts.get(&key)
        }
    }

    fn choose_model(&mut self, model: ModelChoice) {
        let index = self.editing;
        let old_connection = self.custom[index].clone();
        let same = self.choices[index]
            .as_ref()
            .is_some_and(|old| same_model(old, &model));
        if model.provider == "openai-compatible" {
            if let Some((connection, (models, _))) = &self.custom_discovered[index] {
                if models.iter().any(|entry| same_model(entry, &model)) {
                    self.custom[index] = Some(connection.clone());
                }
            }
            if self.custom[index].is_none() {
                self.custom[index] = Some(self.custom_connection());
            }
        } else {
            self.custom[index] = None;
        }
        if !same || old_connection != self.custom[index] {
            self.selected_efforts[index] = None;
            // A pinned tier belonged to the model it was pinned on; the new one
            // opens on its own default rather than inheriting that decision.
            self.fast_tiers[index] = None;
        }
        self.choices[index] = Some(model);
    }

    /// `None` where the backend has no tier at all, so nothing is written for
    /// Claude or a custom endpoint. Otherwise the pinned value, or the model's
    /// default while nothing is pinned.
    fn effective_fast_tier(&self, index: usize) -> Option<bool> {
        let model = self.choices[index].as_ref()?;
        tier_applies(model).then(|| self.fast_tiers[index].unwrap_or(default_fast_tier(model)))
    }

    fn toggle_fast_tier(&mut self, index: usize) {
        if let Some(current) = self.effective_fast_tier(index) {
            self.fast_tiers[index] = Some(!current);
        }
    }

    fn effort_candidates(&self) -> Vec<String> {
        self.effort_candidates_for(self.editing)
    }

    fn effort_candidates_for(&self, index: usize) -> Vec<String> {
        self.known_efforts(index).cloned().unwrap_or_default()
    }

    fn open_effort(&mut self, index: usize) {
        self.editing = index;
        let options = self.effort_options();
        let selected = self.selected_efforts[index]
            .as_ref()
            .and_then(|current| {
                options
                    .iter()
                    .position(|level| level.as_deref() == Some(current.as_str()))
            })
            .unwrap_or(0);
        self.picker = ListState::default().with_selected(Some(selected));
        self.page = Page::Effort;
    }

    fn effort_options(&self) -> Vec<Option<String>> {
        let mut levels = self.effort_candidates();
        if let Some(current) = &self.selected_efforts[self.editing] {
            if !levels.contains(current) {
                levels.push(current.clone());
            }
        }
        std::iter::once(None)
            .chain(levels.into_iter().map(Some))
            .collect()
    }

    fn effort_default_copy(&self) -> (&'static str, &'static str) {
        if self.choices[self.editing]
            .as_ref()
            .is_some_and(|model| model.provider == "openai-compatible")
        {
            (
                "Provider default",
                "Provider default lets the provider choose. Manual overrides are optional.",
            )
        } else {
            (
                "Auto (agent-selected)",
                "Auto lets the agent choose. Manual overrides are optional.",
            )
        }
    }

    fn open_custom(&mut self) {
        self.invalidate_custom_discovery();
        self.custom_field = 0;
        if let Some(profile) = &self.custom[self.editing] {
            self.custom_fields[0] = profile.base_url.clone();
            // Keep the old value out of the editable buffer. It is retained
            // unless the user explicitly changes this field.
            self.custom_fields[2].clear();
        } else {
            self.custom_fields[0] = "https://api.openai.com/v1".into();
            self.custom_fields[2].clear();
        }
        self.custom_key_touched = false;
        self.custom_fields[1] = self.choices[self.editing]
            .as_ref()
            .filter(|choice| choice.provider == "openai-compatible")
            .map(|choice| choice.id.clone())
            .unwrap_or_default();
        self.page = Page::Custom;
        self.query.clear();
    }

    fn start_custom_discovery(&mut self) {
        let base_url = self.custom_fields[0].trim().to_owned();
        if base_url.is_empty() {
            self.message = "Enter a base URL before discovering models.".into();
            return;
        }
        let connection = self.custom_connection();
        let key = connection.api_key.clone();
        self.invalidate_custom_discovery();
        let (sender, receiver) = mpsc::channel();
        self.custom_loading = true;
        self.custom_receiver = Some(receiver);
        self.custom_pending = Some((self.editing, connection));
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(model_catalog::discover_openai_models(
                            &base_url,
                            key.as_deref(),
                        ))
                        .map_err(|error| error.to_string())
                });
            let _ = sender.send(result);
        });
    }

    fn custom_connection(&self) -> CustomApiProfile {
        let base_url = self.custom_fields[0].trim().to_owned();
        let entered_key = &self.custom_fields[2];
        CustomApiProfile {
            api_key: if self.custom_key_touched || !entered_key.trim().is_empty() {
                (!entered_key.trim().is_empty()).then(|| entered_key.clone())
            } else {
                self.custom[self.editing]
                    .as_ref()
                    .filter(|profile| same_origin(&profile.base_url, &base_url))
                    .and_then(|profile| profile.api_key.clone())
            },
            base_url,
        }
    }

    fn invalidate_custom_discovery(&mut self) {
        self.custom_receiver = None;
        self.custom_pending = None;
        self.custom_loading = false;
        self.custom_discovered[self.editing] = None;
    }

    fn poll_custom_discovery(&mut self) {
        let Some(receiver) = &self.custom_receiver else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Model discovery stopped unexpectedly.".into())
            }
        };
        self.custom_receiver = None;
        self.custom_loading = false;
        let Some((parent, connection)) = self.custom_pending.take() else {
            return;
        };
        if parent != self.editing
            || self.page != Page::Custom
            || connection != self.custom_connection()
        {
            return;
        }
        match result {
            Ok(models) => {
                self.custom_discovered[parent] = Some((connection, models));
                self.page = Page::Picker;
                self.query.clear();
                let row = self.first_model_row();
                self.picker.select(Some(row));
            }
            Err(error) => self.message = format!("Model discovery failed: {error}"),
        }
    }

    fn handle(&mut self, key: KeyEvent) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => return Outcome::Cancel,
                KeyCode::Char('s') if self.page == Page::Models => return self.save(),
                KeyCode::Char('u') if matches!(self.page, Page::Picker | Page::Custom) => {
                    self.query.clear();
                    let row = self.first_model_row();
                    self.picker.select(Some(row));
                }
                _ => {}
            }
            return Outcome::Continue;
        }
        self.message.clear();
        match self.page {
            Page::Install => match key.code {
                KeyCode::Esc => return Outcome::Cancel,
                KeyCode::BackTab => {
                    let stops = self.install_stops();
                    self.focus = (self.focus + stops - 1) % stops;
                }
                KeyCode::Tab => {
                    self.focus = (self.focus + 1) % self.install_stops();
                }
                KeyCode::Up => self.move_row(2, false),
                KeyCode::Down => self.move_row(2, true),
                KeyCode::Left => {
                    let buttons = self.install_stops() - 2;
                    self.move_button(2, buttons, false);
                }
                KeyCode::Right => {
                    let buttons = self.install_stops() - 2;
                    self.move_button(2, buttons, true);
                }
                KeyCode::Char(' ') if self.focus < 2 => {
                    self.enabled[self.focus] = !self.enabled[self.focus];
                    if self.enabled[self.focus] && self.choices[self.focus].is_none() {
                        self.choices[self.focus] = Some(recommended_model(self.focus));
                    }
                }
                // A shortcut next to the button, so the way out is reachable
                // without hunting for it.
                KeyCode::Char('r' | 'R') if !self.removable().is_empty() => self.stage_uninstall(),
                KeyCode::Enter if self.focus == self.install_stops() - 1 => return Outcome::Cancel,
                KeyCode::Enter if self.focus == 3 => self.stage_uninstall(),
                KeyCode::Enter => {
                    if !self.parents().is_empty() {
                        self.enter_models_page();
                    } else if !self.removals().is_empty() {
                        // Nothing left checked and something installed: there is
                        // no model to pick, only integrations to remove.
                        return self.save();
                    } else {
                        self.message = "Select Codex or Claude Code with Space.".into();
                    }
                }
                _ => {}
            },
            Page::Models => {
                // Settings first, then Save, Back and Cancel.
                let count = self.fields().len();
                let stops = count + 3;
                match key.code {
                    KeyCode::Esc => {
                        self.page = Page::Install;
                        self.focus = 2;
                    }
                    KeyCode::BackTab => self.focus = (self.focus + stops - 1) % stops,
                    KeyCode::Tab => self.focus = (self.focus + 1) % stops,
                    KeyCode::Up => self.move_row(count, false),
                    KeyCode::Down => self.move_row(count, true),
                    KeyCode::Left => self.move_button(count, 3, false),
                    KeyCode::Right => self.move_button(count, 3, true),
                    // Space reads as "flip this" and Enter as "open this", but
                    // a toggle is the same thing either way.
                    KeyCode::Char(' ') if self.focus < count => self.edit_field(self.focus),
                    KeyCode::Enter if self.focus < count => self.edit_field(self.focus),
                    KeyCode::Enter if self.focus == count => return self.save(),
                    KeyCode::Enter if self.focus == count + 1 => {
                        self.page = Page::Install;
                        self.focus = 2;
                    }
                    KeyCode::Enter => return Outcome::Cancel,
                    _ => {}
                }
            }
            Page::Picker => {
                let candidates = self.candidates();
                // The custom model entry leads the list and is always reachable.
                let count = candidates.len() + CUSTOM_ROW_COUNT;
                let selected = self.picker.selected().unwrap_or(0).min(count - 1);
                match key.code {
                    KeyCode::Esc => {
                        self.page = Page::Models;
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        self.picker.select(Some((selected + count - 1) % count))
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        self.picker.select(Some((selected + 1) % count))
                    }
                    KeyCode::Enter if selected < CUSTOM_ROW_COUNT => {
                        self.open_custom();
                    }
                    KeyCode::Enter => {
                        self.choose_model(candidates[selected - CUSTOM_ROW_COUNT].clone());
                        self.page = Page::Models;
                    }
                    KeyCode::Backspace => {
                        self.query.pop();
                        let row = self.first_model_row();
                        self.picker.select(Some(row));
                    }
                    KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::ALT) => {
                        self.query.push(character);
                        let row = self.first_model_row();
                        self.picker.select(Some(row));
                    }
                    _ => {}
                }
            }
            Page::Custom => match key.code {
                KeyCode::Esc => {
                    self.invalidate_custom_discovery();
                    self.page = Page::Picker;
                    self.query.clear();
                    let row = self.first_model_row();
                    self.picker.select(Some(row));
                }
                KeyCode::F(2) => self.start_custom_discovery(),
                KeyCode::Tab | KeyCode::Down => {
                    self.custom_field = (self.custom_field + 1) % self.custom_fields.len();
                }
                KeyCode::BackTab | KeyCode::Up => {
                    self.custom_field = (self.custom_field + self.custom_fields.len() - 1)
                        % self.custom_fields.len();
                }
                KeyCode::Enter
                    if self.custom_field == 0
                        && !self.custom_fields[0].trim().starts_with("http") =>
                {
                    self.message = "Base URL must be an http(s) URL.".into();
                }
                KeyCode::Enter if self.custom_field + 1 < self.custom_fields.len() => {
                    self.custom_field += 1;
                }
                KeyCode::Enter => {
                    let base_url = self.custom_fields[0].trim();
                    let model = self.custom_fields[1].trim();
                    if base_url.is_empty() || !base_url.starts_with("http") {
                        self.message = "Base URL must be an http(s) URL.".into();
                    } else if model.is_empty() || model.chars().any(char::is_control) {
                        self.message = "Enter a model ID, or press F2 to discover models.".into();
                    } else {
                        let model = model.to_owned();
                        let connection = self.custom_connection();
                        self.invalidate_custom_discovery();
                        if self.custom[self.editing].as_ref() != Some(&connection) {
                            self.selected_efforts[self.editing] = None;
                        }
                        self.custom[self.editing] = Some(connection);
                        self.choose_model(ModelChoice {
                            provider: "openai-compatible".into(),
                            label: format!("Custom API — {model}"),
                            id: model,
                        });
                        self.page = Page::Models;
                    }
                }
                KeyCode::Backspace => {
                    self.invalidate_custom_discovery();
                    if self.custom_field == 2 && !self.custom_key_touched {
                        self.custom_key_touched = true;
                    }
                    self.custom_fields[self.custom_field].pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::ALT) => {
                    self.invalidate_custom_discovery();
                    if self.custom_field == 2 && !self.custom_key_touched {
                        self.custom_key_touched = true;
                        self.custom_fields[2].clear();
                    }
                    self.custom_fields[self.custom_field].push(character)
                }
                _ => {}
            },
            Page::Effort => {
                let options = self.effort_options();
                let count = options.len();
                if count == 0 {
                    self.page = Page::Models;
                } else {
                    let selected = self.picker.selected().unwrap_or(0).min(count - 1);
                    match key.code {
                        KeyCode::Esc => self.page = Page::Models,
                        KeyCode::Up | KeyCode::BackTab => {
                            self.picker.select(Some((selected + count - 1) % count))
                        }
                        KeyCode::Down | KeyCode::Tab => {
                            self.picker.select(Some((selected + 1) % count))
                        }
                        KeyCode::Enter => {
                            self.selected_efforts[self.editing] = options[selected].clone();
                            self.page = Page::Models;
                        }
                        _ => {}
                    }
                }
            }
        }
        Outcome::Continue
    }

    fn model_text(&self, index: usize, model: &ModelChoice, discovery_result: bool) -> String {
        let known = if model.provider == "openai-compatible" {
            self.custom_discovered[index]
                .as_ref()
                .is_some_and(|(connection, (models, _))| {
                    models.iter().any(|candidate| same_model(candidate, model))
                        && (discovery_result || self.custom[index].as_ref() == Some(connection))
                })
        } else {
            self.catalog
                .models
                .iter()
                .any(|candidate| same_model(candidate, model))
        };
        // One source of truth: what the wizard pre-selects is what it marks.
        let recommended =
            (0..PARENTS.len()).any(|parent| same_model(&recommended_model(parent), model));
        let suffix = if self.loading {
            if recommended {
                " [recommended; availability pending]"
            } else {
                " [availability pending]"
            }
        } else if !known {
            " [unverified]"
        } else if recommended {
            " [recommended]"
        } else {
            ""
        };
        format!("{}:{}{suffix}", model.provider, model.id)
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 32 || area.height < 10 {
            frame.render_widget(
                Paragraph::new("Enlarge this window to 32 x 10 to continue.\nCtrl+C cancels.")
                    .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        let theme = self.theme();
        // Hold the chrome to a readable column and centre it, rather than
        // pinning content to the top-left of an arbitrarily large terminal.
        const CARD_WIDTH: u16 = 84;
        let compact = area.height < 18;
        let gap = u16::from(!compact);
        // The wordmark earns its four rows only where the viewport can spare
        // them; anywhere tighter the one-line lockup carries the brand.
        let banner = self.unicode && area.height >= 26 && area.width >= 48;
        let brand = if banner { 4 } else { 1 };
        // Size the card to its page. A box stretched to the viewport reads as
        // an empty screen with a caption, which is what this looked like before.
        let chrome = brand + 4 + 3 * gap;
        let width = area.width.min(CARD_WIDTH);
        let wanted = chrome + 2 + self.body_height(compact, width.saturating_sub(4));
        // The floor is exactly what the layout below needs: chrome plus the
        // `Min(4)` body. Anything larger pads short pages with blank rows.
        let card = centered(area, CARD_WIDTH, wanted.max(chrome + 4));
        let rows = Layout::vertical([
            Constraint::Length(brand), // wordmark or one-line lockup
            Constraint::Length(gap),
            Constraint::Length(1), // step rail
            Constraint::Length(gap),
            Constraint::Min(4), // page body
            Constraint::Length(gap),
            Constraint::Length(1), // actions
            Constraint::Length(1), // page keys
            Constraint::Length(1), // message or global keys
        ])
        .split(card);

        if banner {
            self.draw_banner(frame, rows[0], &theme);
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(")))", theme.accent()),
                    Span::styled(" RepoTracer", theme.bold()),
                    Span::styled("  setup & settings", theme.dim()),
                ])),
                rows[0],
            );
        }
        self.draw_steps(frame, rows[2], &theme);

        let title = match self.page {
            Page::Install => " Install in ".to_owned(),
            Page::Models => " Scout models ".to_owned(),
            Page::Picker => format!(" Model for {} ", LABELS[self.editing]),
            Page::Custom => " Custom OpenAI-compatible API ".to_owned(),
            Page::Effort => " Reasoning effort ".to_owned(),
        };
        let frame_block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme.border())
            .border_style(theme.dim())
            .padding(Padding::horizontal(1))
            .title(Span::styled(title, theme.accent()));
        let body = frame_block.inner(rows[4]);
        frame.render_widget(frame_block, rows[4]);
        match self.page {
            Page::Install => self.draw_install(frame, body, compact, &theme),
            Page::Models => self.draw_models(frame, body, compact, &theme),
            Page::Effort => self.draw_effort(frame, body, compact, &theme),
            Page::Custom => self.draw_custom(frame, body, &theme),
            Page::Picker => self.draw_picker(frame, body, &theme),
        }

        self.draw_actions(frame, rows[6], &theme);
        let keys = match self.page {
            Page::Install if self.parents().is_empty() && !self.removals().is_empty() => {
                "Space toggle   Enter remove   Esc cancel"
            }
            Page::Install if !self.removable().is_empty() => {
                "Space toggle   Enter continue   R uninstall   Esc cancel"
            }
            Page::Install => "Space toggle   Enter continue   Esc cancel",
            Page::Models => "Enter change   Space toggle   Ctrl+S save",
            Page::Picker => "Type to filter   Enter apply   Esc back",
            Page::Custom => "Tab fields   F2 discover   Enter next   Esc back",
            Page::Effort => "Enter apply   Esc back",
        };
        frame.render_widget(Paragraph::new(Span::styled(keys, theme.dim())), rows[7]);
        // A message must never cost the reader the navigation keys, so the two
        // occupy separate rows rather than taking turns.
        let footer = if self.message.is_empty() {
            Line::from(Span::styled(
                format!("Tab / arrows move{}Ctrl+C cancel", theme.separator()),
                theme.dim(),
            ))
        } else {
            Line::from(Span::styled(self.message.clone(), theme.danger()))
        };
        frame.render_widget(Paragraph::new(footer), rows[8]);
    }

    /// Rows the current page wants inside the border. Lists are capped so a
    /// long catalog scrolls instead of pushing the footer off screen.
    fn body_height(&self, compact: bool, width: u16) -> u16 {
        let pad = if compact { 0 } else { 2 };
        let wrapped = |text: &str| {
            let width = width.max(1) as usize;
            text.chars().count().div_ceil(width).max(1) as u16
        };
        match self.page {
            Page::Install => pad + 2 + pad,
            Page::Models => {
                let rows = self.models_rows(compact);
                let note: u16 = if self.catalog.warnings.is_empty() || self.loading {
                    1
                } else {
                    self.catalog.warnings.iter().map(|w| wrapped(w)).sum()
                };
                rows + 1 + note
            }
            Page::Picker => 2 + (self.candidates().len() as u16 + 1).clamp(3, 12),
            Page::Custom => 5,
            Page::Effort => {
                let header = if compact { 1 } else { 2 };
                header + (self.effort_candidates().len() as u16).clamp(1, 8)
            }
        }
    }

    /// The radar sits on the middle row so it reads as part of the wordmark
    /// rather than a bullet in front of it.
    fn draw_banner(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut lines: Vec<Line> = WORDMARK
            .iter()
            .enumerate()
            .map(|(row, art)| {
                Line::from(vec![
                    Span::styled(
                        if row == 1 { "))) " } else { "    " },
                        theme.accent().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(*art, theme.banner(row)),
                ])
            })
            .collect();
        lines.push(Line::from(vec![
            Span::styled("    setup & settings", theme.accent()),
            Span::styled(format!("{}{TAGLINE}", theme.separator()), theme.dim()),
        ]));
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn draw_steps(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let on_install = self.page == Page::Install;
        let style = |active: bool| {
            if active {
                theme.accent().add_modifier(Modifier::BOLD)
            } else {
                theme.dim()
            }
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{} 1 Install in", theme.step_mark(on_install)),
                    style(on_install),
                ),
                Span::styled("    ", theme.plain()),
                Span::styled(
                    format!("{} 2 Scout models", theme.step_mark(!on_install)),
                    style(!on_install),
                ),
            ])),
            area,
        );
    }

    fn draw_actions(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (actions, offset): (&[&str], usize) = match self.page {
            // Name the button after what pressing it does. With every installed
            // agent unchecked there is nothing to continue to, only removal.
            Page::Install if self.parents().is_empty() && !self.removals().is_empty() => {
                (&["Remove", "Cancel"], 2)
            }
            // Removing an integration deserves its own button, not a gesture
            // the reader has to infer from the checkboxes.
            Page::Install if !self.removable().is_empty() => {
                (&["Continue", "Uninstall", "Cancel"], 2)
            }
            Page::Install => (&["Continue", "Cancel"], 2),
            // The buttons come after every setting row, so that is where their
            // focus numbering starts.
            Page::Models => (&["Save", "Back", "Cancel"], self.fields().len()),
            // A sub-page has one way out; say what it is instead of leaving a
            // row of dead space where buttons used to be.
            _ => {
                frame.render_widget(
                    Paragraph::new(Span::styled("Esc returns to Scout models", theme.dim())),
                    area,
                );
                return;
            }
        };
        let buttons: Vec<Span> = actions
            .iter()
            .enumerate()
            .flat_map(|(index, label)| {
                let style = if self.focus == offset + index {
                    theme.highlight()
                } else {
                    theme.dim()
                };
                [
                    Span::styled(format!("  {label}  "), style),
                    Span::styled(" ", theme.plain()),
                ]
            })
            .collect();
        frame.render_widget(Paragraph::new(Line::from(buttons)), area);
    }

    fn draw_install(&self, frame: &mut Frame, area: Rect, compact: bool, theme: &Theme) {
        let parts = Layout::vertical([
            Constraint::Length(if compact { 0 } else { 2 }),
            Constraint::Min(1),
            Constraint::Length(if compact { 0 } else { 2 }),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Pick the agents that should route through RepoTracer.",
                theme.dim(),
            ))
            .wrap(Wrap { trim: false }),
            parts[0],
        );
        let rows: Vec<ListItem> = (0..2)
            .map(|index| {
                // State the consequence of the current checkbox, not just the
                // state on disk, so an about-to-be-removed agent is obvious.
                let (note, style) = match (self.installed[index], self.enabled[index]) {
                    (true, true) => ("   already installed", theme.dim()),
                    (true, false) => ("   will be removed", theme.warn()),
                    (false, _) => ("", theme.dim()),
                };
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{} ", theme.checkbox(self.enabled[index]))),
                    Span::raw(LABELS[index]),
                    Span::styled(note, style),
                ]))
            })
            .collect();
        let mut state = ListState::default().with_selected((self.focus < 2).then_some(self.focus));
        frame.render_stateful_widget(
            List::new(rows)
                .highlight_style(theme.highlight())
                .highlight_symbol(theme.pointer()),
            parts[1],
            &mut state,
        );
        // Say what the checkbox will do, and say it before it is pressed.
        // "Not modified" is only true while nothing is installed.
        let note = if !self.removals().is_empty() {
            Span::styled(
                "Unchecking an installed agent removes its RepoTracer integration.",
                theme.warn(),
            )
        } else if !self.removable().is_empty() {
            Span::styled(
                "Uncheck an installed agent, or press R, to remove its integration.",
                theme.dim(),
            )
        } else {
            Span::styled("Agents left unchecked are not modified.", theme.dim())
        };
        frame.render_widget(
            Paragraph::new(vec![Line::default(), Line::from(note)]).wrap(Wrap { trim: false }),
            parts[2],
        );
    }

    fn draw_models(&mut self, frame: &mut Frame, area: Rect, compact: bool, theme: &Theme) {
        let parts = Layout::vertical([
            Constraint::Length(self.models_rows(compact)),
            Constraint::Min(1),
        ])
        .split(area);
        self.draw_parent_rows(frame, parts[0], compact, theme);
        let note = if self.loading {
            vec![Line::from(Span::styled(
                "Checking subscriptions…",
                theme.dim(),
            ))]
        } else if self.catalog.warnings.is_empty() {
            vec![Line::from(Span::styled(
                "Enter changes the highlighted setting.",
                theme.dim(),
            ))]
        } else {
            self.catalog
                .warnings
                .iter()
                .map(|warning| Line::from(Span::styled(warning.clone(), theme.warn())))
                .collect()
        };
        // parts[0] is sized to the rows exactly, so the gap belongs here.
        let note = std::iter::once(Line::default())
            .chain(note)
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(note).wrap(Wrap { trim: false }), parts[1]);
    }

    /// The value shown against a setting, and whether it still needs an answer.
    fn field_value(&self, parent: usize, field: Field, theme: &Theme) -> (String, bool) {
        let Some(model) = self.choices[parent].as_ref() else {
            return ("not chosen yet".into(), true);
        };
        match field {
            Field::Model => (self.model_text(parent, model, false), false),
            Field::Effort => {
                let default = if model.provider == "openai-compatible" {
                    "Provider default"
                } else {
                    "Auto"
                };
                (
                    self.selected_efforts[parent]
                        .as_deref()
                        .unwrap_or(default)
                        .to_owned(),
                    false,
                )
            }
            Field::FastTier => {
                let on = self.effective_fast_tier(parent).unwrap_or(false);
                (
                    format!("{} {}", theme.checkbox(on), if on { "on" } else { "off" }),
                    false,
                )
            }
        }
    }

    /// Every setting is its own row: the reader arrows onto the one they want
    /// instead of recalling which letter opens it.
    fn draw_parent_rows(&mut self, frame: &mut Frame, area: Rect, compact: bool, theme: &Theme) {
        let fields = self.fields();
        let active = (self.focus < fields.len()).then_some(self.focus);
        let mut lines: Vec<Line> = Vec::new();
        let mut focused_line = 0;
        let mut previous: Option<usize> = None;
        for (position, (parent, field)) in fields.iter().enumerate() {
            let first = previous != Some(*parent);
            // A compact viewport spends its rows on values, so the agent name
            // moves into a column instead of taking a heading of its own.
            if first && !compact {
                if previous.is_some() {
                    lines.push(Line::default());
                }
                lines.push(Line::from(Span::styled(LABELS[*parent], theme.bold())));
            }
            previous = Some(*parent);
            let selected = active == Some(position);
            if selected {
                focused_line = lines.len();
            }
            let (value, pending) = self.field_value(*parent, *field, theme);
            let style = if selected {
                theme.highlight()
            } else if pending {
                theme.warn()
            } else {
                theme.plain()
            };
            let mut spans = vec![Span::styled(
                if selected { theme.pointer() } else { "  " },
                theme.accent(),
            )];
            if compact {
                spans.push(Span::styled(
                    format!("{:<12}", if first { LABELS[*parent] } else { "" }),
                    theme.bold(),
                ));
            }
            spans.push(Span::styled(format!("{:<10}", field.label()), style));
            spans.push(Span::styled(value, style));
            lines.push(Line::from(spans));
        }
        // Keep the focused row on screen where the viewport cannot hold them all.
        let height = area.height.max(1) as usize;
        let scroll = (focused_line + 1).saturating_sub(height) as u16;
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
    }

    fn draw_picker(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let parts = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(area);
        // Keep the typed end visible, including on narrow terminals.
        let capacity = parts[0].width.saturating_sub(4) as usize;
        let input: String = self
            .query
            .chars()
            .rev()
            .take(capacity)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("/ ", theme.dim()),
                    Span::styled(input, theme.plain()),
                    Span::styled(theme.caret(), theme.accent()),
                ]),
                Line::from(Span::styled(
                    if self.loading {
                        "Checking subscriptions…".to_owned()
                    } else {
                        let count = self.candidates().len();
                        format!("{count} available")
                    },
                    theme.dim(),
                )),
            ]),
            parts[0],
        );
        let mut rows: Vec<ListItem> =
            vec![ListItem::new(Span::styled("Custom model…", theme.accent()))];
        rows.extend(
            self.candidates()
                .iter()
                .map(|model| ListItem::new(self.model_text(self.editing, model, true))),
        );
        frame.render_stateful_widget(
            List::new(rows)
                .highlight_style(theme.highlight())
                .highlight_symbol(theme.pointer()),
            parts[1],
            &mut self.picker,
        );
    }

    fn draw_custom(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let labels = ["Base URL", "Model ID", "API key"];
        let lines: Vec<Line> = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let focused = self.custom_field == index;
                // The key is masked whenever one is in effect, typed or saved.
                // Echoing what was just typed would defeat the whole point.
                let effective_key = (index == 2).then(|| self.custom_connection().api_key);
                let value = match &effective_key {
                    Some(Some(_)) => "••••••••".to_owned(),
                    Some(None) => String::new(),
                    None => self.custom_fields[index].clone(),
                };
                let mut spans = vec![
                    Span::styled(if focused { theme.pointer() } else { "  " }, theme.accent()),
                    Span::styled(
                        format!("{label:<10}"),
                        if focused { theme.bold() } else { theme.dim() },
                    ),
                    Span::styled(value, theme.plain()),
                ];
                if focused {
                    spans.push(Span::styled(theme.caret(), theme.accent()));
                } else if matches!(effective_key, Some(None)) {
                    spans.push(Span::styled("optional", theme.dim()));
                }
                Line::from(spans)
            })
            .chain(std::iter::once(Line::from(Span::styled(
                String::new(),
                theme.plain(),
            ))))
            .chain(std::iter::once(Line::from(Span::styled(
                if self.custom_loading {
                    "Discovering /models…".to_owned()
                } else {
                    "F2 lists models from GET /models. The key is never displayed.".to_owned()
                },
                theme.dim(),
            ))))
            .collect();
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }

    fn draw_effort(&mut self, frame: &mut Frame, area: Rect, compact: bool, theme: &Theme) {
        let parts = Layout::vertical([
            Constraint::Length(if compact { 1 } else { 2 }),
            Constraint::Min(1),
        ])
        .split(area);
        let (default_label, default_description) = self.effort_default_copy();
        frame.render_widget(
            Paragraph::new(default_description).wrap(Wrap { trim: false }),
            parts[0],
        );
        let rows = self
            .effort_options()
            .into_iter()
            .map(|effort| ListItem::new(effort.unwrap_or_else(|| default_label.into())))
            .collect::<Vec<_>>();
        frame.render_stateful_widget(
            List::new(rows)
                .highlight_style(theme.highlight())
                .highlight_symbol(theme.pointer()),
            parts[1],
            &mut self.picker,
        );
    }
}

fn same_model(left: &ModelChoice, right: &ModelChoice) -> bool {
    left.provider == right.provider && left.id == right.id
}

/// Restore on every return path, including unwinding.
struct TerminalSession;
impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let session = Self;
        execute!(io::stderr(), EnterAlternateScreen)?;
        Ok(session)
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stderr(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

pub fn configure_with_profiles(
    installed: &[String],
    current: &[CurrentProfile],
) -> io::Result<Option<ModelSelection>> {
    select::require_interactive_terminal()?;
    let session = TerminalSession::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stderr()))?;
    let mut app = App::new_with_profiles(installed, current);
    let discovery = model_catalog::Discovery::start()?;
    let result = (|| loop {
        if app.loading {
            match discovery.receiver.try_recv() {
                Ok(catalog) => app.set_catalog(catalog),
                Err(mpsc::TryRecvError::Disconnected) => app.set_catalog(Catalog {
                    warnings: vec![
                        "Model discovery stopped. Saved or custom models are still available."
                            .into(),
                    ],
                    ..Catalog::default()
                }),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        app.poll_custom_discovery();
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match app.handle(key) {
                    Outcome::Continue => {}
                    Outcome::Cancel => return Ok(None),
                    Outcome::Save(selection) => return Ok(Some(selection)),
                }
            }
        }
    })();
    drop(terminal);
    drop(session);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn model(provider: &str, id: &str) -> ModelChoice {
        ModelChoice {
            provider: provider.into(),
            id: id.into(),
            label: id.into(),
        }
    }
    fn app() -> App {
        let mut app = App::new(&["codex".into(), "claude".into()], &[]);
        // Glyphs otherwise follow the host locale, which a test must not depend on.
        app.unicode = true;
        app.set_catalog(Catalog {
            models: vec![
                model("codex", "gpt-5.6-luna"),
                model("claude", "sonnet"),
                model("claude", "opus"),
            ],
            ..Catalog::default()
        });
        app
    }
    fn key(app: &mut App, code: KeyCode) -> Outcome {
        app.handle(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            key(app, KeyCode::Char(character));
        }
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
            .collect::<String>()
    }

    #[test]
    fn recommended_save_needs_no_review_screen() {
        let mut app = app();
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Models);
        let Outcome::Save(selection) = key(&mut app, KeyCode::Enter) else {
            panic!("Save should be focused")
        };
        assert_eq!(selection.chosen[0].model.id, "gpt-5.6-luna");
        assert_eq!(selection.chosen[1].model.id, "opus");
    }

    #[test]
    fn fast_tier_is_on_for_the_recommended_codex_model_and_off_for_the_rest() {
        let mut app = app();
        assert_eq!(app.effective_fast_tier(0), Some(true));
        app.editing = 0;
        app.choose_model(model("codex", "gpt-5.6-pro"));
        assert_eq!(app.effective_fast_tier(0), Some(false));
    }

    #[test]
    fn claude_and_custom_endpoints_have_no_service_tier() {
        let mut app = app();
        assert_eq!(app.effective_fast_tier(1), None);
        app.editing = 1;
        app.choose_model(model("openai-compatible", "private"));
        assert_eq!(app.effective_fast_tier(1), None);

        // A backend with no tier has no tier row, so there is nothing to arrow
        // onto rather than a setting that answers "no such thing".
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Models);
        assert!(!app.fields().contains(&(1, Field::FastTier)));
        assert_eq!(
            app.fields(),
            vec![
                (0, Field::Model),
                (0, Field::Effort),
                (0, Field::FastTier),
                (1, Field::Model),
                (1, Field::Effort),
            ]
        );
        assert_eq!(app.effective_fast_tier(1), None);
    }

    #[test]
    fn toggling_the_tier_shows_on_the_row_and_reaches_the_saved_choice() {
        let mut app = app();
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Models);
        app.focus = app.field_focus(0, Field::FastTier);
        assert!(screen(&mut app, 100, 32).contains("Fast tier ◉ on"));
        // Space and Enter both flip it; neither is a letter to remember.
        key(&mut app, KeyCode::Char(' '));
        assert!(screen(&mut app, 100, 32).contains("Fast tier ◯ off"));

        let Outcome::Save(selection) = app.save() else {
            panic!("both parents have a model")
        };
        assert_eq!(selection.chosen[0].fast_tier, Some(false));
        // Claude carries no tier at all, so the caller writes none.
        assert_eq!(selection.chosen[1].fast_tier, None);
    }

    #[test]
    fn a_pinned_tier_does_not_follow_a_new_model() {
        let mut app = app();
        app.editing = 0;
        app.toggle_fast_tier(0);
        assert_eq!(app.fast_tiers[0], Some(false));
        app.choose_model(model("codex", "gpt-5.6-pro"));
        assert_eq!(app.fast_tiers[0], None);
        assert_eq!(app.effective_fast_tier(0), Some(false));
        app.choose_model(recommended_model(0));
        assert_eq!(app.effective_fast_tier(0), Some(true));
    }

    #[test]
    fn a_saved_tier_survives_reopening_the_wizard() {
        let profile = CurrentProfile {
            parent: "codex".into(),
            choice: Some(model("codex", "gpt-5.6-luna")),
            custom: None,
            reasoning_effort: None,
            fast_tier: Some(false),
        };
        let app = App::new_with_profiles(&["codex".into()], &[profile]);
        assert_eq!(app.effective_fast_tier(0), Some(false));
    }

    #[test]
    fn custom_form_only_shows_editable_inputs() {
        let mut app = app();
        app.open_picker(0);
        app.open_custom();
        assert!(!screen(&mut app, 80, 24).contains('_'));
        app.custom_fields[0].clear();
        type_text(&mut app, "http://localhost:8080/v1");
        assert!(screen(&mut app, 80, 24).contains("http://localhost:8080/v1"));
    }

    #[test]
    fn defaults_are_selected_and_saveable_before_discovery() {
        let mut app = App::new(&["codex".into(), "claude".into()], &[]);
        assert_eq!(app.choices[0].as_ref().unwrap().id, "gpt-5.6-luna");
        assert_eq!(app.choices[1].as_ref().unwrap().id, "opus");
        assert_eq!(app.selected_efforts, [None, None]);
        key(&mut app, KeyCode::Enter);
        assert!(screen(&mut app, 80, 24).contains("availability pending"));
        let Outcome::Save(selection) = app.save() else {
            panic!("the seeded defaults should be saveable while discovery is pending")
        };
        assert_eq!(selection.chosen.len(), 2);
        assert!(selection
            .chosen
            .iter()
            .all(|entry| entry.reasoning_effort.is_none()));
    }

    #[test]
    fn auto_is_available_before_discovery_and_preserves_unknown_manual_effort() {
        let mut app = App::new(&["codex".into()], &[]);
        app.selected_efforts[0] = Some("max".into());
        app.page = Page::Models;
        app.focus = app.field_focus(0, Field::Effort);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Effort);
        assert_eq!(app.effort_options(), vec![None, Some("max".into())]);
        assert_eq!(app.picker.selected(), Some(1));
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_efforts[0], None);
    }

    #[test]
    fn model_selection_does_not_force_effort_picker() {
        let mut app = app();
        app.catalog.reasoning_efforts.insert(
            model_catalog::model_key("claude", "sonnet"),
            vec!["medium".into(), "high".into()],
        );
        app.open_picker(0);
        type_text(&mut app, "sonnet");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Models);
        assert_eq!(app.selected_efforts[0], None);
    }

    #[test]
    fn late_discovery_preserves_saved_effort_override() {
        let saved = model("codex", "gpt-5.6-luna");
        let profile = CurrentProfile {
            parent: "codex".into(),
            choice: Some(saved.clone()),
            custom: None,
            reasoning_effort: Some("high".into()),
            fast_tier: None,
        };
        let mut app = App::new_with_profiles(&["codex".into()], &[profile]);
        app.set_catalog(Catalog {
            models: vec![saved],
            reasoning_efforts: [(
                model_catalog::model_key("codex", "gpt-5.6-luna"),
                vec!["low".into(), "medium".into(), "high".into()],
            )]
            .into_iter()
            .collect(),
            ..Catalog::default()
        });
        assert_eq!(app.selected_efforts[0].as_deref(), Some("high"));
    }

    #[test]
    fn effort_picker_defaults_to_auto_and_keeps_manual_levels_optional() {
        let mut app = App::new(&["codex".into()], &[]);
        app.set_catalog(Catalog {
            models: vec![model("codex", "gpt-5.6-luna")],
            reasoning_efforts: [(
                model_catalog::model_key("codex", "gpt-5.6-luna"),
                vec!["medium".into(), "high".into(), "xhigh".into(), "max".into()],
            )]
            .into_iter()
            .collect(),
            ..Catalog::default()
        });
        key(&mut app, KeyCode::Enter);
        app.focus = app.field_focus(0, Field::Effort);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Effort);
        assert!(screen(&mut app, 80, 24).contains("Auto (agent-selected)"));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_efforts[0], None);
        app.focus = app.field_focus(0, Field::Effort);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_efforts[0].as_deref(), Some("medium"));
    }

    #[test]
    fn effort_picker_uses_provider_default_for_custom_and_auto_for_native() {
        let mut native = App::new(&["codex".into()], &[]);
        native.page = Page::Effort;
        native.editing = 0;
        let native_screen = screen(&mut native, 80, 24);
        assert!(native_screen.contains("Auto (agent-selected)"));
        assert!(native_screen.contains("Auto lets the agent choose."));
        assert!(!native_screen.contains("Provider default"));

        let mut custom = App::new(&["codex".into()], &[]);
        custom.choices[0] = Some(model("openai-compatible", "private-model"));
        custom.page = Page::Effort;
        custom.editing = 0;
        let custom_screen = screen(&mut custom, 80, 24);
        assert!(custom_screen.contains("Provider default"));
        assert!(custom_screen.contains("Provider default lets the provider choose."));
        assert!(!custom_screen.contains("Auto (agent-selected)"));
        assert!(!custom_screen.contains("Auto lets the agent choose."));
    }

    #[test]
    fn missing_recommended_model_is_not_replaced_by_arbitrary_model() {
        let mut app = App::new(&["claude".into()], &[]);
        app.set_catalog(Catalog {
            models: vec![model("claude", "haiku")],
            ..Catalog::default()
        });
        assert_eq!(app.choices[1].as_ref().unwrap().id, "opus");
        let Outcome::Save(selection) = app.save() else {
            panic!("the unverified recommendation should remain saveable")
        };
        assert_eq!(selection.chosen[0].model.id, "opus");
        assert!(app
            .model_text(1, app.choices[1].as_ref().unwrap(), false)
            .contains("unverified"));
    }

    #[test]
    fn discovery_preserves_saved_cross_provider_and_custom_choices() {
        let catalog = app().catalog;
        let saved = model("claude", "private-model");
        let mut app = App::new(&["codex".into()], &[("codex".into(), Some(saved.clone()))]);
        app.set_catalog(catalog);
        assert_eq!(app.choices[0], Some(saved.clone()));
        app.open_picker(0);
        assert!(app.candidates().contains(&saved));
        assert!(app.model_text(0, &saved, false).contains("unverified"));
    }

    #[test]
    fn inline_search_applies_cross_provider_without_enabling_other_parent() {
        let mut app = app();
        app.enabled[1] = false;
        app.open_picker(0);
        type_text(&mut app, "SONNET");
        assert_eq!(app.candidates().len(), 1);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Models);
        let Outcome::Save(selection) = app.save() else {
            panic!()
        };
        assert_eq!(selection.chosen.len(), 1);
        assert_eq!(selection.chosen[0].model.provider, "claude");
    }

    #[test]
    fn back_keeps_edits_cancel_does_not_save() {
        let mut app = app();
        app.open_picker(0);
        type_text(&mut app, "opus");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.page, Page::Install);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.choices[0].as_ref().unwrap().id, "opus");
        assert!(matches!(
            app.handle(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Outcome::Cancel
        ));
    }

    #[test]
    fn empty_search_offers_validated_custom_entry() {
        let mut app = app();
        app.open_picker(0);
        type_text(&mut app, "missing");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Custom);
        app.custom_fields[0] = "not-a-url".into();
        app.custom_field = 0;
        key(&mut app, KeyCode::Enter);
        assert!(app.message.contains("http"));
        app.custom_fields[0] = "https://gateway.example/v1".into();
        app.custom_fields[1] = "vendor.private-model".into();
        app.custom_fields[2] = "secret".into();
        app.custom_field = 2;
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.choices[0].as_ref().unwrap().id, "vendor.private-model");
        assert_eq!(
            app.custom[0].as_ref().unwrap().base_url,
            "https://gateway.example/v1"
        );
        assert_eq!(
            app.custom[0].as_ref().unwrap().api_key.as_deref(),
            Some("secret")
        );
    }

    #[test]
    fn saved_custom_key_is_not_rendered_or_replaced_unless_touched() {
        let profile = CurrentProfile {
            parent: "codex".into(),
            choice: Some(model("openai-compatible", "private")),
            custom: Some(CustomApiProfile {
                base_url: "https://gateway.example/v1".into(),
                api_key: Some("secret".into()),
            }),
            reasoning_effort: None,
            fast_tier: None,
        };
        let mut app = App::new_with_profiles(&["codex".into()], &[profile]);
        app.unicode = true;
        app.set_catalog(Catalog::default());
        app.open_custom();
        assert!(app.custom_fields[2].is_empty());
        assert!(!screen(&mut app, 80, 24).contains("secret"));
        app.custom_field = 2;
        key(&mut app, KeyCode::Char('n'));
        assert_eq!(app.custom_fields[2], "n");
        assert!(app.custom_key_touched);
    }

    fn saved_custom_app() -> App {
        let mut app = App::new_with_profiles(
            &["codex".into()],
            &[CurrentProfile {
                parent: "codex".into(),
                choice: Some(model("openai-compatible", "private")),
                custom: Some(CustomApiProfile {
                    base_url: "https://old.example/v1".into(),
                    api_key: Some("saved-secret".into()),
                }),
                reasoning_effort: Some("high".into()),
                fast_tier: None,
            }],
        );
        app.unicode = true;
        app
    }

    fn pending_discovery(
        app: &mut App,
    ) -> mpsc::Sender<Result<model_catalog::DiscoveredModels, String>> {
        let (sender, receiver) = mpsc::channel();
        app.custom_pending = Some((app.editing, app.custom_connection()));
        app.custom_receiver = Some(receiver);
        app.custom_loading = true;
        sender
    }

    fn discovered() -> model_catalog::DiscoveredModels {
        (
            vec![model("openai-compatible", "private")],
            Default::default(),
        )
    }

    #[test]
    fn custom_discovery_and_manual_save_share_effective_credentials() {
        for (base_url, key_edit, expected_key) in [
            ("https://old.example/v2", None, Some("saved-secret")),
            ("https://new.example/v1", None, None),
            (
                "https://new.example/v1",
                Some("replacement"),
                Some("replacement"),
            ),
            ("https://new.example/v1", Some(""), None),
            ("https://new.example/v1", Some("  "), None),
        ] {
            for discover in [false, true] {
                let mut app = saved_custom_app();
                app.open_custom();
                app.custom_fields[0] = base_url.into();
                if let Some(value) = key_edit {
                    app.custom_field = 2;
                    key(&mut app, KeyCode::Backspace);
                    type_text(&mut app, value);
                }
                assert_eq!(app.custom_connection().api_key.as_deref(), expected_key);
                let rendered = screen(&mut app, 100, 30);
                assert!(!rendered.contains("saved-secret"));
                assert!(!rendered.contains("replacement"));
                assert_eq!(rendered.contains("••••••••"), expected_key.is_some());
                if discover {
                    let sender = pending_discovery(&mut app);
                    assert_eq!(
                        app.custom_pending.as_ref().unwrap().1.api_key.as_deref(),
                        expected_key
                    );
                    sender.send(Ok(discovered())).unwrap();
                    app.poll_custom_discovery();
                    assert_eq!(app.page, Page::Picker);
                    key(&mut app, KeyCode::Enter);
                } else {
                    app.custom_field = 2;
                    key(&mut app, KeyCode::Enter);
                }
                let Outcome::Save(selection) = app.save() else {
                    panic!("save failed")
                };
                let profile = selection.chosen[0].custom.as_ref().unwrap();
                assert_eq!(profile.base_url, base_url);
                assert_eq!(profile.api_key.as_deref(), expected_key);
                assert_eq!(selection.chosen[0].reasoning_effort, None);
            }
        }
    }

    #[test]
    fn stale_discovery_cannot_replace_newer_form_or_cancelled_page() {
        let mut app = saved_custom_app();
        app.open_custom();
        let old = pending_discovery(&mut app);
        key(&mut app, KeyCode::Char('x'));
        let new = pending_discovery(&mut app);
        new.send(Ok(discovered())).unwrap();
        app.poll_custom_discovery();
        assert!(old.send(Err("stale".into())).is_err());
        assert_eq!(
            app.custom_discovered[0].as_ref().unwrap().0.base_url,
            "https://old.example/v1x"
        );
        app.open_custom();
        let cancelled = pending_discovery(&mut app);
        key(&mut app, KeyCode::Esc);
        assert!(cancelled.send(Ok(discovered())).is_err());
        app.poll_custom_discovery();
        assert_eq!(app.page, Page::Picker);
        assert!(app.custom_discovered[0].is_none());
    }

    #[test]
    fn capability_refresh_preserves_manual_effort_and_automatic_default() {
        let mut app = saved_custom_app();
        app.set_catalog(Catalog::default());
        assert_eq!(app.selected_efforts[0].as_deref(), Some("high"));
        app.open_custom();
        pending_discovery(&mut app).send(Ok(discovered())).unwrap();
        app.poll_custom_discovery();
        app.choose_model(model("openai-compatible", "private"));
        assert_eq!(app.selected_efforts[0].as_deref(), Some("high"));
        for levels in [
            vec!["medium".into(), "high".into()],
            vec!["medium".into()],
            vec![],
        ] {
            app.custom_discovered[0].as_mut().unwrap().1 .1.insert(
                model_catalog::model_key("openai-compatible", "private"),
                levels,
            );
            app.choose_model(model("openai-compatible", "private"));
            assert_eq!(app.selected_efforts[0].as_deref(), Some("high"));
            app.open_effort(0);
            assert!(app.effort_options().contains(&Some("high".into())));
            app.selected_efforts[0] = None;
            app.choose_model(model("openai-compatible", "private"));
            assert_eq!(app.selected_efforts[0], None);
            app.selected_efforts[0] = Some("high".into());
        }
        app.selected_efforts[0] = Some("high".into());
        app.choose_model(model("codex", "another"));
        assert_eq!(app.selected_efforts[0], None);
    }

    #[test]
    fn native_refresh_does_not_erase_custom_results_or_unknown_native_effort() {
        let mut app = saved_custom_app();
        app.open_custom();
        pending_discovery(&mut app).send(Ok(discovered())).unwrap();
        app.poll_custom_discovery();
        app.set_catalog(Catalog::default());
        assert!(app.custom_discovered[0].is_some());
        let custom_model = model("openai-compatible", "private");
        assert!(!app
            .model_text(0, &custom_model, true)
            .contains("unverified"));
        app.choose_model(custom_model.clone());
        app.page = Page::Models;
        assert!(!app
            .model_text(0, &custom_model, false)
            .contains("unverified"));
        assert!(app
            .model_text(1, &custom_model, false)
            .contains("unverified"));
        app.open_picker(1);
        assert!(!app
            .candidates()
            .iter()
            .any(|m| m.provider == "openai-compatible"));
        app.choices[1] = Some(model("codex", "native"));
        app.selected_efforts[1] = Some("high".into());
        app.set_catalog(Catalog::default());
        assert_eq!(app.selected_efforts[1].as_deref(), Some("high"));
    }

    #[test]
    fn no_installation_selected_cannot_continue_or_save() {
        let mut app = App::new(&[], &[]);
        // A first run opens pre-checked, so clear it to reach the empty state
        // this guard is about.
        app.enabled = [false, false];
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Install);
        assert!(matches!(app.save(), Outcome::Continue));
        key(&mut app, KeyCode::Char(' '));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Models);
    }

    #[test]
    fn escape_from_picker_restores_mapping_focus() {
        let mut app = app();
        key(&mut app, KeyCode::Enter);
        app.focus = 1;
        key(&mut app, KeyCode::Enter);
        type_text(&mut app, "sonnet");
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.focus, 1);
        assert_eq!(app.choices[1].as_ref().unwrap().id, "opus");
    }

    #[test]
    fn screen_sizes_keep_controls_and_selection_visible() {
        for (width, height) in [(100, 30), (80, 24), (40, 16), (32, 10)] {
            let mut app = app();
            key(&mut app, KeyCode::Enter);
            let text = screen(&mut app, width, height);
            assert!(text.contains("Save"), "{width}x{height}: {text}");
            assert!(!text.contains("Review current values"));
            app.open_picker(0);
            type_text(&mut app, "sonnet");
            let text = screen(&mut app, width, height);
            assert!(text.contains("sonnet"), "{width}x{height}: {text}");
        }
    }

    #[test]
    fn small_terminal_has_recovery_instructions() {
        assert!(screen(&mut app(), 25, 6).contains("Enlarge"));
    }

    #[test]
    fn monochrome_retains_focus_without_color() {
        let mut app = app();
        app.no_color = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert!(terminal
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset));
        assert!(screen(&mut app, 80, 24).contains("❯ ◉ Codex"));
    }

    /// Cells drawn with the focus pill, in screen order.
    fn highlighted(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .filter(|cell| cell.bg == Color::Cyan)
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn the_focused_button_is_highlighted_once_the_settings_are_counted() {
        let mut app = app();
        key(&mut app, KeyCode::Enter);
        // The buttons are numbered after every setting row, so counting agents
        // instead of settings would leave the page with nothing highlighted.
        assert_eq!(app.focus, app.fields().len());
        assert!(highlighted(&mut app, 100, 32).contains("Save"));
        key(&mut app, KeyCode::Right);
        assert!(highlighted(&mut app, 100, 32).contains("Back"));
        key(&mut app, KeyCode::Up);
        let focused = highlighted(&mut app, 100, 32);
        assert!(focused.contains("Effort"));
        assert!(!focused.contains("Back"));
    }

    #[test]
    fn picker_scrolls_to_focused_model() {
        let mut app = app();
        app.catalog.models = (0..50)
            .map(|index| model("codex", &format!("model-{index:02}")))
            .collect();
        app.open_picker(0);
        // Row zero is the custom entry, so catalog entry 49 sits on row 50.
        app.picker.select(Some(50));
        assert!(screen(&mut app, 60, 18).contains("❯ codex:model-49"));
    }

    #[test]
    fn horizontal_navigation_stays_inside_the_button_row() {
        let mut app = app();
        key(&mut app, KeyCode::Enter);
        let save = app.fields().len();
        assert_eq!(app.focus, save);
        // Right cycles Save, Back, Cancel and round again; it never wraps up
        // into the settings, which are a column and not part of this row.
        for expected in [save + 1, save + 2, save] {
            key(&mut app, KeyCode::Right);
            assert_eq!(app.focus, expected);
        }
        key(&mut app, KeyCode::Left);
        assert_eq!(app.focus, save + 2);
        // Up leaves the row as a whole rather than stepping through Back.
        key(&mut app, KeyCode::Up);
        assert_eq!(app.focus, save - 1);
        key(&mut app, KeyCode::Right);
        assert_eq!(app.focus, save - 1);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.focus, save);

        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Install);
        assert_eq!(app.focus, 2);
        // The install page follows the same rule: its checkboxes are rows and
        // its buttons are one row underneath them.
        key(&mut app, KeyCode::Left);
        assert_eq!(app.focus, app.install_stops() - 1);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.focus, 1);
    }

    #[test]
    fn picker_keeps_focused_result_visible_at_minimum_size() {
        let mut app = app();
        app.open_picker(0);
        app.picker.select(Some(2));
        assert!(screen(&mut app, 32, 10).contains("❯ claude:sonnet"));
    }

    #[test]
    fn unchecking_an_installed_agent_requests_its_removal() {
        let mut app = app();
        app.enabled[1] = false;
        assert_eq!(app.removals(), vec!["claude".to_owned()]);
        key(&mut app, KeyCode::Enter);
        let Outcome::Save(selection) = key(&mut app, KeyCode::Enter) else {
            panic!("Save should be focused")
        };
        // The kept parent is still configured; only the unchecked one is cut.
        assert_eq!(selection.chosen.len(), 1);
        assert_eq!(selection.chosen[0].parent, "codex");
        assert_eq!(selection.removed, vec!["claude".to_owned()]);
    }

    #[test]
    fn an_agent_that_was_never_installed_is_not_a_removal() {
        let mut app = App::new(&["codex".into()], &[]);
        app.unicode = true;
        app.set_catalog(Catalog::default());
        assert!(!app.enabled[1]);
        assert_eq!(app.removals(), Vec::<String>::new());
    }

    #[test]
    fn clearing_every_installed_agent_saves_a_removal_instead_of_refusing() {
        let mut app = app();
        app.enabled = [false, false];
        // Previously this dead-ended on "Select Codex or Claude Code".
        let screen = screen(&mut app, 80, 24);
        assert!(screen.contains("Remove"));
        assert!(screen.contains("will be removed"));
        let Outcome::Save(selection) = key(&mut app, KeyCode::Enter) else {
            panic!("Enter should commit the removal")
        };
        assert!(selection.chosen.is_empty());
        assert_eq!(selection.removed, vec!["codex".to_owned(), "claude".into()]);
    }

    #[test]
    fn nothing_installed_and_nothing_chosen_still_asks_for_a_choice() {
        let mut app = App::new(&[], &[]);
        app.unicode = true;
        app.set_catalog(Catalog::default());
        // Clearing a first run leaves nothing to install and nothing to remove.
        app.enabled = [false, false];
        assert!(matches!(key(&mut app, KeyCode::Enter), Outcome::Continue));
        assert_eq!(app.message, "Select Codex or Claude Code with Space.");
    }

    #[test]
    fn a_first_run_opens_with_both_agents_checked() {
        let mut app = App::new(&[], &[]);
        app.unicode = true;
        app.set_catalog(Catalog::default());
        assert_eq!(app.enabled, [true, true]);
        // Pre-checking must not invent an install state: nothing is on disk,
        // so unchecking an agent is not a removal.
        assert_eq!(app.installed, [false, false]);
        assert_eq!(app.removals(), Vec::<String>::new());
        // Enter goes straight to the models page rather than scolding.
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Models);
    }

    #[test]
    fn a_rerun_mirrors_what_is_installed_rather_than_pre_checking() {
        let app = App::new(&["codex".into()], &[]);
        assert_eq!(app.enabled, [true, false]);
        assert_eq!(app.installed, [true, false]);
    }

    #[test]
    fn an_installed_run_offers_an_explicit_uninstall_button() {
        let mut app = app();
        let installed = screen(&mut app, 100, 32);
        assert!(installed.contains("Uninstall"));
        assert!(installed.contains("R uninstall"));
        // Nothing installed means nothing to remove, so the button is absent.
        let mut fresh = App::new(&[], &[]);
        fresh.unicode = true;
        fresh.set_catalog(Catalog::default());
        assert!(!screen(&mut fresh, 100, 32).contains("Uninstall"));
    }

    #[test]
    fn uninstall_stages_every_installed_agent_and_enter_commits_it() {
        let mut app = app();
        key(&mut app, KeyCode::Char('r'));
        assert_eq!(app.enabled, [false, false]);
        // Staged, not done: the page still has to be confirmed.
        let staged = screen(&mut app, 100, 32);
        assert!(staged.contains("will be removed"));
        assert!(staged.contains("Remove"));
        let Outcome::Save(selection) = key(&mut app, KeyCode::Enter) else {
            panic!("Enter on Remove should commit the removal")
        };
        assert!(selection.chosen.is_empty());
        assert_eq!(selection.removed, vec!["codex".to_owned(), "claude".into()]);
    }

    #[test]
    fn the_uninstall_button_is_reachable_by_tabbing() {
        let mut app = app();
        app.focus = 2;
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, 3);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.enabled, [false, false]);
        // With nothing left to stage the page drops back to two buttons.
        assert_eq!(app.install_stops(), 4);
        assert_eq!(app.focus, 2);
    }

    #[test]
    fn the_custom_model_entry_leads_the_picker() {
        let mut app = app();
        app.open_picker(1);
        // The saved model is still what opens focused, one row below custom.
        assert_eq!(app.picker.selected(), Some(3));
        let rendered = screen(&mut app, 80, 24);
        let custom = rendered.find("Custom model…").expect("custom entry");
        let first = rendered.find("codex:gpt-5.6-luna").expect("first model");
        assert!(custom < first, "custom must lead the list: {rendered}");
        // Filtering lands on a model, never on the escape hatch.
        type_text(&mut app, "sonnet");
        assert_eq!(app.picker.selected(), Some(1));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.choices[1].as_ref().unwrap().id, "sonnet");
    }

    #[test]
    fn the_leading_custom_entry_opens_the_custom_form() {
        let mut app = app();
        app.open_picker(0);
        app.picker.select(Some(0));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Custom);
    }

    #[test]
    fn an_empty_filter_result_still_reaches_the_custom_entry() {
        let mut app = app();
        app.open_picker(0);
        type_text(&mut app, "no-such-model");
        assert!(app.candidates().is_empty());
        assert_eq!(app.picker.selected(), Some(0));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Custom);
    }

    #[test]
    fn the_wordmark_appears_only_where_there_is_room_for_it() {
        let mut app = app();
        assert!(screen(&mut app, 100, 32).contains("▀▀▀"));
        // A short viewport keeps the one-line lockup rather than clipping art.
        let small = screen(&mut app, 100, 24);
        assert!(!small.contains("▀▀▀"));
        assert!(small.contains("))) RepoTracer"));
    }

    #[test]
    fn the_wordmark_is_never_drawn_without_utf8() {
        let mut app = app();
        app.unicode = false;
        let rendered = screen(&mut app, 100, 32);
        assert!(!rendered.contains('█'));
        assert!(rendered.contains(")))"));
    }
}
