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
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{io, sync::mpsc, time::Duration};

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
}

#[derive(Clone, PartialEq, Eq)]
pub struct ParentModelChoice {
    pub parent: String,
    pub model: ModelChoice,
    pub custom: Option<CustomApiProfile>,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ModelSelection(pub Vec<ParentModelChoice>);

const PARENTS: [&str; 2] = ["codex", "claude"];
const LABELS: [&str; 2] = ["Codex", "Claude Code"];

fn recommended_model(index: usize) -> ModelChoice {
    let (provider, id, label) = match index {
        0 => ("codex", "gpt-5.6-luna", "Codex — gpt-5.6-luna"),
        1 => ("claude", "sonnet", "Claude Code — sonnet"),
        _ => unreachable!("the parent list has two entries"),
    };
    ModelChoice {
        provider: provider.into(),
        id: id.into(),
        label: label.into(),
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
    picker: ListState,
    message: String,
    no_color: bool,
}

enum Outcome {
    Continue,
    Cancel,
    Save(ModelSelection),
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
            })
            .collect::<Vec<_>>();
        Self::new_with_profiles(installed, &profiles)
    }

    fn new_with_profiles(installed: &[String], current: &[CurrentProfile]) -> Self {
        let enabled = PARENTS.map(|parent| installed.iter().any(|value| value == parent));
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
        Self {
            page: Page::Install,
            installed: enabled,
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
            picker: ListState::default(),
            message: String::new(),
            no_color: std::env::var_os("NO_COLOR").is_some(),
        }
    }

    fn set_catalog(&mut self, catalog: Catalog) {
        self.catalog = catalog;
        self.loading = false;
        self.picker.select(Some(0));
    }

    fn parents(&self) -> Vec<usize> {
        (0..2).filter(|index| self.enabled[*index]).collect()
    }

    fn save(&mut self) -> Outcome {
        let mut result = Vec::new();
        for index in self.parents() {
            let Some(model) = self.choices[index].clone() else {
                self.message = format!("Choose a scout for {} first.", LABELS[index]);
                self.focus = self
                    .parents()
                    .iter()
                    .position(|value| *value == index)
                    .unwrap_or(0);
                return Outcome::Continue;
            };
            result.push(ParentModelChoice {
                parent: PARENTS[index].into(),
                model,
                custom: self.custom[index].clone(),
                reasoning_effort: self.selected_efforts[index].clone(),
            });
        }
        if result.is_empty() {
            return Outcome::Continue;
        }
        Outcome::Save(ModelSelection(result))
    }

    fn candidates(&self) -> Vec<ModelChoice> {
        let mut models = self.catalog.models.clone();
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
            })
            .unwrap_or(0);
        self.picker = ListState::default().with_selected(Some(selected));
    }

    fn choose_model(&mut self, model: ModelChoice) {
        let index = self.editing;
        let changed = self.choices[index]
            .as_ref()
            .is_none_or(|current| !same_model(current, &model));
        if model.provider == "openai-compatible" {
            if self.custom[index].is_none() {
                self.custom[index] = Some(CustomApiProfile {
                    base_url: self.custom_fields[0].clone(),
                    api_key: (!self.custom_fields[2].is_empty())
                        .then(|| self.custom_fields[2].clone()),
                });
            }
        } else {
            self.custom[index] = None;
        }
        self.choices[index] = Some(model);
        if changed {
            // A model switch returns to the safe automatic baseline. An
            // explicit effort for the same model survives catalog updates.
            self.selected_efforts[index] = None;
        }
    }

    fn effort_candidates(&self) -> Vec<String> {
        self.choices[self.editing]
            .as_ref()
            .and_then(|model| {
                self.catalog
                    .reasoning_efforts
                    .get(&model_catalog::model_key(&model.provider, &model.id))
            })
            .cloned()
            .unwrap_or_default()
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

    fn open_custom(&mut self) {
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
        let key = (!self.custom_fields[2].is_empty()).then(|| self.custom_fields[2].clone());
        let (sender, receiver) = mpsc::channel();
        self.custom_loading = true;
        self.custom_receiver = Some(receiver);
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
                    self.picker.select(Some(0));
                }
                _ => {}
            }
            return Outcome::Continue;
        }
        self.message.clear();
        match self.page {
            Page::Install => match key.code {
                KeyCode::Esc => return Outcome::Cancel,
                KeyCode::Up | KeyCode::Left | KeyCode::BackTab => self.focus = (self.focus + 3) % 4,
                KeyCode::Down | KeyCode::Right | KeyCode::Tab => self.focus = (self.focus + 1) % 4,
                KeyCode::Char(' ') if self.focus < 2 => {
                    self.enabled[self.focus] = !self.enabled[self.focus];
                    if self.enabled[self.focus] && self.choices[self.focus].is_none() {
                        self.choices[self.focus] = Some(recommended_model(self.focus));
                    }
                }
                KeyCode::Enter if self.focus == 3 => return Outcome::Cancel,
                KeyCode::Enter => {
                    if self.parents().is_empty() {
                        self.message = "Select Codex or Claude Code with Space.".into();
                    } else {
                        self.page = Page::Models;
                        self.focus = self.parents().len();
                    }
                }
                _ => {}
            },
            Page::Models => {
                let parents = self.parents();
                let count = parents.len();
                match key.code {
                    KeyCode::Esc => {
                        self.page = Page::Install;
                        self.focus = 2;
                    }
                    KeyCode::Up | KeyCode::Left | KeyCode::BackTab => {
                        self.focus = (self.focus + count + 2) % (count + 3)
                    }
                    KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                        self.focus = (self.focus + 1) % (count + 3)
                    }
                    KeyCode::Char('a') => {
                        self.focus = 0;
                        self.open_picker(parents[0]);
                    }
                    KeyCode::Char('e') if self.focus < count => {
                        let index = parents[self.focus];
                        self.open_effort(index);
                    }
                    KeyCode::Enter if self.focus < count => self.open_picker(parents[self.focus]),
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
                let count = candidates.len() + 1; // Explicit custom model entry is always reachable.
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
                    KeyCode::Enter if selected < candidates.len() => {
                        self.choose_model(candidates[selected].clone());
                        self.page = Page::Models;
                    }
                    KeyCode::Enter => {
                        self.open_custom();
                    }
                    KeyCode::Backspace => {
                        self.query.pop();
                        self.picker.select(Some(0));
                    }
                    KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::ALT) => {
                        self.query.push(character);
                        self.picker.select(Some(0));
                    }
                    _ => {}
                }
            }
            Page::Custom => match key.code {
                KeyCode::Esc => {
                    self.page = Page::Picker;
                    self.query.clear();
                    self.picker.select(Some(0));
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
                        self.custom[self.editing] = Some(CustomApiProfile {
                            base_url: base_url.into(),
                            api_key: if self.custom_key_touched || !self.custom_fields[2].is_empty()
                            {
                                (!self.custom_fields[2].is_empty())
                                    .then(|| self.custom_fields[2].clone())
                            } else {
                                self.custom[self.editing]
                                    .as_ref()
                                    .and_then(|profile| profile.api_key.clone())
                            },
                        });
                        self.choose_model(ModelChoice {
                            provider: "openai-compatible".into(),
                            id: model.into(),
                            label: format!("Custom API — {model}"),
                        });
                        self.page = Page::Models;
                    }
                }
                KeyCode::Backspace => {
                    if self.custom_field == 2 && !self.custom_key_touched {
                        self.custom_key_touched = true;
                    }
                    self.custom_fields[self.custom_field].pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::ALT) => {
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

    fn highlight(&self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED);
        if self.no_color {
            style
        } else {
            style.fg(Color::Cyan)
        }
    }

    fn model_text(&self, model: &ModelChoice) -> String {
        let known = self
            .catalog
            .models
            .iter()
            .any(|candidate| same_model(candidate, model));
        let recommended = matches!(
            (model.provider.as_str(), model.id.as_str()),
            ("codex", "gpt-5.6-luna") | ("claude", "sonnet")
        );
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
                Paragraph::new("Enlarge to 32 x 10 to configure.\nCtrl+C cancels.")
                    .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        let tall = area.height >= 22;
        let compact = area.height < 16;
        let parts = Layout::vertical([
            Constraint::Length(if tall { 3 } else { 1 }),
            Constraint::Length(if compact { 1 } else { 2 }),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .split(area);
        let title = if tall {
            "  ))) RepoTracer\n      Setup & settings"
        } else {
            "  ))) RepoTracer"
        };
        frame.render_widget(
            Paragraph::new(title).style(Style::default().add_modifier(Modifier::BOLD)),
            parts[0],
        );
        let step = if self.page == Page::Install {
            "1 / 2   Install in"
        } else {
            "2 / 2   Scout models"
        };
        frame.render_widget(
            Paragraph::new(step).block(Block::default().borders(Borders::BOTTOM)),
            parts[1],
        );
        let help = match self.page {
            Page::Install => "Space select  Enter next  Esc cancel",
            Page::Models => "Enter select  E effort  A advanced  Ctrl+S save",
            Page::Picker => "Type to search  Enter apply  Esc back",
            Page::Custom => "Tab fields  F2 discover  Enter next/save  Esc back",
            Page::Effort => "Enter apply  Esc back  Tab / arrows move",
        };
        let footer = if self.message.is_empty() {
            format!("{help}\nTab / arrows move  Ctrl+C cancel")
        } else {
            format!("{}\n{help}", self.message)
        };
        frame.render_widget(Paragraph::new(footer), parts[4]);
        let (actions, offset): (&[&str], usize) = match self.page {
            Page::Install => (&["Continue", "Cancel"], 2),
            Page::Models => (&["Save", "Back", "Cancel"], self.parents().len()),
            _ => (&[], 0),
        };
        let buttons: Vec<Span> = actions
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let style = if self.focus == offset + index {
                    self.highlight()
                } else {
                    Style::default()
                };
                Span::styled(format!(" [ {label} ] "), style)
            })
            .collect();
        frame.render_widget(Paragraph::new(Line::from(buttons)), parts[3]);
        if self.page == Page::Install {
            let rows: Vec<ListItem> = (0..2)
                .map(|index| {
                    ListItem::new(format!(
                        "[{}] {}",
                        if self.enabled[index] { "x" } else { " " },
                        LABELS[index]
                    ))
                })
                .collect();
            let body = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(if self.installed.iter().any(|value| *value) && tall {
                    2
                } else {
                    0
                }),
            ])
            .split(parts[2]);
            let mut state =
                ListState::default().with_selected((self.focus < 2).then_some(self.focus));
            frame.render_stateful_widget(
                List::new(rows)
                    .highlight_style(self.highlight())
                    .highlight_symbol("> "),
                body[0],
                &mut state,
            );
            frame.render_widget(
                Paragraph::new("Unchecked installations stay unchanged.")
                    .wrap(Wrap { trim: false }),
                body[1],
            );
            return;
        }
        let parents = self.parents();
        let editing = matches!(self.page, Page::Picker | Page::Custom | Page::Effort);
        let body = Layout::vertical([
            Constraint::Length(parents.len() as u16 * if compact { 1 } else { 2 }),
            Constraint::Min(1),
        ])
        .split(parts[2]);
        let rows: Vec<ListItem> = parents
            .iter()
            .map(|index| {
                let model = self.choices[*index]
                    .as_ref()
                    .map(|model| {
                        let effort = self.selected_efforts[*index].as_deref().unwrap_or("Auto");
                        let effort = format!(" · effort {effort}");
                        format!("{}{effort}", self.model_text(model))
                    })
                    .unwrap_or_else(|| "Choose a model".into());
                if compact {
                    ListItem::new(format!("{} > {model}", LABELS[*index]))
                } else {
                    ListItem::new(vec![
                        Line::from(format!("{} scout", LABELS[*index])),
                        Line::from(format!("  {model}")),
                    ])
                }
            })
            .collect();
        let selected = if editing {
            parents
                .iter()
                .position(|index| *index == self.editing)
                .unwrap_or(0)
        } else {
            self.focus
        };
        let mut state =
            ListState::default().with_selected((selected < parents.len()).then_some(selected));
        frame.render_stateful_widget(
            List::new(rows)
                .highlight_style(self.highlight())
                .highlight_symbol("> "),
            body[0],
            &mut state,
        );
        if self.page == Page::Models {
            let note = if self.loading {
                "Checking subscriptions...".into()
            } else if self.catalog.warnings.is_empty() {
                "Select a scout above to change it.".into()
            } else {
                self.catalog.warnings.join("\n")
            };
            frame.render_widget(Paragraph::new(note).wrap(Wrap { trim: false }), body[1]);
        } else if self.page == Page::Effort {
            let rows = self
                .effort_options()
                .into_iter()
                .map(|effort| {
                    ListItem::new(effort.unwrap_or_else(|| "Auto (agent-selected)".into()))
                })
                .collect::<Vec<_>>();
            let effort_body =
                Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(body[1]);
            frame.render_widget(
                Paragraph::new("Auto lets the agent choose. Manual overrides are optional.")
                    .wrap(Wrap { trim: false }),
                effort_body[0],
            );
            frame.render_stateful_widget(
                List::new(rows)
                    .highlight_style(self.highlight())
                    .highlight_symbol("> "),
                effort_body[1],
                &mut self.picker,
            );
        } else {
            let picker = Layout::vertical([
                Constraint::Length(if compact { 1 } else { 2 }),
                Constraint::Min(1),
            ])
            .split(body[1]);
            let label = if self.page == Page::Custom {
                "Custom OpenAI-compatible API"
            } else {
                "Search models"
            };
            // Keep the typed end visible, including on narrow terminals.
            let capacity = picker[0].width.saturating_sub(2) as usize;
            let input: String = self
                .query
                .chars()
                .rev()
                .take(capacity)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let search = if self.page == Page::Custom {
                label.to_owned()
            } else if compact {
                format!("/ {input}_")
            } else {
                format!("{label}\n{input}_")
            };
            frame.render_widget(Paragraph::new(search), picker[0]);
            if self.page == Page::Custom {
                let fields = ["Base URL", "Model ID", "API key (optional)"];
                let lines = fields
                    .iter()
                    .enumerate()
                    .map(|(index, label)| {
                        let value = if index == 2
                            && (!self.custom_fields[index].is_empty()
                                || self.custom[self.editing]
                                    .as_ref()
                                    .and_then(|profile| profile.api_key.as_ref())
                                    .is_some())
                        {
                            "••••••••".to_owned()
                        } else {
                            self.custom_fields[index].clone()
                        };
                        let marker = if self.custom_field == index { ">" } else { " " };
                        Line::from(format!("{marker} {label}: {value}"))
                    })
                    .chain(std::iter::once(Line::from(if self.custom_loading {
                        "Discovering /models…".to_owned()
                    } else {
                        "F2 discovers GET /models; API key is never displayed.".to_owned()
                    })))
                    .collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), picker[1]);
            } else {
                let mut rows: Vec<ListItem> = self
                    .candidates()
                    .iter()
                    .map(|model| ListItem::new(self.model_text(model)))
                    .collect();
                rows.push(ListItem::new("Custom model..."));
                let highlight = self.highlight();
                let block = if compact {
                    Block::default()
                } else {
                    Block::default().title(if self.loading {
                        "Checking subscriptions..."
                    } else {
                        "Available models"
                    })
                };
                frame.render_stateful_widget(
                    List::new(rows)
                        .block(block)
                        .highlight_style(highlight)
                        .highlight_symbol("> "),
                    picker[1],
                    &mut self.picker,
                );
            }
        }
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
        if app.custom_loading {
            if let Some(receiver) = &app.custom_receiver {
                match receiver.try_recv() {
                    Ok(Ok((models, efforts))) => {
                        app.catalog.models.extend(models);
                        app.catalog.reasoning_efforts.extend(efforts);
                        app.custom_loading = false;
                        app.custom_receiver = None;
                        app.loading = false;
                        app.page = Page::Picker;
                        app.query.clear();
                        app.picker.select(Some(0));
                    }
                    Ok(Err(error)) => {
                        app.custom_loading = false;
                        app.custom_receiver = None;
                        app.message = format!("Model discovery failed: {error}");
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        app.custom_loading = false;
                        app.custom_receiver = None;
                        app.message = "Model discovery stopped unexpectedly.".into();
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
        }
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
        assert_eq!(selection.0[0].model.id, "gpt-5.6-luna");
        assert_eq!(selection.0[1].model.id, "sonnet");
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
        assert_eq!(app.choices[1].as_ref().unwrap().id, "sonnet");
        assert_eq!(app.selected_efforts, [None, None]);
        key(&mut app, KeyCode::Enter);
        assert!(screen(&mut app, 80, 24).contains("availability pending"));
        let Outcome::Save(selection) = app.save() else {
            panic!("the seeded defaults should be saveable while discovery is pending")
        };
        assert_eq!(selection.0.len(), 2);
        assert!(selection
            .0
            .iter()
            .all(|entry| entry.reasoning_effort.is_none()));
    }

    #[test]
    fn auto_is_available_before_discovery_and_preserves_unknown_manual_effort() {
        let mut app = App::new(&["codex".into()], &[]);
        app.selected_efforts[0] = Some("max".into());
        app.page = Page::Models;
        app.focus = 0;
        key(&mut app, KeyCode::Char('e'));
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
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Char('e'));
        assert_eq!(app.page, Page::Effort);
        assert!(screen(&mut app, 80, 24).contains("Auto (agent-selected)"));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_efforts[0], None);
        key(&mut app, KeyCode::Char('e'));
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_efforts[0].as_deref(), Some("medium"));
    }

    #[test]
    fn missing_recommended_model_is_not_replaced_by_arbitrary_model() {
        let mut app = App::new(&["claude".into()], &[]);
        app.set_catalog(Catalog {
            models: vec![model("claude", "haiku")],
            ..Catalog::default()
        });
        assert_eq!(app.choices[1].as_ref().unwrap().id, "sonnet");
        let Outcome::Save(selection) = app.save() else {
            panic!("the unverified recommendation should remain saveable")
        };
        assert_eq!(selection.0[0].model.id, "sonnet");
        assert!(app
            .model_text(app.choices[1].as_ref().unwrap())
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
        assert!(app.model_text(&saved).contains("unverified"));
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
        assert_eq!(selection.0.len(), 1);
        assert_eq!(selection.0[0].model.provider, "claude");
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
        };
        let mut app = App::new_with_profiles(&["codex".into()], &[profile]);
        app.set_catalog(Catalog::default());
        app.open_custom();
        assert!(app.custom_fields[2].is_empty());
        assert!(!screen(&mut app, 80, 24).contains("secret"));
        app.custom_field = 2;
        key(&mut app, KeyCode::Char('n'));
        assert_eq!(app.custom_fields[2], "n");
        assert!(app.custom_key_touched);
    }

    #[test]
    fn no_installation_selected_cannot_continue_or_save() {
        let mut app = App::new(&[], &[]);
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
        type_text(&mut app, "opus");
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.focus, 1);
        assert_eq!(app.choices[1].as_ref().unwrap().id, "sonnet");
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
        assert!(screen(&mut app, 80, 24).contains("> [x] Codex"));
    }

    #[test]
    fn picker_scrolls_to_focused_model() {
        let mut app = app();
        app.catalog.models = (0..50)
            .map(|index| model("codex", &format!("model-{index:02}")))
            .collect();
        app.open_picker(0);
        app.picker.select(Some(49));
        assert!(screen(&mut app, 60, 18).contains("> codex:model-49"));
    }

    #[test]
    fn bottom_buttons_support_horizontal_navigation() {
        let mut app = app();
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.page, Page::Install);
        key(&mut app, KeyCode::Left);
        assert_eq!(app.focus, 1);
    }

    #[test]
    fn picker_keeps_focused_result_visible_at_minimum_size() {
        let mut app = app();
        app.open_picker(0);
        app.picker.select(Some(1));
        assert!(screen(&mut app, 32, 10).contains("> claude:sonnet"));
    }
}
