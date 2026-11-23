//! Drawing: a retro control panel in the NIRUX colours.
use super::app::{App, Mode, PickFor, StackRow, Tab, TABS};
use crate::engine;
use crate::model::{self, Health, StackState};
use crate::util;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::VecDeque;

const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn spark(hist: &VecDeque<f64>, width: usize) -> String {
    let take: Vec<f64> = hist.iter().rev().take(width).rev().copied().collect();
    let max = take.iter().copied().fold(5.0f64, f64::max);
    let mut s: String = take
        .iter()
        .map(|v| {
            if *v <= 0.05 {
                ' '
            } else {
                BARS[((v / max) * 7.0).round().clamp(0.0, 7.0) as usize]
            }
        })
        .collect();
    while s.chars().count() < width {
        s.insert(0, ' ');
    }
    s
}

/// A meter: ▕█████░░░░▏
fn meter(frac: f64, width: usize) -> String {
    let n = (frac.clamp(0.0, 1.0) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(n), "░".repeat(width - n))
}

/// "Up 4 weeks (healthy)" as "Up 4 weeks ♥".
fn short_status(s: &str) -> String {
    s.replace(" (healthy)", " ♥")
        .replace(" (unhealthy)", " ✕")
        .replace(" (health: starting)", " ♡")
}

/// An image without its digest.
fn short_image(s: &str) -> String {
    s.split('@').next().unwrap_or(s).to_string()
}

fn pad(s: &str, w: usize) -> String {
    let n = unicode_width::UnicodeWidthStr::width(s);
    if n >= w {
        let mut out = String::new();
        let mut used = 0;
        for c in s.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
            if used + cw + 1 > w {
                break;
            }
            out.push(c);
            used += cw;
        }
        if w > 0 {
            out.push('…');
            used += 1;
        }
        out + &" ".repeat(w.saturating_sub(used))
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}

fn rpad(s: &str, w: usize) -> String {
    let n = unicode_width::UnicodeWidthStr::width(s);
    if n >= w {
        pad(s, w)
    } else {
        format!("{}{s}", " ".repeat(w - n))
    }
}

impl App {
    fn st(&self) -> Style {
        Style::default().fg(self.pal.fg).bg(self.pal.bg)
    }

    fn c(&self, c: Color) -> Style {
        Style::default().fg(c)
    }

    fn panel<'a>(&self, title: &'a str, focused: bool) -> Block<'a> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(if focused {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .border_style(self.c(if focused {
                self.pal.accent
            } else {
                self.pal.border
            }))
            .title(Span::styled(
                format!(" {title} "),
                self.c(if focused {
                    self.pal.accent
                } else {
                    self.pal.muted
                })
                .add_modifier(Modifier::BOLD),
            ))
            .style(self.st())
    }

    fn led(&self, state: StackState) -> Span<'static> {
        let (sym, col) = match state {
            StackState::Running => ("●", self.pal.ok),
            StackState::Partial => ("◐", self.pal.warn),
            StackState::Failed => ("✕", self.pal.err),
            StackState::Stopped => ("○", self.pal.muted),
            StackState::Down => ("·", self.pal.muted),
        };
        Span::styled(sym, self.c(col).add_modifier(Modifier::BOLD))
    }

    fn container_led(&self, c: &model::Container) -> Span<'static> {
        let (sym, col) = match c.state.as_str() {
            "running" if c.health == Health::Unhealthy => ("●", self.pal.err),
            "running" if c.health == Health::Starting => ("●", self.pal.warn),
            "running" => ("●", self.pal.ok),
            "paused" => ("‖", self.pal.warn),
            "restarting" => ("↻", self.pal.warn),
            "exited"
                if c.exit_code()
                    .is_some_and(|x| x != 0 && x != 137 && x != 143) =>
            {
                ("✕", self.pal.err)
            }
            "dead" => ("✕", self.pal.err),
            _ => ("○", self.pal.muted),
        };
        Span::styled(sym, self.c(col).add_modifier(Modifier::BOLD))
    }

    fn health_span(&self, h: Health) -> Span<'static> {
        match h {
            Health::Healthy => Span::styled("♥", self.c(self.pal.ok)),
            Health::Unhealthy => {
                Span::styled("♥", self.c(self.pal.err).add_modifier(Modifier::SLOW_BLINK))
            }
            Health::Starting => Span::styled("♡", self.c(self.pal.warn)),
            Health::None => Span::raw(" "),
        }
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Block::default().style(app.st()), area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(area);
    header(f, app, rows[0]);
    tabs(f, app, rows[1]);
    if let Some(e) = &app.unreachable {
        let p = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  ▓▒░ NO CARRIER ░▒▓",
                app.c(app.pal.err).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(format!("  {e}"), app.c(app.pal.fg))),
            Line::from(""),
            Line::from(Span::styled(
                "  Retrying… R tries now, q quits.",
                app.c(app.pal.muted),
            )),
        ])
        .wrap(Wrap { trim: false })
        .block(app.panel("DOCKER", true));
        f.render_widget(p, rows[2]);
    } else if !app.loaded {
        let p = Paragraph::new(Span::styled("  Reading Docker…", app.c(app.pal.muted)))
            .block(app.panel("DOCKER", true));
        f.render_widget(p, rows[2]);
    } else {
        match app.tab {
            Tab::System => system(f, app, rows[2]),
            _ => {
                let wide = rows[2].width >= 110;
                let cols = Layout::default()
                    .direction(if wide {
                        Direction::Horizontal
                    } else {
                        Direction::Vertical
                    })
                    .constraints(if wide {
                        [Constraint::Percentage(58), Constraint::Percentage(42)]
                    } else {
                        [Constraint::Percentage(55), Constraint::Percentage(45)]
                    })
                    .split(rows[2]);
                list(f, app, cols[0]);
                details(f, app, cols[1]);
            }
        }
    }
    activity(f, app, rows[3]);
    footer(f, app, rows[4]);
    match &app.mode {
        Mode::Help => help(f, app, area),
        Mode::Confirm(_) => confirm(f, app, area),
        Mode::Uninstall(_) => uninstall(f, app, area),
        Mode::Pick(_) => picker(f, app, area),
        Mode::Logs(_) => logs(f, app, rows[2].union(rows[3])),
        Mode::Text(_) => text(f, app, rows[2].union(rows[3])),
        Mode::Normal | Mode::Filter => {}
    }
}

fn header(f: &mut Frame, app: &App, r: Rect) {
    let running = app.snap.containers.iter().filter(|c| c.running()).count();
    let stopped = app.snap.containers.len() - running;
    let cpu: f64 = app.live.values().map(|l| l.last.cpu).sum();
    let mem: u64 = app.live.values().map(|l| l.last.mem).sum();
    let left = vec![
        Span::styled("▓▒░", app.c(app.pal.accent)),
        Span::styled(
            " DOCKER·STACKS ",
            Style::default()
                .fg(app.pal.bg)
                .bg(app.pal.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("░▒▓ ", app.c(app.pal.accent)),
        Span::styled(
            app.hostname.clone(),
            app.c(app.pal.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.snap.version.is_empty() {
                String::new()
            } else {
                format!("  docker {}", app.snap.version)
            },
            app.c(app.pal.muted),
        ),
        Span::styled("   ● ", app.c(app.pal.ok)),
        Span::styled(format!("{running} up"), app.c(app.pal.fg)),
        Span::styled("  ○ ", app.c(app.pal.muted)),
        Span::styled(format!("{stopped} stopped"), app.c(app.pal.fg)),
        Span::styled(
            format!("  ▤ {} stacks", app.snap.stacks.len()),
            app.c(app.pal.fg),
        ),
    ];
    let right = format!(
        "cpu {:>5.1}%  mem {:>6}  {} ",
        cpu,
        model::human_bytes(mem as i64),
        util::clock(engine::now())
    );
    let used: usize = left.iter().map(|s| s.width()).sum();
    let gap = (r.width as usize).saturating_sub(used + right.chars().count());
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(right, app.c(app.pal.muted)));
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(app.pal.surface)),
        r,
    );
}

fn tabs(f: &mut Frame, app: &mut App, r: Rect) {
    let mut spans = vec![Span::raw(" ")];
    let mut x = r.x + 1;
    app.hit_tabs.clear();
    for (i, t) in TABS.iter().enumerate() {
        let label = format!(" {} {} ", i + 1, t.label());
        let w = label.chars().count() as u16;
        let style = if *t == app.tab {
            Style::default()
                .fg(app.pal.bg)
                .bg(app.pal.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            app.c(app.pal.muted)
        };
        app.hit_tabs.push((x, x + w, *t));
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        x += w + 1;
    }
    let filter = &app.filter[app.tab.index()];
    if matches!(app.mode, Mode::Filter) || !filter.is_empty() {
        spans.push(Span::styled(format!("  / {filter}"), app.c(app.pal.warn)));
        if matches!(app.mode, Mode::Filter) {
            spans.push(Span::styled(
                "▌",
                app.c(app.pal.warn).add_modifier(Modifier::SLOW_BLINK),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(app.st()), r);
}

fn list(f: &mut Frame, app: &mut App, r: Rect) {
    let title = format!("{} · {}", app.tab.label(), app.row_count(app.tab));
    let block = app.panel(&title, true);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let w = inner.width as usize;
    let (head, rows): (String, Vec<Line>) = match app.tab {
        Tab::Stacks => stack_lines(app, w),
        Tab::Containers => container_lines(app, w),
        Tab::Images => image_lines(app, w),
        Tab::Volumes => volume_lines(app, w),
        Tab::Networks => network_lines(app, w),
        Tab::System => (String::new(), vec![]),
    };
    if inner.height < 2 {
        return;
    }
    f.render_widget(
        Paragraph::new(Span::styled(
            head,
            app.c(app.pal.muted).add_modifier(Modifier::BOLD),
        )),
        Rect { height: 1, ..inner },
    );
    let body = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    let n = rows.len();
    let sel = app.selected().min(n.saturating_sub(1));
    let h = body.height as usize;
    let top = if sel >= h { sel + 1 - h } else { 0 };
    app.hit_rows = Some((body.y, body.y + body.height, body.x, top));
    if n == 0 {
        let msg = if app.filter[app.tab.index()].is_empty() {
            "  Nothing here."
        } else {
            "  Nothing matches the filter (Esc clears it)."
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, app.c(app.pal.muted))),
            body,
        );
        return;
    }
    let lines: Vec<Line> = rows
        .into_iter()
        .enumerate()
        .skip(top)
        .take(h)
        .map(|(i, l)| {
            if i == sel {
                let mut l = l;
                l.style = Style::default().bg(app.pal.select_bg).fg(app.pal.select_fg);
                let used: usize = l.spans.iter().map(|s| s.width()).sum();
                l.spans.push(Span::raw(" ".repeat(w.saturating_sub(used))));
                l
            } else {
                l
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), body);
    // A scroll mark on the right edge.
    if n > h {
        let pos = (sel as f64 / (n - 1).max(1) as f64 * (h - 1) as f64).round() as u16;
        f.render_widget(
            Paragraph::new(Span::styled("▐", app.c(app.pal.accent))),
            Rect {
                x: r.x + r.width - 1,
                y: body.y + pos,
                width: 1,
                height: 1,
            },
        );
    }
}

fn stack_lines(app: &App, w: usize) -> (String, Vec<Line<'static>>) {
    let name_w = w.saturating_sub(42).clamp(10, 40);
    let head = format!(
        "  {}  {:>5}    {:>6} {:>6}  {:<12} {}",
        pad("NAME", name_w),
        "UP",
        "CPU",
        "MEM",
        "ACTIVITY",
        "LINK"
    );
    let c = &app.snap.containers;
    let rows = app
        .stack_rows()
        .into_iter()
        .map(|row| match row {
            StackRow::Stack(st) => {
                let state = st.state(c);
                let (cpu, mem, hist) = app.live_sum(&st.containers);
                let up = if st.containers.is_empty() {
                    "—".to_string()
                } else {
                    format!("{}/{}", st.running(c), st.expected(c).len())
                };
                let links = app.links_of(&st.containers);
                let link = links
                    .first()
                    .map(|l| l.url.replace("http://", "").replace("https://", ""))
                    .unwrap_or_default();
                let running = state != StackState::Down && state != StackState::Stopped;
                Line::from(vec![
                    Span::raw(" "),
                    app.led(state),
                    Span::raw(" "),
                    Span::styled(
                        pad(&st.name, name_w),
                        app.c(app.pal.fg).add_modifier(if running {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                    ),
                    Span::styled(format!("  {:>5} ", up), app.c(app.pal.muted)),
                    app.health_span(st.health(c)),
                    Span::raw("  "),
                    Span::styled(
                        if running {
                            format!("{cpu:>5.1}%")
                        } else {
                            "      ".into()
                        },
                        app.c(if cpu > 80.0 { app.pal.warn } else { app.pal.fg }),
                    ),
                    Span::styled(
                        if running {
                            format!(" {:>6}", model::human_bytes(mem as i64))
                        } else {
                            "       ".into()
                        },
                        app.c(app.pal.fg),
                    ),
                    Span::styled(format!("  {} ", spark(&hist, 12)), app.c(app.pal.accent)),
                    Span::styled(link, app.c(app.pal.muted)),
                ])
            }
            StackRow::Loose(v) => {
                let running = v.iter().filter(|i| c[**i].running()).count();
                let (cpu, mem, hist) = app.live_sum(&v);
                Line::from(vec![
                    Span::styled(" ◇ ", app.c(app.pal.muted)),
                    Span::styled(
                        pad("outside any stack", name_w),
                        app.c(app.pal.muted).add_modifier(Modifier::ITALIC),
                    ),
                    Span::styled(
                        format!("  {:>5}  ", format!("{running}/{}", v.len())),
                        app.c(app.pal.muted),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("{cpu:>5.1}% {:>6}", model::human_bytes(mem as i64)),
                        app.c(app.pal.fg),
                    ),
                    Span::styled(format!("  {} ", spark(&hist, 12)), app.c(app.pal.accent)),
                ])
            }
        })
        .collect();
    (head, rows)
}

fn container_lines(app: &App, w: usize) -> (String, Vec<Line<'static>>) {
    let name_w = (w / 4).clamp(12, 34);
    let image_w = (w / 5).clamp(10, 30);
    let head = format!(
        "  {}  {}  {:<16} {:>6} {:>6}  {}",
        pad("NAME", name_w),
        pad("IMAGE", image_w),
        "STATUS",
        "CPU",
        "MEM",
        "STACK"
    );
    let rows = app
        .container_rows()
        .into_iter()
        .map(|i| {
            let c = &app.snap.containers[i];
            let live = app.live.get(&c.id);
            Line::from(vec![
                Span::raw(" "),
                app.container_led(c),
                Span::raw(" "),
                Span::styled(pad(&c.name, name_w), app.c(app.pal.fg)),
                Span::styled(
                    format!("  {}", pad(&c.image, image_w)),
                    app.c(app.pal.muted),
                ),
                Span::styled(
                    format!("  {}", pad(&short_status(&c.status), 16)),
                    app.c(app.pal.fg),
                ),
                Span::styled(
                    live.map(|l| {
                        format!(
                            " {:>5.1}% {:>6}",
                            l.last.cpu,
                            model::human_bytes(l.last.mem as i64)
                        )
                    })
                    .unwrap_or_else(|| " ".repeat(14)),
                    app.c(app.pal.fg),
                ),
                Span::styled(
                    format!("  {}", c.project.clone().unwrap_or_default()),
                    app.c(app.pal.muted),
                ),
            ])
        })
        .collect();
    (head, rows)
}

fn image_lines(app: &App, w: usize) -> (String, Vec<Line<'static>>) {
    let name_w = w.saturating_sub(40).clamp(16, 70);
    let head = format!(
        "  {}  {:<12} {:>7} {:>6}  {}",
        pad("REPOSITORY:TAG", name_w),
        "ID",
        "SIZE",
        "AGE",
        "USED BY"
    );
    let now = engine::now();
    let rows = app
        .image_rows()
        .into_iter()
        .map(|i| {
            let im = &app.snap.images[i];
            let col = if im.dangling() {
                app.pal.muted
            } else {
                app.pal.fg
            };
            Line::from(vec![
                Span::styled(
                    if im.used_by > 0 { " ● " } else { " ○ " },
                    app.c(if im.used_by > 0 {
                        app.pal.ok
                    } else {
                        app.pal.muted
                    }),
                ),
                Span::styled(pad(&im.label(), name_w), app.c(col)),
                Span::styled(format!("  {:<12}", im.short_id()), app.c(app.pal.muted)),
                Span::styled(
                    format!(" {:>7}", model::human_bytes(im.size)),
                    app.c(app.pal.fg),
                ),
                Span::styled(
                    format!(" {:>6}", model::ago(im.created, now)),
                    app.c(app.pal.muted),
                ),
                Span::styled(
                    format!(
                        "  {}",
                        if im.used_by > 0 {
                            im.used_by.to_string()
                        } else {
                            "—".into()
                        }
                    ),
                    app.c(app.pal.fg),
                ),
            ])
        })
        .collect();
    (head, rows)
}

fn volume_lines(app: &App, w: usize) -> (String, Vec<Line<'static>>) {
    let name_w = w.saturating_sub(40).clamp(16, 64);
    let head = format!(
        "  {}  {:<14} {:>7}  {}",
        pad("NAME", name_w),
        "STACK",
        "SIZE",
        "USED BY"
    );
    let rows = app
        .volume_rows()
        .into_iter()
        .map(|i| {
            let v = &app.snap.volumes[i];
            let used = !v.used_by.is_empty();
            Line::from(vec![
                Span::styled(
                    if used { " ● " } else { " ○ " },
                    app.c(if used { app.pal.ok } else { app.pal.muted }),
                ),
                Span::styled(pad(&v.name, name_w), app.c(app.pal.fg)),
                Span::styled(
                    format!("  {}", pad(v.project.as_deref().unwrap_or("—"), 14)),
                    app.c(app.pal.muted),
                ),
                Span::styled(
                    format!(
                        " {:>7}",
                        v.size.map(model::human_bytes).unwrap_or_else(|| "·".into())
                    ),
                    app.c(app.pal.fg),
                ),
                Span::styled(format!("  {}", v.used_by.join(", ")), app.c(app.pal.muted)),
            ])
        })
        .collect();
    (head, rows)
}

fn network_lines(app: &App, w: usize) -> (String, Vec<Line<'static>>) {
    let name_w = w.saturating_sub(40).clamp(16, 50);
    let head = format!(
        "  {}  {:<8} {:<6}  {}",
        pad("NAME", name_w),
        "DRIVER",
        "SCOPE",
        "CONTAINERS"
    );
    let rows = app
        .network_rows()
        .into_iter()
        .map(|i| {
            let n = &app.snap.networks[i];
            Line::from(vec![
                Span::styled(
                    if n.builtin() {
                        " ◆ "
                    } else if n.containers.is_empty() {
                        " ○ "
                    } else {
                        " ● "
                    },
                    app.c(if n.containers.is_empty() {
                        app.pal.muted
                    } else {
                        app.pal.ok
                    }),
                ),
                Span::styled(pad(&n.name, name_w), app.c(app.pal.fg)),
                Span::styled(
                    format!("  {:<8} {:<6}", n.driver, n.scope),
                    app.c(app.pal.muted),
                ),
                Span::styled(format!("  {}", n.containers.len()), app.c(app.pal.fg)),
            ])
        })
        .collect();
    (head, rows)
}

fn kv(app: &App, k: &str, v: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<11}", k), app.c(app.pal.muted)),
        Span::styled(v.into(), app.c(app.pal.fg)),
    ])
}

fn section(app: &App, title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("─ {title} ─"),
        app.c(app.pal.accent).add_modifier(Modifier::BOLD),
    ))
}

fn details(f: &mut Frame, app: &App, r: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let c = &app.snap.containers;
    let now = engine::now();
    let title;
    match app.tab {
        Tab::Stacks => {
            title = "STACK".to_string();
            let idx = app.stack_row_containers();
            if let Some(st) = app.current_stack() {
                let state = st.state(c);
                lines.push(Line::from(vec![
                    app.led(state),
                    Span::styled(
                        format!(" {} ", st.name.to_uppercase()),
                        app.c(app.pal.fg).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(state.label().to_string(), app.c(app.pal.muted)),
                ]));
                if let Some(d) = &st.dir {
                    lines.push(kv(app, "folder", d.display().to_string()));
                }
                for file in &st.files {
                    let mark = match model::files_state(std::slice::from_ref(file)) {
                        model::Files::Ok => "",
                        model::Files::NoAccess => "  (no access)",
                        model::Files::Missing => "  (missing)",
                    };
                    lines.push(kv(
                        app,
                        "file",
                        format!(
                            "{}{mark}",
                            file.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        ),
                    ));
                }
                if let Some(e) = &st.config_error {
                    lines.push(Line::from(Span::styled(
                        format!("! {e}"),
                        app.c(app.pal.err),
                    )));
                }
                lines.push(Line::from(""));
                lines.push(section(app, "SERVICES"));
                let mut names: Vec<String> = st.expected(c).into_iter().collect();
                for i in &st.containers {
                    if let Some(s) = &c[*i].service {
                        if !names.contains(s) {
                            names.push(s.clone());
                        }
                    }
                }
                for svc in names {
                    let cont: Vec<&model::Container> = st
                        .containers
                        .iter()
                        .map(|i| &c[*i])
                        .filter(|x| x.service.as_deref() == Some(svc.as_str()))
                        .collect();
                    let def = st.services.iter().find(|d| d.name == svc);
                    match cont.first() {
                        Some(x) => {
                            let live = app.live.get(&x.id);
                            lines.push(Line::from(vec![
                                app.container_led(x),
                                Span::styled(format!(" {}", pad(&svc, 16)), app.c(app.pal.fg)),
                                Span::styled(
                                    pad(&short_status(&x.status), 14),
                                    app.c(app.pal.muted),
                                ),
                                Span::styled(
                                    live.map(|l| {
                                        format!(
                                            " {:>5.1}% {:>6}",
                                            l.last.cpu,
                                            model::human_bytes(l.last.mem as i64)
                                        )
                                    })
                                    .unwrap_or_default(),
                                    app.c(app.pal.fg),
                                ),
                            ]));
                            lines.push(Line::from(Span::styled(
                                format!("    {}", short_image(&x.image)),
                                app.c(app.pal.muted),
                            )));
                        }
                        None => {
                            lines.push(Line::from(vec![
                                Span::styled("·", app.c(app.pal.muted)),
                                Span::styled(format!(" {}", pad(&svc, 16)), app.c(app.pal.muted)),
                                Span::styled(
                                    def.map(|d| {
                                        if d.profiles.is_empty() {
                                            "not created".to_string()
                                        } else {
                                            format!("profile {}", d.profiles.join(","))
                                        }
                                    })
                                    .unwrap_or_default(),
                                    app.c(app.pal.muted),
                                ),
                            ]));
                            if let Some(d) = def {
                                if !d.image.is_empty() {
                                    lines.push(Line::from(Span::styled(
                                        format!("    {}", d.image),
                                        app.c(app.pal.muted),
                                    )));
                                }
                            }
                        }
                    }
                }
                let links = app.links_of(&idx);
                if !links.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(section(app, "LINKS  o open · y copy"));
                    for l in links.iter().take(6) {
                        lines.push(Line::from(vec![
                            Span::styled("↗ ", app.c(app.pal.accent)),
                            Span::styled(
                                l.url.clone(),
                                app.c(app.pal.fg).add_modifier(Modifier::UNDERLINED),
                            ),
                            Span::styled(format!("  {}", l.from), app.c(app.pal.muted)),
                        ]));
                    }
                }
                let vols: Vec<&model::Volume> = app
                    .snap
                    .volumes
                    .iter()
                    .filter(|v| v.project.as_deref() == Some(st.name.as_str()))
                    .collect();
                if !vols.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(section(app, "VOLUMES"));
                    for v in vols {
                        lines.push(Line::from(vec![
                            Span::styled("▣ ", app.c(app.pal.muted)),
                            Span::styled(v.name.clone(), app.c(app.pal.fg)),
                            Span::styled(
                                v.size
                                    .map(|s| format!("  {}", model::human_bytes(s)))
                                    .unwrap_or_default(),
                                app.c(app.pal.muted),
                            ),
                        ]));
                    }
                }
                let dbs = st
                    .containers
                    .iter()
                    .filter(|i| App::db_kind(&c[**i].image).is_some())
                    .count();
                if dbs > 0 {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!(
                            "B dumps the {dbs} database(s) to {}",
                            app.cfg.backups().display()
                        ),
                        app.c(app.pal.muted),
                    )));
                }
            } else if !idx.is_empty() {
                lines.push(Line::from(Span::styled(
                    "CONTAINERS OUTSIDE ANY STACK",
                    app.c(app.pal.fg).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(
                    "Started with docker run; manage them under 2.",
                    app.c(app.pal.muted),
                )));
                lines.push(Line::from(""));
                for i in idx {
                    let x = &c[i];
                    lines.push(Line::from(vec![
                        app.container_led(x),
                        Span::styled(format!(" {}", pad(&x.name, 22)), app.c(app.pal.fg)),
                        Span::styled(x.status.clone(), app.c(app.pal.muted)),
                    ]));
                }
            }
        }
        Tab::Containers => {
            title = "CONTAINER".to_string();
            if let Some(x) = app.current_container() {
                lines.push(Line::from(vec![
                    app.container_led(x),
                    Span::styled(
                        format!(" {}", x.name),
                        app.c(app.pal.fg).add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(kv(app, "status", x.status.clone()));
                lines.push(kv(app, "image", x.image.clone()));
                lines.push(kv(app, "id", x.short_id().to_string()));
                lines.push(kv(
                    app,
                    "created",
                    format!("{} ago", model::ago(x.created, now)),
                ));
                if let Some(p) = &x.project {
                    lines.push(kv(
                        app,
                        "stack",
                        format!("{p} · {}", x.service.clone().unwrap_or_default()),
                    ));
                }
                if let Some(l) = app.live.get(&x.id) {
                    let s = l.last;
                    lines.push(Line::from(""));
                    lines.push(section(app, "LIVE"));
                    lines.push(Line::from(vec![
                        Span::styled("cpu      ", app.c(app.pal.muted)),
                        Span::styled(format!("{:>6.1}% ", s.cpu), app.c(app.pal.fg)),
                        Span::styled(spark(&l.cpu, 24), app.c(app.pal.accent)),
                    ]));
                    let frac = if s.mem_limit > 0 {
                        s.mem as f64 / s.mem_limit as f64
                    } else {
                        0.0
                    };
                    lines.push(Line::from(vec![
                        Span::styled("memory   ", app.c(app.pal.muted)),
                        Span::styled(
                            format!("{:>7} ", model::human_bytes(s.mem as i64)),
                            app.c(app.pal.fg),
                        ),
                        Span::styled(
                            meter(frac, 16),
                            app.c(if frac > 0.85 {
                                app.pal.err
                            } else {
                                app.pal.accent
                            }),
                        ),
                        Span::styled(
                            format!(" of {}", model::human_bytes(s.mem_limit as i64)),
                            app.c(app.pal.muted),
                        ),
                    ]));
                    lines.push(kv(
                        app,
                        "network",
                        format!(
                            "↓ {}  ↑ {}",
                            model::human_bytes(s.rx as i64),
                            model::human_bytes(s.tx as i64)
                        ),
                    ));
                    lines.push(kv(
                        app,
                        "disk io",
                        format!(
                            "read {}  write {}",
                            model::human_bytes(s.read as i64),
                            model::human_bytes(s.write as i64)
                        ),
                    ));
                    lines.push(kv(app, "pids", s.pids.to_string()));
                }
                if !x.ports.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(section(app, "PORTS"));
                    for p in &x.ports {
                        lines.push(Line::from(Span::styled(
                            match p.public {
                                Some(pub_) => format!(
                                    "{}:{pub_} → {}/{}",
                                    if p.ip.is_empty() { "*" } else { &p.ip },
                                    p.private,
                                    p.proto
                                ),
                                None => format!("{}/{} (not published)", p.private, p.proto),
                            },
                            app.c(app.pal.fg),
                        )));
                    }
                }
                let i = app.container_rows()[app.sel[1]];
                let links = app.links_of(&[i]);
                if !links.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(section(app, "LINKS  o open · y copy"));
                    for l in links {
                        lines.push(Line::from(vec![
                            Span::styled("↗ ", app.c(app.pal.accent)),
                            Span::styled(
                                l.url,
                                app.c(app.pal.fg).add_modifier(Modifier::UNDERLINED),
                            ),
                        ]));
                    }
                }
                if !x.volumes.is_empty() {
                    lines.push(kv(app, "volumes", x.volumes.join(", ")));
                }
                if !x.networks.is_empty() {
                    lines.push(kv(app, "networks", x.networks.join(", ")));
                }
            }
        }
        Tab::Images => {
            title = "IMAGE".to_string();
            if let Some(im) = app
                .image_rows()
                .get(app.sel[2])
                .map(|i| &app.snap.images[*i])
            {
                lines.push(Line::from(Span::styled(
                    im.label(),
                    app.c(app.pal.fg).add_modifier(Modifier::BOLD),
                )));
                lines.push(kv(app, "id", im.short_id().to_string()));
                lines.push(kv(app, "size", model::human_bytes(im.size)));
                lines.push(kv(
                    app,
                    "created",
                    format!("{} ago", model::ago(im.created, now)),
                ));
                lines.push(Line::from(""));
                lines.push(section(app, "USED BY"));
                let users: Vec<&model::Container> =
                    c.iter().filter(|x| x.image_id == im.id).collect();
                if users.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "no container: D removes it",
                        app.c(app.pal.muted),
                    )));
                }
                for x in users {
                    lines.push(Line::from(vec![
                        app.container_led(x),
                        Span::styled(format!(" {}", x.name), app.c(app.pal.fg)),
                    ]));
                }
            }
        }
        Tab::Volumes => {
            title = "VOLUME".to_string();
            if let Some(v) = app
                .volume_rows()
                .get(app.sel[3])
                .map(|i| &app.snap.volumes[*i])
            {
                lines.push(Line::from(Span::styled(
                    v.name.clone(),
                    app.c(app.pal.fg).add_modifier(Modifier::BOLD),
                )));
                lines.push(kv(app, "driver", v.driver.clone()));
                lines.push(kv(
                    app,
                    "stack",
                    v.project.clone().unwrap_or_else(|| "—".into()),
                ));
                lines.push(kv(
                    app,
                    "size",
                    v.size
                        .map(model::human_bytes)
                        .unwrap_or_else(|| "z measures".into()),
                ));
                lines.push(kv(app, "path", v.mountpoint.clone()));
                lines.push(Line::from(""));
                lines.push(section(app, "USED BY"));
                if v.used_by.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "no container",
                        app.c(app.pal.muted),
                    )));
                }
                for n in &v.used_by {
                    lines.push(Line::from(Span::styled(n.clone(), app.c(app.pal.fg))));
                }
            }
        }
        Tab::Networks => {
            title = "NETWORK".to_string();
            if let Some(n) = app
                .network_rows()
                .get(app.sel[4])
                .map(|i| &app.snap.networks[*i])
            {
                lines.push(Line::from(Span::styled(
                    n.name.clone(),
                    app.c(app.pal.fg).add_modifier(Modifier::BOLD),
                )));
                lines.push(kv(app, "driver", n.driver.clone()));
                lines.push(kv(app, "scope", n.scope.clone()));
                lines.push(kv(
                    app,
                    "stack",
                    n.project.clone().unwrap_or_else(|| "—".into()),
                ));
                lines.push(Line::from(""));
                lines.push(section(app, "CONTAINERS"));
                for x in &n.containers {
                    lines.push(Line::from(Span::styled(x.clone(), app.c(app.pal.fg))));
                }
            }
        }
        Tab::System => return,
    }
    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(app.panel(&title, false));
    f.render_widget(p, r);
}

fn system(f: &mut Frame, app: &App, r: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(r);
    let i = &app.snap.info;
    let mut lines = vec![
        kv(app, "host", i["Name"].as_str().unwrap_or("").to_string()),
        kv(
            app,
            "system",
            format!(
                "{} · {}",
                i["OperatingSystem"].as_str().unwrap_or(""),
                i["KernelVersion"].as_str().unwrap_or("")
            ),
        ),
        kv(
            app,
            "docker",
            format!(
                "{} · {}",
                app.snap.version,
                i["Driver"].as_str().unwrap_or("")
            ),
        ),
        kv(app, "cpus", i["NCPU"].as_i64().unwrap_or(0).to_string()),
        kv(
            app,
            "memory",
            model::human_bytes(i["MemTotal"].as_i64().unwrap_or(0)),
        ),
        kv(
            app,
            "root",
            i["DockerRootDir"].as_str().unwrap_or("").to_string(),
        ),
        kv(
            app,
            "containers",
            format!(
                "{} running · {} paused · {} stopped",
                i["ContainersRunning"].as_i64().unwrap_or(0),
                i["ContainersPaused"].as_i64().unwrap_or(0),
                i["ContainersStopped"].as_i64().unwrap_or(0)
            ),
        ),
        kv(app, "images", app.snap.images.len().to_string()),
        kv(
            app,
            "refresh",
            format!("{} ms a snapshot · live from events", app.snap.took_ms),
        ),
        Line::from(""),
        section(app, "DISK"),
    ];
    match &app.df {
        None => lines.push(Line::from(Span::styled(
            "z measures disk usage (takes a moment)",
            app.c(app.pal.muted),
        ))),
        Some(df) => {
            let imgs: i64 = df["Images"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| x["Size"].as_i64().unwrap_or(0))
                .sum();
            let imgs_free: i64 = df["Images"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|x| x["Containers"].as_i64().unwrap_or(0) == 0)
                .map(|x| x["Size"].as_i64().unwrap_or(0))
                .sum();
            let cont: i64 = df["Containers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| x["SizeRw"].as_i64().unwrap_or(0))
                .sum();
            let vols: i64 = df["Volumes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| x["UsageData"]["Size"].as_i64().unwrap_or(0).max(0))
                .sum();
            let vols_free: i64 = df["Volumes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|x| x["UsageData"]["RefCount"].as_i64().unwrap_or(0) == 0)
                .map(|x| x["UsageData"]["Size"].as_i64().unwrap_or(0).max(0))
                .sum();
            let cache: i64 = df["BuildCache"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| x["Size"].as_i64().unwrap_or(0))
                .sum();
            let total = (imgs + cont + vols + cache).max(1);
            for (name, size, free) in [
                ("images", imgs, imgs_free),
                ("containers", cont, 0),
                ("volumes", vols, vols_free),
                ("build cache", cache, cache),
            ] {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<12}", name), app.c(app.pal.muted)),
                    Span::styled(meter(size as f64 / total as f64, 18), app.c(app.pal.accent)),
                    Span::styled(
                        format!(" {:>7}", model::human_bytes(size)),
                        app.c(app.pal.fg),
                    ),
                    Span::styled(
                        if free > 0 {
                            format!("  {} reclaimable", model::human_bytes(free))
                        } else {
                            String::new()
                        },
                        app.c(app.pal.muted),
                    ),
                ]));
            }
        }
    }
    lines.push(Line::from(""));
    lines.push(section(app, "CLEAN UP"));
    for (k, what) in [
        ("P", "remove dangling images"),
        ("C", "clear the build cache"),
        ("X", "clean up everything unused (volumes kept)"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {k} "),
                Style::default().fg(app.pal.bg).bg(app.pal.accent),
            ),
            Span::styled(format!(" {what}"), app.c(app.pal.fg)),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(app.panel("SYSTEM", true)),
        cols[0],
    );
    let h = cols[1].height.saturating_sub(2) as usize;
    let ev: Vec<Line> = app
        .events
        .iter()
        .rev()
        .take(h)
        .map(|(t, e)| {
            let col = if e.contains("die")
                || e.contains("oom")
                || e.contains("unhealthy")
                || e.contains("kill")
            {
                app.pal.err
            } else if e.contains("start") || e.contains("create") {
                app.pal.ok
            } else {
                app.pal.fg
            };
            Line::from(vec![
                Span::styled(format!("{} ", util::clock(*t)), app.c(app.pal.muted)),
                Span::styled(e.clone(), app.c(col)),
            ])
        })
        .collect();
    let ev = if ev.is_empty() {
        vec![Line::from(Span::styled(
            "Waiting for something to happen…",
            app.c(app.pal.muted),
        ))]
    } else {
        ev
    };
    f.render_widget(
        Paragraph::new(ev).block(app.panel("EVENTS", false)),
        cols[1],
    );
}

fn activity(f: &mut Frame, app: &App, r: Rect) {
    let h = r.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = app
        .activity
        .iter()
        .rev()
        .take(h)
        .map(|a| {
            let (sym, col, tail) = match &a.result {
                None => {
                    let spin = ["◐", "◓", "◑", "◒"][(engine::now() as usize) % 4];
                    (spin, app.pal.warn, a.last.clone())
                }
                Some(Ok(t)) => ("✓", app.pal.ok, t.clone()),
                Some(Err(e)) => ("✕", app.pal.err, e.clone()),
            };
            Line::from(vec![
                Span::styled(format!("{} ", util::clock(a.at)), app.c(app.pal.muted)),
                Span::styled(format!("{sym} "), app.c(col).add_modifier(Modifier::BOLD)),
                Span::styled(a.label.clone(), app.c(app.pal.fg)),
                Span::styled(
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!("  {tail}")
                    },
                    app.c(app.pal.muted),
                ),
            ])
        })
        .collect();
    lines.reverse();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Ready. ? shows every key.",
            app.c(app.pal.muted),
        )));
    }
    let title = match app.busy() {
        0 => "ACTIVITY".to_string(),
        n => format!("ACTIVITY · {n} running"),
    };
    f.render_widget(Paragraph::new(lines).block(app.panel(&title, false)), r);
}

fn chip(app: &App, k: &str, what: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {k} "),
            Style::default().fg(app.pal.bg).bg(app.pal.muted),
        ),
        Span::styled(format!(" {what} "), app.c(app.pal.fg)),
    ]
}

fn footer(f: &mut Frame, app: &App, r: Rect) {
    if let Some((text, warn, at)) = &app.flash {
        if at.elapsed().as_secs() < 5 {
            let style = if *warn {
                Style::default().fg(app.pal.bg).bg(app.pal.err)
            } else {
                Style::default().fg(app.pal.bg).bg(app.pal.ok)
            };
            f.render_widget(
                Paragraph::new(Span::styled(format!(" {text} "), style))
                    .style(Style::default().bg(app.pal.surface)),
                r,
            );
            return;
        }
    }
    let keys: &[(&str, &str)] = match (&app.mode, app.tab) {
        (Mode::Filter, _) => &[("type", "filter"), ("Enter", "keep"), ("Esc", "clear")],
        (_, Tab::Stacks) => &[
            ("u", "up"),
            ("d", "down"),
            ("s", "stop"),
            ("r", "restart"),
            ("U", "update"),
            ("l", "logs"),
            ("e", "shell"),
            ("o", "open"),
            ("E", "edit"),
            ("x", "uninstall"),
            ("?", "more"),
        ],
        (_, Tab::Containers) => &[
            ("S", "start"),
            ("s", "stop"),
            ("r", "restart"),
            ("l", "logs"),
            ("e", "shell"),
            ("o", "open"),
            ("i", "inspect"),
            ("D", "remove"),
            ("?", "more"),
        ],
        (_, Tab::Images) => &[
            ("p", "pull"),
            ("D", "remove"),
            ("P", "prune dangling"),
            ("A", "prune unused"),
            ("i", "inspect"),
        ],
        (_, Tab::Volumes) => &[
            ("z", "sizes"),
            ("D", "delete"),
            ("P", "prune unused"),
            ("i", "inspect"),
        ],
        (_, Tab::Networks) => &[("D", "remove"), ("P", "prune"), ("i", "inspect")],
        (_, Tab::System) => &[
            ("z", "disk usage"),
            ("P", "dangling"),
            ("C", "build cache"),
            ("X", "clean up"),
        ],
    };
    let mut spans = Vec::new();
    for (k, w) in keys {
        spans.extend(chip(app, k, w));
    }
    spans.extend(chip(app, "/", "filter"));
    spans.extend(chip(app, "t", &app.pal.name.clone()));
    spans.extend(chip(app, "q", "quit"));
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(app.pal.surface)),
        r,
    );
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn dialog_block<'a>(app: &App, title: &'a str, danger: bool) -> Block<'a> {
    let col = if danger { app.pal.err } else { app.pal.accent };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(app.c(col))
        .title(Span::styled(
            format!(" {title} "),
            app.c(col).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.pal.surface).fg(app.pal.fg))
}

fn confirm(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Confirm(c) = &app.mode else { return };
    let h = c.body.len() as u16 + 6;
    let r = centered(area, 72, h);
    f.render_widget(Clear, r);
    let mut lines: Vec<Line> = vec![Line::from("")];
    for b in &c.body {
        lines.push(Line::from(Span::raw(format!(" {b}"))));
    }
    lines.push(Line::from(""));
    match &c.word {
        Some(_) => lines.push(Line::from(vec![
            Span::styled(" > ", app.c(app.pal.warn)),
            Span::styled(
                c.typed.clone(),
                app.c(app.pal.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", app.c(app.pal.warn).add_modifier(Modifier::SLOW_BLINK)),
            Span::styled("   Enter · Esc", app.c(app.pal.muted)),
        ])),
        None => {
            let mut l = chip(app, "y", "yes");
            l.extend(chip(app, "n", "no"));
            lines.push(Line::from(l));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(dialog_block(app, &c.title, c.danger)),
        r,
    );
}

fn uninstall(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Uninstall(u) = &app.mode else {
        return;
    };
    let r = centered(area, 76, 16);
    f.render_widget(Clear, r);
    let box_ = |on: bool, can: bool| {
        if !can {
            "[–]"
        } else if on {
            "[x]"
        } else {
            "[ ]"
        }
    };
    let rows = [
        (
            box_(u.backup, u.can_backup),
            "Dump the databases first",
            if u.can_backup {
                ""
            } else {
                "  (no running PostgreSQL or MySQL)"
            },
        ),
        (
            box_(u.volumes, true),
            "Delete the volumes: the data is gone for good",
            "",
        ),
        (box_(u.images, true), "Delete the images it uses", ""),
        (
            box_(u.trash, u.can_trash),
            "Move the project folder to .trash",
            if u.can_trash {
                ""
            } else {
                "  (only folders right in a stacks root)"
            },
        ),
    ];
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::raw(
            " Containers and networks are removed. Choose what else goes:",
        )),
        Line::from(""),
    ];
    for (i, (b, what, note)) in rows.iter().enumerate() {
        let style = if i == u.row {
            Style::default().bg(app.pal.select_bg).fg(app.pal.select_fg)
        } else {
            Style::default()
        };
        let danger = i == 1 && u.volumes;
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {b} "),
                style.fg(if danger { app.pal.err } else { app.pal.accent }),
            ),
            Span::styled(what.to_string(), style),
            Span::styled(note.to_string(), app.c(app.pal.muted)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓ choose · Space toggles · type the name and Enter",
        app.c(app.pal.muted),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(format!(" {} > ", u.stack), app.c(app.pal.warn)),
        Span::styled(
            u.typed.clone(),
            app.c(app.pal.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("▌", app.c(app.pal.warn).add_modifier(Modifier::SLOW_BLINK)),
    ]));
    let title = format!("UNINSTALL {}", u.stack.to_uppercase());
    f.render_widget(
        Paragraph::new(lines).block(dialog_block(app, &title, true)),
        r,
    );
}

fn picker(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Pick(p) = &app.mode else { return };
    let r = centered(area, 84, p.items.len() as u16 + 4);
    f.render_widget(Clear, r);
    let lines: Vec<Line> = p
        .items
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            let style = if i == p.selected {
                Style::default().bg(app.pal.select_bg).fg(app.pal.select_fg)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(format!(" {} ", i + 1), app.c(app.pal.accent)),
                Span::styled(format!("{label} "), style),
            ])
        })
        .collect();
    let title = match p.purpose {
        PickFor::Open => "OPEN WHICH LINK",
        PickFor::Copy => "COPY WHICH LINK",
        PickFor::Shell => "SHELL IN WHICH SERVICE",
    };
    let mut all = vec![Line::from("")];
    all.extend(lines);
    f.render_widget(
        Paragraph::new(all).block(dialog_block(app, title, false)),
        r,
    );
    let _ = &p.title;
}

fn log_style(app: &App, line: &str) -> Style {
    let l = line.to_lowercase();
    if l.contains("error") || l.contains("fatal") || l.contains("panic") || l.contains("exception")
    {
        app.c(app.pal.err)
    } else if l.contains("warn") {
        app.c(app.pal.warn)
    } else {
        app.c(app.pal.fg)
    }
}

fn logs(f: &mut Frame, app: &App, r: Rect) {
    let Mode::Logs(v) = &app.mode else { return };
    f.render_widget(Clear, r);
    let state = if v.back == 0 {
        "▶ following"
    } else {
        "‖ paused"
    };
    let title = format!("LOGS · {} · {state}", v.title);
    let block = dialog_block(app, &title, false);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let h = inner.height.saturating_sub(1) as usize;
    let n = v.lines.len();
    let end = n.saturating_sub(v.back);
    let start = end.saturating_sub(h);
    let needle = v.search.to_lowercase();
    let lines: Vec<Line> = v
        .lines
        .range(start..end)
        .map(|l| {
            // "service-1  | text": the service in the accent colour.
            let (head, rest) = match l.find(" | ") {
                Some(i) if i < 48 => (format!("{} ", &l[..i]), l[i + 3..].to_string()),
                _ => (String::new(), l.clone()),
            };
            let mut style = log_style(app, &rest);
            if !needle.is_empty() && rest.to_lowercase().contains(&needle) {
                style = style.bg(app.pal.select_bg);
            }
            Line::from(vec![
                Span::styled(head, app.c(app.pal.accent)),
                Span::styled(rest, style),
            ])
        })
        .collect();
    let mut p = Paragraph::new(lines);
    if v.wrap {
        p = p.wrap(Wrap { trim: false });
    }
    f.render_widget(
        p,
        Rect {
            height: inner.height.saturating_sub(1),
            ..inner
        },
    );
    let status = if v.searching {
        format!(" search: {}▌", v.search)
    } else {
        format!(
            " {} lines · ↑↓ PgUp/PgDn scroll · End follow · / search n N · w wrap{} · t time{} · Esc close",
            n,
            if v.wrap { " on" } else { "" },
            if v.services.len() > 1 { " · s service" } else { "" }
        )
    };
    f.render_widget(
        Paragraph::new(Span::styled(status, app.c(app.pal.muted))),
        Rect {
            y: inner.y + inner.height.saturating_sub(1),
            height: 1,
            ..inner
        },
    );
}

fn text(f: &mut Frame, app: &App, r: Rect) {
    let Mode::Text(t) = &app.mode else { return };
    f.render_widget(Clear, r);
    let block = dialog_block(app, &t.title, false);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let h = inner.height.saturating_sub(1) as usize;
    let needle = t.search.to_lowercase();
    let lines: Vec<Line> = t
        .lines
        .iter()
        .skip(t.top)
        .take(h)
        .map(|l| {
            let key_end = l.find("\": ");
            let style = if !needle.is_empty() && l.to_lowercase().contains(&needle) {
                Style::default().bg(app.pal.select_bg)
            } else {
                Style::default()
            };
            match key_end {
                Some(i) => Line::from(vec![
                    Span::styled(l[..=i].to_string(), style.fg(app.pal.accent)),
                    Span::styled(l[i + 1..].to_string(), style.fg(app.pal.fg)),
                ]),
                None => Line::from(Span::styled(l.clone(), style.fg(app.pal.fg))),
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines),
        Rect {
            height: inner.height.saturating_sub(1),
            ..inner
        },
    );
    let status = if t.searching {
        format!(" search: {}▌", t.search)
    } else {
        format!(
            " {}/{} · ↑↓ PgUp/PgDn · / search n · y copy · Esc close",
            t.top + 1,
            t.lines.len()
        )
    };
    f.render_widget(
        Paragraph::new(Span::styled(status, app.c(app.pal.muted))),
        Rect {
            y: inner.y + inner.height.saturating_sub(1),
            height: 1,
            ..inner
        },
    );
}

const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "EVERYWHERE",
        &[
            ("1-6 Tab ←→", "views"),
            ("↑↓ jk PgUp PgDn g G", "move"),
            ("/  Esc", "filter, clear it"),
            ("Enter", "logs (stacks, containers), inspect (others)"),
            ("R", "refresh now"),
            ("t", "theme: NIRUX, amber, phosphor, C64, paper, terminal"),
            ("q  Ctrl+C", "quit"),
        ],
    ),
    (
        "STACKS",
        &[
            ("u  S  s  r", "up, start, stop, restart"),
            ("d", "down: containers go, volumes stay"),
            ("p  U  b", "pull, update (pull and up), build and up"),
            ("l  e", "logs, a shell in a service"),
            ("o  y", "open a link, copy it"),
            (
                "E  c  f  i",
                "edit the compose file, resolved config, folder, inspect",
            ),
            ("B", "dump its PostgreSQL / MySQL / MariaDB"),
            ("x", "uninstall: backups, volumes, images, folder"),
        ],
    ),
    (
        "CONTAINERS",
        &[
            ("S  s  r  P", "start, stop, restart, pause"),
            ("K  D", "kill, remove"),
            ("l  e  o  y  i", "logs, shell, open, copy link, inspect"),
        ],
    ),
    (
        "IMAGES · VOLUMES · NETWORKS · SYSTEM",
        &[
            ("p  D  i", "pull an image, remove, inspect"),
            ("P  A", "prune dangling / unused"),
            ("z", "measure sizes"),
            ("C  X", "build cache, clean up everything unused"),
        ],
    ),
];

fn help(f: &mut Frame, app: &App, area: Rect) {
    let n: usize = HELP.iter().map(|(_, k)| k.len() + 2).sum();
    let r = centered(area, 96, n as u16 + 4);
    f.render_widget(Clear, r);
    let mut lines = vec![];
    for (title, keys) in HELP {
        lines.push(section(app, title));
        for (k, w) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("  {}", pad(k, 22)), app.c(app.pal.accent)),
                Span::styled(w.to_string(), app.c(app.pal.fg)),
            ]));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "  Links come from published ports, Traefik and Caddy labels, and docker-stacks.url.",
        app.c(app.pal.muted),
    )));
    f.render_widget(
        Paragraph::new(lines).block(dialog_block(app, "KEYS", false)),
        r,
    );
    let _ = rpad;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meters_and_sparks_have_their_width() {
        assert_eq!(meter(0.5, 4), "██░░");
        let mut h = VecDeque::new();
        h.extend([0.0, 10.0, 50.0]);
        assert_eq!(spark(&h, 5).chars().count(), 5);
        assert_eq!(pad("abcdef", 4), "abc…");
        assert_eq!(pad("ab", 4), "ab  ");
    }
}
