//! Status TUI **Sub** tab — CLIProxyAPI sidecar + available models.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph, Row, Table};

use super::palette;
use crate::reroute::SubProvider;
use crate::reroute::cliproxy::{self, Model};

/// Live state for the Sub tab.
pub struct SubPanel {
    enabled: bool,
    running: bool,
    version: Option<String>,
    url: String,
    models: Vec<Model>,
    selected: usize,
    status: String,
    /// True when a config write needs daemon restart + Claude auth sync after the TUI exits.
    pub needs_apply: bool,
}

impl Default for SubPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl SubPanel {
    pub fn new() -> Self {
        let mut panel = Self {
            enabled: false,
            running: false,
            version: None,
            url: cliproxy::default_base_url(),
            models: Vec::new(),
            selected: 0,
            status: String::new(),
            needs_apply: false,
        };
        panel.refresh();
        panel
    }

    pub fn refresh(&mut self) {
        let st = cliproxy::status();
        self.enabled = st.enabled;
        self.running = st.running;
        self.version = st.version;
        self.url = st.base_url;
        self.models = cliproxy::list_models().unwrap_or_default();
        let n = self.row_count();
        if self.selected >= n {
            self.selected = n.saturating_sub(1);
        }
    }

    pub fn preselect_provider(&mut self, _provider: SubProvider) {
        self.status = "Enter pins a CLIProxyAPI model · x turns reroute off".into();
    }

    /// Stable demo state for the README SVG export (not written to disk).
    pub fn seed_export_demo(&mut self) {
        self.enabled = true;
        self.running = true;
        self.version = Some("7.2.130".into());
        self.url = cliproxy::default_base_url();
        self.models = vec![
            Model {
                id: "gpt-5.4".into(),
                owned_by: "openai".into(),
            },
            Model {
                id: "gemini-3-flash".into(),
                owned_by: "google".into(),
            },
        ];
        self.status = "demo".into();
    }

    fn rows(&self) -> Vec<(String, String)> {
        if !self.models.is_empty() {
            return self
                .models
                .iter()
                .map(|m| (m.id.clone(), m.owned_by.clone()))
                .collect();
        }
        cliproxy::BACKENDS
            .iter()
            .map(|b| (b.id.to_string(), b.aliases.join(", ")))
            .collect()
    }

    fn row_count(&self) -> usize {
        self.rows().len()
    }

    pub fn handle_key(&mut self, code: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let n = self.row_count();
                if n > 0 && self.selected + 1 < n {
                    self.selected += 1;
                }
                true
            }
            KeyCode::Enter => {
                self.pin_selected();
                true
            }
            KeyCode::Char('x') | KeyCode::Backspace => {
                self.turn_off();
                true
            }
            KeyCode::Char('r') => {
                self.refresh();
                self.status = "refreshed".into();
                true
            }
            _ => false,
        }
    }

    fn pin_selected(&mut self) {
        let rows = self.rows();
        let Some((id, _)) = rows.get(self.selected) else {
            self.turn_on();
            return;
        };
        let id = id.clone();
        if let Err(e) = cliproxy::ensure_running() {
            self.status = format!("CLIProxyAPI: {e:#}");
            return;
        }
        if let Err(e) = llmtrim_core::config::enable_sub("on") {
            self.status = format!("could not enable: {e:#}");
            return;
        }
        if let Err(e) = llmtrim_core::config::write_sub_model(Some(&id)) {
            self.status = format!("could not pin {id}: {e:#}");
            return;
        }
        self.enabled = true;
        self.running = cliproxy::is_running();
        self.needs_apply = true;
        self.status = format!("pinned {id}");
        self.refresh();
    }

    fn turn_on(&mut self) {
        if let Err(e) = cliproxy::ensure_running() {
            self.status = format!("CLIProxyAPI: {e:#}");
            return;
        }
        if let Err(e) = llmtrim_core::config::enable_sub("on") {
            self.status = format!("could not enable: {e:#}");
            return;
        }
        let _ = llmtrim_core::config::write_sub_model(None);
        self.enabled = true;
        self.running = cliproxy::is_running();
        self.needs_apply = true;
        self.status = "reroute on via CLIProxyAPI".into();
        self.refresh();
    }

    fn turn_off(&mut self) {
        if let Err(e) = llmtrim_core::config::disable_sub() {
            self.status = format!("could not disable: {e:#}");
            return;
        }
        let _ = llmtrim_core::config::write_sub_model(None);
        let _ = cliproxy::stop();
        self.enabled = false;
        self.needs_apply = true;
        self.status = "reroute off".into();
        self.refresh();
    }

    pub fn help_keys(&self) -> &'static str {
        " Tab tabs · ↑↓ models · ⏎ pin · x off · r refresh · t theme · q"
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
            Constraint::Length(5),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(inner);

        let state = if self.enabled && self.running {
            "on · running"
        } else if self.enabled {
            "on · sidecar down"
        } else {
            "off"
        };
        let ver = self
            .version
            .as_deref()
            .map(|v| format!(" v{v}"))
            .unwrap_or_default();
        let header = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("CLIProxyAPI ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!("{state}{ver}")),
            ]),
            Line::from(self.url.as_str()),
            Line::from("Enter pins a model/CLI. x off. Sign in: llmtrim sub auth"),
        ]);
        f.render_widget(header, chunks[0]);

        let rows = self.rows();
        if rows.is_empty() {
            f.render_widget(
                Paragraph::new("Sidecar not running — Enter to start, or `llmtrim sub on`."),
                chunks[1],
            );
        } else {
            let table_rows: Vec<Row> = rows
                .iter()
                .enumerate()
                .map(|(i, (id, owner))| {
                    let style = if i == self.selected {
                        Style::default()
                            .fg(palette::accent())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    Row::new(vec![id.clone(), owner.clone()]).style(style)
                })
                .collect();
            let col2 = if self.models.is_empty() {
                "cli aliases"
            } else {
                "owned_by"
            };
            let table = Table::new(
                table_rows,
                [Constraint::Percentage(65), Constraint::Percentage(35)],
            )
            .header(
                Row::new(vec!["model / cli", col2])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            );
            f.render_widget(table, chunks[1]);
        }

        if !self.status.is_empty() {
            f.render_widget(Paragraph::new(self.status.as_str()), chunks[2]);
        }
    }
}

/// Reconcile Claude dummy-auth + restart the interceptor so a Sub-tab write takes effect.
/// Best-effort: never panics; returns a short status string for the caller to print after exit.
///
/// Mirrors `main::apply_sub_change`: always sync auth env; only restart when a daemon is
/// already running (so a pure-config edit never spawns a new interceptor by surprise).
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
        assert_eq!(p.selected, 0);
    }
}
