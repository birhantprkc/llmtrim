//! Status TUI **Sub** tab — mode (off / always / fallback) + Claude → CLIProxyAPI map.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListItem, ListState, Padding, Paragraph, Row, Table,
};

use super::palette;
use crate::reroute::cliproxy::{self, OfficialModel};
use crate::reroute::{SubProvider, Tier};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutingPreset {
    Off,
    Always,
    Fallback,
}

impl RoutingPreset {
    pub const ALL: [RoutingPreset; 3] = [
        RoutingPreset::Off,
        RoutingPreset::Always,
        RoutingPreset::Fallback,
    ];

    fn label(self) -> &'static str {
        match self {
            RoutingPreset::Off => "Off",
            RoutingPreset::Always => "Always",
            RoutingPreset::Fallback => "Fallback",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Presets,
    Map,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Col {
    From,
    To,
}

fn hop_display(hop: &str) -> &str {
    match hop {
        "anthropic" | "direct" => "what's in use",
        "on" | "cliproxy" | "cli-proxy" | "cli-proxy-api" | "cliproxyapi" => "mapped models",
        "codex" | "chatgpt" | "openai" => "Codex",
        "claude" => "Claude",
        "gemini" | "antigravity" | "aistudio" => "Gemini",
        "grok" | "xai" | "x-ai" => "Grok",
        "kimi" | "moonshot" => "Kimi",
        "vertex" => "Vertex",
        "qwen" => "Qwen",
        "copilot" | "github" => "Copilot",
        other => other,
    }
}

fn format_try_chain(hops: &[String]) -> String {
    let names: Vec<&str> = hops.iter().map(|h| hop_display(h)).collect();
    match names.as_slice() {
        [] => "Try the next hop on failure".into(),
        [one] => format!("Try {one}"),
        [first, rest @ ..] => format!("Try {first}, then {}", rest.join(", then ")),
    }
}

fn side_label(col: Col) -> &'static str {
    match col {
        Col::From => "input",
        Col::To => "output",
    }
}

pub struct SubPanel {
    focus: Focus,
    selected: RoutingPreset,
    applied: RoutingPreset,
    chain: Vec<String>,
    rows: Vec<(String, String)>,
    row: usize,
    col: Col,
    catalog: Vec<OfficialModel>,
    search: String,
    filtered: Vec<String>,
    filter_idx: usize,
    map_dirty: bool,
    running: bool,
    status: String,
    pub needs_apply: bool,
}

impl Default for SubPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl SubPanel {
    pub fn new() -> Self {
        let applied = Self::read_applied_preset();
        let mut panel = Self {
            focus: Focus::Presets,
            selected: applied,
            applied,
            chain: Self::read_chain(),
            rows: Vec::new(),
            row: 0,
            col: Col::To,
            catalog: Vec::new(),
            search: String::new(),
            filtered: Vec::new(),
            filter_idx: 0,
            map_dirty: false,
            running: false,
            status: String::new(),
            needs_apply: false,
        };
        panel.reload_catalog();
        panel.reload_map();
        panel
    }

    pub fn capturing_keys(&self) -> bool {
        self.focus == Focus::Map
    }

    pub fn refresh(&mut self) {
        let applied = Self::read_applied_preset();
        if !self.needs_apply {
            self.selected = applied;
        }
        self.applied = applied;
        self.chain = Self::read_chain();
        self.running = cliproxy::is_running();
        if self.focus == Focus::Presets && !self.map_dirty {
            self.reload_catalog();
            self.reload_map();
        }
    }

    pub fn preselect_provider(&mut self, _provider: SubProvider) {
        self.selected = RoutingPreset::Always;
        self.status = "highlighted Always — Enter to apply · e to edit map".into();
    }

    pub fn seed_export_demo(&mut self) {
        self.selected = RoutingPreset::Always;
        self.applied = RoutingPreset::Always;
        self.running = true;
        self.rows = vec![
            ("fable".into(), "grok-4.6".into()),
            ("opus".into(), "grok-4.6".into()),
            ("sonnet".into(), "grok-4.6".into()),
            ("haiku".into(), "grok-composer-2.5-fast".into()),
        ];
        self.status = "demo".into();
    }

    fn read_applied_preset() -> RoutingPreset {
        let file = load_config_file();
        let env = |k: &str| std::env::var(k).ok();
        let Some(_active) = resolve_active(&env, file.as_ref()) else {
            return RoutingPreset::Off;
        };
        if resolve_fallback(&env, file.as_ref()) {
            RoutingPreset::Fallback
        } else {
            RoutingPreset::Always
        }
    }

    fn reload_catalog(&mut self) {
        self.catalog = cliproxy::official_models();
        self.refilter();
    }

    fn reload_map(&mut self) {
        let overrides = llmtrim_core::config::sub_tiers_for("on");
        let mut rows: Vec<(String, String)> = overrides.into_iter().collect();
        if rows.is_empty() {
            rows = Tier::ALL
                .iter()
                .map(|t| (t.as_str().to_string(), String::new()))
                .collect();
        }
        self.rows = rows;
        if self.row >= self.rows.len() {
            self.row = self.rows.len().saturating_sub(1);
        }
        self.map_dirty = false;
        self.search.clear();
        self.refilter();
    }

    fn input_suggestions(&self) -> Vec<String> {
        let mut out: Vec<String> = Tier::ALL.iter().map(|t| t.as_str().to_string()).collect();
        for m in &self.catalog {
            if !out.iter().any(|x| x == &m.id) {
                out.push(m.id.clone());
            }
        }
        out
    }

    fn refilter(&mut self) {
        let q = self.search.trim().to_ascii_lowercase();
        let pool = match self.col {
            Col::From => self.input_suggestions(),
            Col::To => self
                .catalog
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>(),
        };
        self.filtered = if q.is_empty() {
            pool
        } else {
            pool.into_iter()
                .filter(|id| {
                    id.to_ascii_lowercase().contains(&q)
                        || self.catalog.iter().any(|m| {
                            m.id == *id
                                && (m.display_name.to_ascii_lowercase().contains(&q)
                                    || m.family.to_ascii_lowercase().contains(&q)
                                    || m.owned_by.to_ascii_lowercase().contains(&q))
                        })
                })
                .collect()
        };
        let current = self.rows.get(self.row).map(|(f, t)| match self.col {
            Col::From => f.as_str(),
            Col::To => t.as_str(),
        });
        self.filter_idx = current
            .and_then(|c| self.filtered.iter().position(|id| id == c))
            .unwrap_or(0);
        if self.filter_idx >= self.filtered.len() {
            self.filter_idx = self.filtered.len().saturating_sub(1);
        }
    }

    pub fn handle_key(&mut self, code: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;
        match self.focus {
            Focus::Presets => match code {
                KeyCode::Left | KeyCode::Char('h') => self.cycle_preset(-1),
                KeyCode::Right | KeyCode::Char('l') => self.cycle_preset(1),
                KeyCode::Char('[') => self.rotate_chain(-1),
                KeyCode::Char(']') => self.rotate_chain(1),
                KeyCode::Enter => self.apply_selected(),
                KeyCode::Char('e') => {
                    self.focus = Focus::Map;
                    self.col = Col::To;
                    self.search.clear();
                    self.refilter();
                    self.status =
                        "← from  → to  ·  type to search  ·  s save  ·  a add  ·  d del  ·  Esc"
                            .into();
                }
                KeyCode::Char('r') => {
                    self.reload_catalog();
                    self.refresh();
                    self.status = "refreshed official model list".into();
                }
                _ => return false,
            },
            Focus::Map => match code {
                KeyCode::Esc => {
                    if !self.search.is_empty() {
                        self.search.clear();
                        self.refilter();
                    } else if self.map_dirty {
                        self.reload_map();
                        self.status = "unsaved map changes discarded".into();
                        self.focus = Focus::Presets;
                    } else {
                        self.focus = Focus::Presets;
                    }
                }
                KeyCode::Left => {
                    self.col = Col::From;
                    self.search.clear();
                    self.refilter();
                    self.status = "editing input (incoming model or tier)".into();
                }
                KeyCode::Right => {
                    self.col = Col::To;
                    self.search.clear();
                    self.refilter();
                    self.status = "editing output (mapped model)".into();
                }
                KeyCode::Up => self.move_pick(-1),
                KeyCode::Down => self.move_pick(1),
                KeyCode::Tab => self.move_row(1),
                KeyCode::BackTab => self.move_row(-1),
                KeyCode::Char('a') | KeyCode::Char('+') | KeyCode::Insert
                    if self.search.is_empty() =>
                {
                    self.add_row();
                }
                KeyCode::Char('d') | KeyCode::Char('-') | KeyCode::Delete
                    if self.search.is_empty() =>
                {
                    self.remove_row();
                }
                KeyCode::Char('s') if self.search.is_empty() => self.save_map(),
                KeyCode::Char('w') if self.search.is_empty() => self.save_map(),
                KeyCode::Backspace => {
                    self.search.pop();
                    self.apply_search_to_cell();
                }
                KeyCode::Enter => {
                    if let Some(id) = self.filtered.get(self.filter_idx).cloned() {
                        self.set_cell(id);
                        self.search.clear();
                        self.refilter();
                    }
                }
                KeyCode::Char(c) if !c.is_control() => {
                    self.search.push(c);
                    self.apply_search_to_cell();
                }
                _ => return false,
            },
        }
        true
    }

    fn move_row(&mut self, dir: i32) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len() as i32;
        self.row = (self.row as i32 + dir).rem_euclid(n) as usize;
        self.search.clear();
        self.refilter();
    }

    fn add_row(&mut self) {
        self.rows.push((String::new(), String::new()));
        self.row = self.rows.len() - 1;
        self.col = Col::From;
        self.search.clear();
        self.map_dirty = true;
        self.refilter();
        self.status = "new row — type the input model".into();
    }

    fn remove_row(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.rows.remove(self.row);
        if self.row >= self.rows.len() {
            self.row = self.rows.len().saturating_sub(1);
        }
        self.map_dirty = true;
        self.search.clear();
        self.refilter();
        self.status = "row removed".into();
    }

    fn set_cell(&mut self, value: String) {
        if let Some(row) = self.rows.get_mut(self.row) {
            match self.col {
                Col::From => row.0 = value,
                Col::To => row.1 = value,
            }
            self.map_dirty = true;
        }
    }

    fn apply_search_to_cell(&mut self) {
        self.refilter();
        self.filter_idx = 0;
        self.set_cell(self.search.clone());
    }

    fn move_pick(&mut self, dir: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let n = self.filtered.len() as i32;
        self.filter_idx = (self.filter_idx as i32 + dir).rem_euclid(n) as usize;
    }

    fn cycle_preset(&mut self, dir: i32) {
        let list = RoutingPreset::ALL;
        let pos = list.iter().position(|p| *p == self.selected).unwrap_or(0) as i32;
        let next = (pos + dir).rem_euclid(list.len() as i32) as usize;
        self.selected = list[next];
        self.status.clear();
    }

    fn read_chain() -> Vec<String> {
        if let Ok(v) = std::env::var("LLMTRIM_SUB_CHAIN") {
            return v.split(',').filter_map(cliproxy::parse_hop).collect();
        }
        load_config_file()
            .as_ref()
            .and_then(|v| v.get("sub"))
            .and_then(|v| v.get("chain"))
            .map(|v| match v {
                toml::Value::Array(items) => items
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .filter_map(cliproxy::parse_hop)
                    .collect(),
                toml::Value::String(s) => s.split(',').filter_map(cliproxy::parse_hop).collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default()
    }

    fn rotate_chain(&mut self, dir: i32) {
        if self.selected != RoutingPreset::Fallback {
            self.selected = RoutingPreset::Fallback;
        }
        if self.chain.is_empty() {
            self.chain = vec!["anthropic".into(), "on".into()];
        }
        let n = self.chain.len() as i32;
        if n == 0 {
            return;
        }
        let k = dir.rem_euclid(n) as usize;
        self.chain.rotate_left(k);
        self.status = format_try_chain(&self.chain);
    }

    fn apply_selected(&mut self) {
        match self.apply_preset(self.selected) {
            Ok(msg) => {
                self.applied = self.selected;
                self.needs_apply = true;
                self.reload_map();
                self.status = msg;
            }
            Err(e) => self.status = format!("apply failed: {e}"),
        }
    }

    fn apply_preset(&self, preset: RoutingPreset) -> anyhow::Result<String> {
        let _ = llmtrim_core::config::write_sub_model(None);
        match preset {
            RoutingPreset::Off => {
                llmtrim_core::config::disable_sub()?;
                Ok("reroute off — Anthropic only".into())
            }
            RoutingPreset::Always => {
                cliproxy::ensure_for_existing_user()?;
                llmtrim_core::config::enable_sub("on")?;
                llmtrim_core::config::write_sub_mode(false)?;
                Ok("always on — every turn uses the mapped models".into())
            }
            RoutingPreset::Fallback => {
                cliproxy::ensure_for_existing_user()?;
                llmtrim_core::config::enable_sub("on")?;
                llmtrim_core::config::write_sub_mode(true)?;
                let chain = if self.chain.is_empty() {
                    vec!["anthropic".into(), "on".into()]
                } else {
                    self.chain.clone()
                };
                llmtrim_core::config::write_sub_chain(&chain)?;
                Ok(format_try_chain(&chain))
            }
        }
    }

    fn save_map(&mut self) {
        let mut map = BTreeMap::new();
        for (from, to) in &self.rows {
            let from = from.trim();
            let to = to.trim();
            if !from.is_empty() && !to.is_empty() {
                map.insert(from.to_string(), to.to_string());
            }
        }
        match llmtrim_core::config::write_sub_tiers("on", &map) {
            Ok(()) => {
                self.map_dirty = false;
                self.needs_apply = true;
                self.search.clear();
                self.status = format!("saved {} mappings — quit the TUI to apply", map.len());
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    pub fn save_now(&mut self) {
        self.save_map();
    }

    pub fn help_keys(&self) -> &'static str {
        match self.focus {
            Focus::Presets => {
                " Tab tabs · ←→ mode · [ ] chain · Enter apply · e map · r refresh · q"
            }
            Focus::Map if self.map_dirty => {
                " ← from · → to · ↑↓ pick · Enter pick · s SAVE · a add · d del · Esc discard"
            }
            Focus::Map => " ← from · → to · ↑↓ pick · Enter pick · s save · a add · d del · Esc",
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let frame = Style::default().fg(palette::frame());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(frame)
            .title(" sub · CLIProxyAPI ")
            .title_style(frame.add_modifier(Modifier::BOLD))
            .padding(Padding::new(1, 1, 0, 0));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let suggest_h = if self.focus == Focus::Map { 8 } else { 0 };
        let chunks = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(suggest_h),
            Constraint::Length(2),
        ])
        .split(inner);

        let presets: Vec<Span> = RoutingPreset::ALL
            .iter()
            .flat_map(|p| {
                let on = *p == self.selected;
                let applied = *p == self.applied;
                let mut style = Style::default();
                if on {
                    style = style.fg(palette::accent()).add_modifier(Modifier::BOLD);
                }
                let mark = if applied { "*" } else { " " };
                [
                    Span::styled(format!("{mark}{} ", p.label()), style),
                    Span::raw(" "),
                ]
            })
            .collect();
        let sidecar = if self.running {
            "sidecar up"
        } else {
            "sidecar down"
        };
        let header = Paragraph::new(vec![
            Line::from(presets),
            Line::from(format!(
                "{sidecar} · {} official models · * = applied",
                self.catalog.len()
            )),
            Line::from(if self.selected == RoutingPreset::Fallback {
                let hops = if self.chain.is_empty() {
                    vec!["anthropic".into(), "on".into()]
                } else {
                    self.chain.clone()
                };
                format_try_chain(&hops)
            } else {
                String::new()
            }),
        ]);
        f.render_widget(header, chunks[0]);

        let rows: Vec<Row> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, (from, to))| {
                let active = self.focus == Focus::Map && i == self.row;
                let from_s = if from.is_empty() {
                    "…"
                } else {
                    from.as_str()
                };
                let to_s = if to.is_empty() {
                    "(pass through)"
                } else {
                    to.as_str()
                };
                let from_style = if active && self.col == Col::From {
                    Style::default()
                        .fg(palette::accent())
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else if active {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let to_style = if active && self.col == Col::To {
                    Style::default()
                        .fg(palette::accent())
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else if active {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Row::new(vec![
                    ratatui::widgets::Cell::from(from_s.to_string()).style(from_style),
                    ratatui::widgets::Cell::from(to_s.to_string()).style(to_style),
                ])
            })
            .collect();
        let table = Table::new(
            rows,
            [Constraint::Percentage(40), Constraint::Percentage(60)],
        )
        .header(
            Row::new(vec!["input", "output"]).style(Style::default().add_modifier(Modifier::BOLD)),
        );
        f.render_widget(table, chunks[1]);

        if self.focus == Focus::Map {
            let title = if self.search.is_empty() {
                format!("{} · {} models", side_label(self.col), self.filtered.len())
            } else {
                format!("\"{}\" · {} matches", self.search, self.filtered.len())
            };
            let items: Vec<ListItem> = if self.filtered.is_empty() {
                vec![ListItem::new("(no matches)")]
            } else {
                self.filtered
                    .iter()
                    .map(|id| ListItem::new(id.as_str()))
                    .collect()
            };
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(palette::frame()))
                        .title(title),
                )
                .highlight_style(
                    Style::default()
                        .fg(palette::accent())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                )
                .highlight_symbol("› ");
            let mut state = ListState::default();
            if !self.filtered.is_empty() {
                state.select(Some(self.filter_idx.min(self.filtered.len() - 1)));
            }
            f.render_stateful_widget(list, chunks[2], &mut state);
        }

        let hint = if self.focus == Focus::Map {
            let n = self.filtered.len();
            let side = match self.col {
                Col::From => "from",
                Col::To => "to",
            };
            format!(
                "{side} search: {}  ·  {n} hits  ·  {}",
                if self.search.is_empty() {
                    "_"
                } else {
                    self.search.as_str()
                },
                self.status
            )
        } else {
            self.status.clone()
        };
        f.render_widget(Paragraph::new(hint), chunks[2]);
    }
}

fn load_config_file() -> Option<toml::Value> {
    let path = std::env::var("LLMTRIM_CONFIG")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var("HOME")
                        .ok()
                        .map(|h| std::path::PathBuf::from(h).join(".config"))
                })
                .map(|b| b.join("llmtrim").join("config.toml"))
        })?;
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
}

fn resolve_active(
    env: &impl Fn(&str) -> Option<String>,
    file: Option<&toml::Value>,
) -> Option<String> {
    if let Some(v) = env("LLMTRIM_SUB").filter(|s| !s.is_empty()) {
        let s = v.trim().to_ascii_lowercase();
        return (s != "off").then_some(s);
    }
    let sub = file?.get("sub")?;
    let s = sub
        .as_str()
        .or_else(|| {
            sub.get("active")
                .or_else(|| sub.get("provider"))
                .and_then(toml::Value::as_str)
        })?
        .trim()
        .to_ascii_lowercase();
    (s != "off" && !s.is_empty()).then_some(s)
}

fn resolve_fallback(env: &impl Fn(&str) -> Option<String>, file: Option<&toml::Value>) -> bool {
    let parse = |raw: &str| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "fallback" | "on_error" | "on-error" | "onerror"
        )
    };
    if let Some(v) = env("LLMTRIM_SUB_MODE").filter(|s| !s.is_empty()) {
        return parse(&v);
    }
    file.and_then(|v| v.get("sub"))
        .and_then(|v| v.get("mode"))
        .and_then(toml::Value::as_str)
        .is_some_and(parse)
}

/// Reconcile Claude dummy-auth + restart the interceptor so a Sub-tab write takes effect.
pub fn apply_pending_changes() -> String {
    use crate::statusline::SubAuthEnvChange;
    let want = llmtrim_core::config::sub_skip_anthropic_login();
    let mut parts = Vec::new();
    match crate::statusline::sync_sub_auth_env(want) {
        Ok(SubAuthEnvChange::Injected) => parts.push(
            "Claude Code: dummy ANTHROPIC_AUTH_TOKEN set (connectors off; restart Claude Code)."
                .to_string(),
        ),
        Ok(SubAuthEnvChange::Removed) => parts.push(
            "Claude Code: dummy ANTHROPIC_AUTH_TOKEN removed (Anthropic login may be required)."
                .to_string(),
        ),
        Ok(SubAuthEnvChange::Unchanged) => {}
        Err(e) => parts.push(format!("Claude Code auth env update failed: {e:#}")),
    }
    if llmtrim_core::config::sub_always_on()
        && let Err(e) = cliproxy::ensure_for_existing_user()
    {
        parts.push(format!("CLIProxyAPI: {e:#}"));
    }
    let daemon_msg = match crate::daemon::running() {
        None => {
            "Subscription routing saved (no daemon running — next start picks it up).".to_string()
        }
        Some(state) => {
            let port = state.port;
            match crate::daemon::stop_and_wait_free(port) {
                Ok(true) => match crate::daemon::spawn_detached(port) {
                    Ok(pid) => {
                        format!("Subscription routing applied (restarted daemon pid {pid}).")
                    }
                    Err(e) => format!(
                        "Subscription config saved, but restart failed: {e:#}. Run `llmtrim start --force`."
                    ),
                },
                Ok(false) => {
                    "Subscription config saved, but the old daemon did not release the port. Run `llmtrim start --force`."
                        .to_string()
                }
                Err(e) => format!(
                    "Subscription config saved, but restart failed: {e:#}. Run `llmtrim start --force`."
                ),
            }
        }
    };
    parts.push(daemon_msg);
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_panel_starts_on_presets() {
        let p = SubPanel::new();
        assert!(!p.needs_apply);
        assert!(!p.capturing_keys());
    }

    #[test]
    fn try_chain_sentence_hides_internal_on() {
        assert_eq!(
            format_try_chain(&["anthropic".into(), "on".into()]),
            "Try what's in use, then mapped models"
        );
        assert_eq!(
            format_try_chain(&["codex".into(), "anthropic".into()]),
            "Try Codex, then what's in use"
        );
    }
}
