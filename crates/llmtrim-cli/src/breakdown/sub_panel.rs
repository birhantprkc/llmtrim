//! Status TUI **Sub** tab — mode (off / always / fallback) + Claude-tier → CLIProxyAPI map.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph, Row, Table};

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
            RoutingPreset::Always => "Always → CLIProxyAPI",
            RoutingPreset::Fallback => "Fallback",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Presets,
    Map,
}

pub struct SubPanel {
    focus: Focus,
    selected: RoutingPreset,
    applied: RoutingPreset,
    chain: Vec<String>,
    tiers: [Tier; 4],
    chosen: [String; 4],
    catalog: Vec<OfficialModel>,
    search: String,
    filtered: Vec<OfficialModel>,
    filter_idx: usize,
    tier_row: usize,
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
            tiers: Tier::ALL,
            chosen: [String::new(), String::new(), String::new(), String::new()],
            catalog: Vec::new(),
            search: String::new(),
            filtered: Vec::new(),
            filter_idx: 0,
            tier_row: 0,
            map_dirty: false,
            running: false,
            status: String::new(),
            needs_apply: false,
        };
        panel.reload_catalog();
        panel.reload_map();
        panel
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
        self.status = "highlighted Always — Enter to apply · e to edit opus/sonnet/haiku/fable map"
            .into();
    }

    pub fn seed_export_demo(&mut self) {
        self.selected = RoutingPreset::Always;
        self.applied = RoutingPreset::Always;
        self.running = true;
        self.chosen = [
            "grok-4.6".into(),
            "grok-4.6".into(),
            "grok-4.6".into(),
            "grok-composer-2.5-fast".into(),
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
        for (i, t) in self.tiers.iter().enumerate() {
            self.chosen[i] = overrides.get(t.as_str()).cloned().unwrap_or_default();
        }
        self.map_dirty = false;
        self.refilter();
    }

    fn refilter(&mut self) {
        self.filtered = cliproxy::search_official(&self.catalog, &self.search);
        if self.filter_idx >= self.filtered.len() {
            self.filter_idx = self.filtered.len().saturating_sub(1);
        }
        let current = self.chosen[self.tier_row].as_str();
        if let Some(i) = self.filtered.iter().position(|m| m.id == current) {
            self.filter_idx = i;
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
                    self.refilter();
                    self.status = "type to search official CLIProxyAPI models · ←→ pick · w save"
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
                    if self.map_dirty {
                        self.reload_map();
                        self.status = "unsaved map changes discarded".into();
                    }
                    self.search.clear();
                    self.focus = Focus::Presets;
                }
                KeyCode::Up | KeyCode::Char('k') if self.search.is_empty() => {
                    if self.tier_row > 0 {
                        self.tier_row -= 1;
                        self.refilter();
                    }
                }
                KeyCode::Down | KeyCode::Char('j') if self.search.is_empty() => {
                    if self.tier_row + 1 < self.tiers.len() {
                        self.tier_row += 1;
                        self.refilter();
                    }
                }
                KeyCode::Left => self.cycle_model(-1),
                KeyCode::Right => self.cycle_model(1),
                KeyCode::Char('w') => self.save_map(),
                KeyCode::Backspace => {
                    self.search.pop();
                    self.refilter();
                }
                KeyCode::Char(c) if !c.is_control() => {
                    self.search.push(c);
                    self.refilter();
                    if let Some(m) = self.filtered.first() {
                        self.filter_idx = 0;
                        self.chosen[self.tier_row] = m.id.clone();
                        self.map_dirty = true;
                    }
                }
                _ => return false,
            },
        }
        true
    }

    fn cycle_preset(&mut self, dir: i32) {
        let list = RoutingPreset::ALL;
        let pos = list.iter().position(|p| *p == self.selected).unwrap_or(0) as i32;
        let next = (pos + dir).rem_euclid(list.len() as i32) as usize;
        self.selected = list[next];
        self.status.clear();
    }

    fn cycle_model(&mut self, dir: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let n = self.filtered.len() as i32;
        self.filter_idx = (self.filter_idx as i32 + dir).rem_euclid(n) as usize;
        self.chosen[self.tier_row] = self.filtered[self.filter_idx].id.clone();
        self.map_dirty = true;
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
        self.status = format!("chain {}", self.chain.join(" → "));
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
                Ok("always → CLIProxyAPI (tier map still applies)".into())
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
                Ok(format!("fallback · {}", chain.join(" → ")))
            }
        }
    }

    fn save_map(&mut self) {
        let mut map = BTreeMap::new();
        for (t, m) in self.tiers.iter().zip(self.chosen.iter()) {
            if !m.is_empty() {
                map.insert(t.as_str().to_string(), m.clone());
            }
        }
        match llmtrim_core::config::write_sub_tiers("on", &map) {
            Ok(()) => {
                self.map_dirty = false;
                self.needs_apply = true;
                self.status = "tier map saved".into();
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    pub fn help_keys(&self) -> &'static str {
        match self.focus {
            Focus::Presets => " Tab tabs · ←→ mode · [ ] chain · ⏎ apply · e map · r refresh · q",
            Focus::Map => " ↑↓ tier · type search · ←→ model · w save · Esc back · q",
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

        let chunks = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(8),
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
        let sidecar = if self.running { "sidecar up" } else { "sidecar down" };
        let header = Paragraph::new(vec![
            Line::from(presets),
            Line::from(format!(
                "{sidecar} · {} official models · * = applied",
                self.catalog.len()
            )),
            Line::from(if self.selected == RoutingPreset::Fallback {
                format!("chain {}", if self.chain.is_empty() { "anthropic → on".into() } else { self.chain.join(" → ") })
            } else {
                String::new()
            }),
        ]);
        f.render_widget(header, chunks[0]);

        let rows: Vec<Row> = self
            .tiers
            .iter()
            .zip(self.chosen.iter())
            .enumerate()
            .map(|(i, (t, m))| {
                let style = if self.focus == Focus::Map && i == self.tier_row {
                    Style::default()
                        .fg(palette::accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let label = if m.is_empty() {
                    "(pass through)".to_string()
                } else {
                    m.clone()
                };
                Row::new(vec![t.as_str().to_string(), label]).style(style)
            })
            .collect();
        let table = Table::new(rows, [Constraint::Length(10), Constraint::Min(20)]).header(
            Row::new(vec!["claude", "CLIProxyAPI model"])
                .style(Style::default().add_modifier(Modifier::BOLD)),
        );
        f.render_widget(table, chunks[1]);

        let hint = if self.focus == Focus::Map {
            let n = self.filtered.len();
            format!(
                "search: {}  ·  {n} hits  ·  {}",
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
        assert_eq!(p.focus, Focus::Presets);
    }
}
