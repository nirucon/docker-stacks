//! What the screen shows and what keys do.
use crate::config::{self, Config};
use crate::docker::{enc, Docker};
use crate::engine::{self, LogFollow, Msg, Ops, Sample, Snapshot, StatsStreams};
use crate::model::{self, Container, Link, Stack};
use crate::theme::{self, Palette};
use crate::util;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Stacks,
    Containers,
    Images,
    Volumes,
    Networks,
    System,
}

pub const TABS: [Tab; 6] = [
    Tab::Stacks,
    Tab::Containers,
    Tab::Images,
    Tab::Volumes,
    Tab::Networks,
    Tab::System,
];

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Stacks => "STACKS",
            Tab::Containers => "CONTAINERS",
            Tab::Images => "IMAGES",
            Tab::Volumes => "VOLUMES",
            Tab::Networks => "NETWORKS",
            Tab::System => "SYSTEM",
        }
    }

    pub fn index(self) -> usize {
        TABS.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// What happens when a confirmation is accepted.
#[derive(Clone, Debug)]
pub enum Action {
    Run(String, Vec<String>),
    Api(String, &'static str, String, u64),
    Chain(String, Vec<Step>),
}

/// One step of an uninstall.
#[derive(Clone, Debug)]
pub enum Step {
    Run(Vec<String>),
    Dump(Vec<String>, PathBuf),
    Trash(PathBuf, PathBuf),
    Forget(String),
}

pub struct Confirm {
    pub title: String,
    pub body: Vec<String>,
    pub action: Action,
    pub danger: bool,
    /// Typed text that must equal this to accept, for the worst actions.
    pub word: Option<String>,
    pub typed: String,
}

pub struct Uninstall {
    pub stack: String,
    pub row: usize,
    pub backup: bool,
    pub volumes: bool,
    pub images: bool,
    pub trash: bool,
    pub can_backup: bool,
    pub can_trash: bool,
    pub typed: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickFor {
    Open,
    Copy,
    Shell,
}

pub struct Picker {
    pub title: String,
    pub items: Vec<(String, String)>,
    pub selected: usize,
    pub purpose: PickFor,
}

pub struct LogView {
    pub title: String,
    pub lines: VecDeque<String>,
    /// Lines from the bottom; 0 follows.
    pub back: usize,
    pub wrap: bool,
    pub search: String,
    pub searching: bool,
    pub timestamps: bool,
    /// For a stack: the services to cycle through ("" is all).
    pub services: Vec<String>,
    pub service: usize,
    pub source: LogSource,
    pub follow: Option<LogFollow>,
}

#[derive(Clone)]
pub enum LogSource {
    Stack(String),
    Container(String, String),
}

pub struct TextView {
    pub title: String,
    pub lines: Vec<String>,
    pub top: usize,
    pub search: String,
    pub searching: bool,
}

pub enum Mode {
    Normal,
    Filter,
    Confirm(Confirm),
    Uninstall(Uninstall),
    Pick(Picker),
    Logs(Box<LogView>),
    Text(TextView),
    Help,
}

pub struct Activity {
    pub id: u64,
    pub label: String,
    pub last: String,
    pub result: Option<Result<String, String>>,
    pub at: i64,
}

pub struct Chain {
    pub id: u64,
    pub label: String,
    pub steps: VecDeque<Step>,
}

#[derive(Default, Clone)]
pub struct Live {
    pub last: Sample,
    pub cpu: VecDeque<f64>,
}

/// Something that wants the terminal for itself (a shell, an editor).
pub enum Foreground {
    Command(Vec<String>),
}

pub struct App {
    pub cfg: Config,
    pub pal: Palette,
    pub docker: Docker,
    pub snap: Snapshot,
    pub loaded: bool,
    pub unreachable: Option<String>,
    pub live: HashMap<String, Live>,
    pub tab: Tab,
    pub sel: [usize; 6],
    pub filter: [String; 6],
    pub mode: Mode,
    pub activity: VecDeque<Activity>,
    pub events: VecDeque<(i64, String)>,
    pub chain: Option<Chain>,
    pub ops: Ops,
    pub poke: Sender<()>,
    pub streams: StatsStreams,
    pub tx: Sender<Msg>,
    pub df: Option<Value>,
    pub host: String,
    pub hostname: String,
    pub quit: bool,
    pub foreground: Option<Foreground>,
    pub log_seq: u64,
    /// Where rows and tabs were drawn, for the mouse.
    pub hit_rows: Option<(u16, u16, u16, usize)>,
    pub hit_tabs: Vec<(u16, u16, Tab)>,
    pub flash: Option<(String, bool, std::time::Instant)>,
}

/// A row of the stack list: a stack, or the containers outside any stack.
pub enum StackRow<'a> {
    Stack(&'a Stack),
    Loose(Vec<usize>),
}

const DB_PG: [&str; 4] = ["postgres", "postgis", "timescale", "pgvecto"];
const DB_MY: [&str; 2] = ["mysql", "mariadb"];

impl App {
    pub fn new(cfg: Config, docker: Docker, tx: Sender<Msg>, poke: Sender<()>) -> Self {
        let pal = theme::palette(&cfg.theme);
        let host = cfg.url_host();
        Self {
            pal,
            docker,
            snap: Snapshot::default(),
            loaded: false,
            unreachable: None,
            live: HashMap::new(),
            tab: Tab::Stacks,
            sel: [0; 6],
            filter: Default::default(),
            mode: Mode::Normal,
            activity: VecDeque::new(),
            events: VecDeque::new(),
            chain: None,
            ops: Ops::new(tx.clone()),
            poke,
            streams: StatsStreams::default(),
            tx,
            df: None,
            host,
            hostname: config::hostname(),
            quit: false,
            foreground: None,
            log_seq: 1000,
            hit_rows: None,
            hit_tabs: Vec::new(),
            flash: None,
            cfg,
        }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.flash = Some((text.into(), false, std::time::Instant::now()));
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.flash = Some((text.into(), true, std::time::Instant::now()));
    }

    // ── Messages ────────────────────────────────────────────────────────

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Snapshot(s) => {
                self.snap = *s;
                self.loaded = true;
                self.unreachable = None;
                let running: HashSet<String> = self
                    .snap
                    .containers
                    .iter()
                    .filter(|c| c.running())
                    .map(|c| c.id.clone())
                    .collect();
                self.live.retain(|id, _| running.contains(id));
                self.streams.follow(&self.docker, &running, &self.tx);
                for (i, tab) in TABS.iter().enumerate() {
                    let n = self.row_count(*tab);
                    self.sel[i] = self.sel[i].min(n.saturating_sub(1));
                }
            }
            Msg::Unreachable(e) => self.unreachable = Some(e),
            Msg::Stats(id, s) => {
                let l = self.live.entry(id).or_default();
                l.last = s;
                l.cpu.push_back(s.cpu);
                while l.cpu.len() > 40 {
                    l.cpu.pop_front();
                }
            }
            Msg::Event(text) => {
                self.events.push_back((engine::now(), text));
                while self.events.len() > 300 {
                    self.events.pop_front();
                }
            }
            Msg::Line(id, line) => {
                if let Some(a) = self.activity.iter_mut().find(|a| a.id == id) {
                    a.last = line;
                }
            }
            Msg::Done(id, result) => {
                if let Some(a) = self.activity.iter_mut().find(|a| a.id == id) {
                    a.result = Some(result.clone());
                }
                let _ = self.poke.send(());
                self.chain_next(id, result);
            }
            Msg::Logs(id, batch) => {
                if let Mode::Logs(v) = &mut self.mode {
                    if v.follow.as_ref().is_some_and(|f| f.id == id) {
                        for l in batch {
                            v.lines.push_back(l);
                            if v.back > 0 {
                                v.back += 1;
                            }
                        }
                        while v.lines.len() > 20_000 {
                            v.lines.pop_front();
                        }
                    }
                }
            }
            Msg::DiskUsage(v) => {
                // Sizes per volume come with it.
                if let Some(vols) = v["Volumes"].as_array() {
                    for x in vols {
                        let name = x["Name"].as_str().unwrap_or("");
                        if let Some(vol) = self.snap.volumes.iter_mut().find(|v| v.name == name) {
                            vol.size = x["UsageData"]["Size"].as_i64().filter(|s| *s >= 0);
                        }
                    }
                }
                self.df = Some(v);
            }
        }
    }

    fn start(&mut self, label: impl Into<String>, id: u64) {
        self.activity.push_back(Activity {
            id,
            label: label.into(),
            last: String::new(),
            result: None,
            at: engine::now(),
        });
        while self.activity.len() > 60 {
            self.activity.pop_front();
        }
    }

    pub fn busy(&self) -> usize {
        self.activity.iter().filter(|a| a.result.is_none()).count()
    }

    fn run(&mut self, label: String, args: Vec<String>) {
        let id = self.ops.run(args);
        self.start(label, id);
    }

    fn api(&mut self, label: String, method: &'static str, path: String, timeout: u64) {
        let id = self.ops.api(&self.docker, method, path, timeout);
        self.start(label, id);
    }

    fn perform(&mut self, action: Action) {
        match action {
            Action::Run(label, args) => self.run(label, args),
            Action::Api(label, method, path, timeout) => self.api(label, method, path, timeout),
            Action::Chain(label, steps) => {
                self.chain = Some(Chain {
                    id: 0,
                    label,
                    steps: steps.into(),
                });
                self.chain_next(0, Ok(String::new()));
            }
        }
    }

    /// Start the next step of a chain once the previous one is done; stop
    /// at the first failure.
    fn chain_next(&mut self, finished: u64, result: Result<String, String>) {
        let Some(chain) = &mut self.chain else { return };
        if chain.id != finished {
            return;
        }
        if let Err(e) = result {
            let label = chain.label.clone();
            self.chain = None;
            self.warn(format!("{label} stopped: {e}"));
            return;
        }
        let Some(step) = chain.steps.pop_front() else {
            let label = chain.label.clone();
            self.chain = None;
            self.say(format!("{label}: done"));
            return;
        };
        let label = chain.label.clone();
        match step {
            Step::Run(args) => {
                let what = verb_of(&args);
                let id = self.ops.run(args);
                self.start(format!("{label}: {}", short(&what, 40)), id);
                if let Some(c) = &mut self.chain {
                    c.id = id;
                }
            }
            Step::Dump(args, file) => {
                let id = self.ops.dump_to(args, file.clone());
                self.start(format!("{label}: backup"), id);
                if let Some(c) = &mut self.chain {
                    c.id = id;
                }
            }
            Step::Trash(dir, root) => {
                let (_, _, _, stamp) = util::local(engine::now());
                let name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let to = root.join(".trash").join(format!("{name}-{stamp}"));
                let r = std::fs::create_dir_all(root.join(".trash"))
                    .and_then(|_| std::fs::rename(&dir, &to))
                    .map(|_| format!("moved to {}", to.display()))
                    .map_err(|e| e.to_string());
                self.start(format!("{label}: folder to trash"), 0);
                if let Some(a) = self.activity.back_mut() {
                    a.result = Some(r.clone());
                }
                if let Some(c) = &mut self.chain {
                    c.id = 0;
                }
                self.chain_next(0, r);
            }
            Step::Forget(name) => {
                config::forget(&name);
                if let Some(c) = &mut self.chain {
                    c.id = 0;
                }
                self.chain_next(0, Ok(String::new()));
            }
        }
    }

    // ── Rows ────────────────────────────────────────────────────────────

    fn matches(&self, tab: Tab, text: &str) -> bool {
        let f = &self.filter[tab.index()];
        f.is_empty() || text.to_lowercase().contains(&f.to_lowercase())
    }

    pub fn loose(&self) -> Vec<usize> {
        self.snap
            .containers
            .iter()
            .enumerate()
            .filter(|(_, c)| c.project.is_none())
            .map(|(i, _)| i)
            .collect()
    }

    pub fn stack_rows(&self) -> Vec<StackRow<'_>> {
        let mut rows: Vec<StackRow> = self
            .snap
            .stacks
            .iter()
            .filter(|s| {
                let path = s
                    .dir
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                self.matches(Tab::Stacks, &format!("{} {path}", s.name))
            })
            .map(StackRow::Stack)
            .collect();
        let loose = self.loose();
        if self.cfg.show_loose && !loose.is_empty() && self.matches(Tab::Stacks, "loose containers")
        {
            rows.push(StackRow::Loose(loose));
        }
        rows
    }

    pub fn container_rows(&self) -> Vec<usize> {
        self.snap
            .containers
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                self.matches(
                    Tab::Containers,
                    &format!(
                        "{} {} {}",
                        c.name,
                        c.image,
                        c.project.clone().unwrap_or_default()
                    ),
                )
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn image_rows(&self) -> Vec<usize> {
        (0..self.snap.images.len())
            .filter(|i| {
                let im = &self.snap.images[*i];
                self.matches(Tab::Images, &format!("{} {}", im.label(), im.short_id()))
            })
            .collect()
    }

    pub fn volume_rows(&self) -> Vec<usize> {
        (0..self.snap.volumes.len())
            .filter(|i| {
                let v = &self.snap.volumes[*i];
                self.matches(
                    Tab::Volumes,
                    &format!("{} {}", v.name, v.project.clone().unwrap_or_default()),
                )
            })
            .collect()
    }

    pub fn network_rows(&self) -> Vec<usize> {
        (0..self.snap.networks.len())
            .filter(|i| self.matches(Tab::Networks, &self.snap.networks[*i].name))
            .collect()
    }

    pub fn row_count(&self, tab: Tab) -> usize {
        match tab {
            Tab::Stacks => self.stack_rows().len(),
            Tab::Containers => self.container_rows().len(),
            Tab::Images => self.image_rows().len(),
            Tab::Volumes => self.volume_rows().len(),
            Tab::Networks => self.network_rows().len(),
            Tab::System => 0,
        }
    }

    pub fn selected(&self) -> usize {
        self.sel[self.tab.index()]
    }

    pub fn current_stack(&self) -> Option<&Stack> {
        match self.stack_rows().into_iter().nth(self.sel[0])? {
            StackRow::Stack(s) => Some(s),
            StackRow::Loose(_) => None,
        }
    }

    pub fn current_container(&self) -> Option<&Container> {
        let i = *self.container_rows().get(self.sel[1])?;
        self.snap.containers.get(i)
    }

    /// Containers of the stack row selected (or the loose ones).
    pub fn stack_row_containers(&self) -> Vec<usize> {
        match self.stack_rows().into_iter().nth(self.sel[0]) {
            Some(StackRow::Stack(s)) => s.containers.clone(),
            Some(StackRow::Loose(v)) => v,
            None => Vec::new(),
        }
    }

    pub fn links_of(&self, idx: &[usize]) -> Vec<Link> {
        let mut out: Vec<Link> = Vec::new();
        for i in idx {
            let c = &self.snap.containers[*i];
            if c.running() {
                for l in model::container_links(c, &self.host) {
                    let key = l.url.trim_end_matches('/');
                    if !out.iter().any(|o| o.url.trim_end_matches('/') == key) {
                        out.push(l);
                    }
                }
            }
        }
        out
    }

    pub fn live_sum(&self, idx: &[usize]) -> (f64, u64, VecDeque<f64>) {
        let mut cpu = 0.0;
        let mut mem = 0;
        let mut hist: VecDeque<f64> = VecDeque::new();
        for i in idx {
            if let Some(l) = self.live.get(&self.snap.containers[*i].id) {
                cpu += l.last.cpu;
                mem += l.last.mem;
                let n = l.cpu.len();
                for (k, v) in l.cpu.iter().enumerate() {
                    let at = 40usize.saturating_sub(n) + k;
                    while hist.len() <= at {
                        hist.push_back(0.0);
                    }
                    hist[at] += *v;
                }
            }
        }
        (cpu, mem, hist)
    }

    // ── Keys ────────────────────────────────────────────────────────────

    pub fn key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        self.mode = match mode {
            Mode::Normal => {
                self.normal_key(k);
                return;
            }
            Mode::Filter => self.filter_key(k),
            Mode::Help => match k.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter => {
                    Mode::Normal
                }
                _ => Mode::Help,
            },
            Mode::Confirm(c) => self.confirm_key(c, k),
            Mode::Uninstall(u) => self.uninstall_key(u, k),
            Mode::Pick(p) => self.pick_key(p, k),
            Mode::Logs(v) => self.logs_key(v, k),
            Mode::Text(t) => text_key(t, k),
        };
    }

    fn filter_key(&mut self, k: KeyEvent) -> Mode {
        let f = &mut self.filter[self.tab.index()];
        match k.code {
            KeyCode::Esc => {
                f.clear();
                Mode::Normal
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::Up => Mode::Normal,
            KeyCode::Backspace => {
                f.pop();
                self.sel[self.tab.index()] = 0;
                Mode::Filter
            }
            KeyCode::Char(c) => {
                f.push(c);
                self.sel[self.tab.index()] = 0;
                Mode::Filter
            }
            _ => Mode::Filter,
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.row_count(self.tab);
        let i = self.tab.index();
        if n == 0 {
            self.sel[i] = 0;
            return;
        }
        self.sel[i] = (self.sel[i] as isize + delta).clamp(0, n as isize - 1) as usize;
    }

    fn normal_key(&mut self, k: KeyEvent) {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.mode = Mode::Help,
            KeyCode::Tab => self.tab = TABS[(self.tab.index() + 1) % TABS.len()],
            KeyCode::BackTab => self.tab = TABS[(self.tab.index() + TABS.len() - 1) % TABS.len()],
            KeyCode::Char(c @ '1'..='6') => self.tab = TABS[(c as u8 - b'1') as usize],
            KeyCode::Left | KeyCode::Char('h') if !shift => {
                self.tab = TABS[(self.tab.index() + TABS.len() - 1) % TABS.len()]
            }
            KeyCode::Right => self.tab = TABS[(self.tab.index() + 1) % TABS.len()],
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-15),
            KeyCode::PageDown => self.move_sel(15),
            KeyCode::Home | KeyCode::Char('g') => self.sel[self.tab.index()] = 0,
            KeyCode::End | KeyCode::Char('G') => self.move_sel(isize::MAX / 2),
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Esc => self.filter[self.tab.index()].clear(),
            KeyCode::Char('R') | KeyCode::F(5) => {
                let _ = self.poke.send(());
                self.say("Refreshing");
            }
            KeyCode::Char('t') => {
                let i = theme::NAMES
                    .iter()
                    .position(|n| *n == self.cfg.theme)
                    .unwrap_or(0);
                self.cfg.theme = theme::NAMES[(i + 1) % theme::NAMES.len()].to_string();
                self.pal = theme::palette(&self.cfg.theme);
                self.cfg.save_theme();
                self.say(format!("Theme: {}", self.pal.name));
            }
            KeyCode::Char(c) => match self.tab {
                Tab::Stacks => self.stack_key(c),
                Tab::Containers => self.container_key(c),
                Tab::Images => self.image_key(c),
                Tab::Volumes => self.volume_key(c),
                Tab::Networks => self.network_key(c),
                Tab::System => self.system_key(c),
            },
            KeyCode::Enter => match self.tab {
                Tab::Stacks => self.stack_key('l'),
                Tab::Containers => self.container_key('l'),
                Tab::Images | Tab::Volumes | Tab::Networks => self.inspect(),
                Tab::System => {}
            },
            _ => {}
        }
    }

    fn ask(&mut self, title: &str, body: Vec<String>, action: Action, danger: bool) {
        self.mode = Mode::Confirm(Confirm {
            title: title.into(),
            body,
            action,
            danger,
            word: None,
            typed: String::new(),
        });
    }

    fn stack_key(&mut self, c: char) {
        let containers = self.stack_row_containers();
        match c {
            'o' | 'y' => {
                return self.pick_link(
                    &containers,
                    if c == 'o' {
                        PickFor::Open
                    } else {
                        PickFor::Copy
                    },
                )
            }
            'e' => return self.pick_shell(&containers),
            'l' if self.current_stack().is_none() => return,
            _ => {}
        }
        let Some(st) = self.current_stack().cloned() else {
            if !matches!(c, 'o' | 'y' | 'e') && "udsSrpUblEcxBfi".contains(c) {
                self.warn("Containers outside a stack: manage them under 2 CONTAINERS");
            }
            return;
        };
        let name = st.name.clone();
        let files = model::files_state(&st.files);
        let has_file = files == model::Files::Ok;
        let need_file = |app: &mut App| {
            if !has_file {
                app.warn(model::file_problem(&name, files));
            }
            has_file
        };
        match c {
            'u' => {
                if need_file(self) {
                    self.run(
                        format!("up {name}"),
                        engine::compose(&st, &["up", "-d", "--remove-orphans"]),
                    );
                }
            }
            'S' => self.run(format!("start {name}"), engine::compose(&st, &["start"])),
            's' => self.run(format!("stop {name}"), engine::compose(&st, &["stop"])),
            'r' => self.run(
                format!("restart {name}"),
                engine::compose(&st, &["restart"]),
            ),
            'p' => {
                if need_file(self) {
                    self.run(format!("pull {name}"), engine::compose(&st, &["pull"]));
                }
            }
            'U' => {
                if need_file(self) {
                    let pull = engine::compose(&st, &["pull"]);
                    let up = engine::compose(&st, &["up", "-d", "--remove-orphans"]);
                    self.perform(Action::Chain(
                        format!("update {name}"),
                        vec![Step::Run(pull), Step::Run(up)],
                    ));
                }
            }
            'b' => {
                if need_file(self) {
                    if st.services.iter().any(|s| s.build) {
                        let build = engine::compose(&st, &["build", "--pull"]);
                        let up = engine::compose(&st, &["up", "-d", "--remove-orphans"]);
                        self.perform(Action::Chain(
                            format!("build {name}"),
                            vec![Step::Run(build), Step::Run(up)],
                        ));
                    } else {
                        self.warn(format!(
                            "{name} builds nothing: every service uses an image"
                        ));
                    }
                }
            }
            'd' => self.ask(
                &format!("Take {name} down?"),
                vec![
                    "Stops and removes its containers and networks.".into(),
                    "Volumes, images and files are kept; u brings it back.".into(),
                ],
                Action::Run(
                    format!("down {name}"),
                    engine::compose(&st, &["down", "--remove-orphans"]),
                ),
                false,
            ),
            'l' => self.open_logs(LogSource::Stack(name.clone()), &st),
            'E' => {
                if let Some(f) = st.files.iter().find(|f| f.is_file()) {
                    let editor = std::env::var("VISUAL")
                        .or_else(|_| std::env::var("EDITOR"))
                        .unwrap_or_else(|_| "nano".into());
                    let mut args: Vec<String> =
                        editor.split_whitespace().map(str::to_string).collect();
                    args.push(f.display().to_string());
                    self.foreground = Some(Foreground::Command(args));
                }
            }
            'c' => {
                if need_file(self) {
                    let args = engine::compose(&st, &["config"]);
                    let out = std::process::Command::new(&args[0])
                        .args(&args[1..])
                        .output();
                    match out {
                        Ok(o) => {
                            let text = if o.status.success() {
                                String::from_utf8_lossy(&o.stdout).into_owned()
                            } else {
                                String::from_utf8_lossy(&o.stderr).into_owned()
                            };
                            self.mode = Mode::Text(TextView::new(
                                format!("{name} · docker compose config (resolved)"),
                                &text,
                            ));
                        }
                        Err(e) => self.warn(e.to_string()),
                    }
                }
            }
            'f' => {
                if let Some(d) = &st.dir {
                    let d = d.display().to_string();
                    match util::open(&d) {
                        Ok(()) => self.say(format!("Opened {d}")),
                        Err(_) => {
                            util::osc52(&d);
                            self.say(format!("{d} (copied)"));
                        }
                    }
                }
            }
            'x' => self.open_uninstall(&st),
            'B' => {
                let steps = self.backup_steps(&st);
                if steps.is_empty() {
                    self.warn(format!(
                        "{name}: no running PostgreSQL, MySQL or MariaDB service to dump"
                    ));
                } else {
                    self.perform(Action::Chain(format!("backup {name}"), steps));
                }
            }
            'i' => {
                let ids: Vec<String> = st
                    .containers
                    .iter()
                    .map(|i| self.snap.containers[*i].id.clone())
                    .collect();
                let mut text = String::new();
                for id in ids {
                    if let Ok(v) = self.docker.get(&format!("/containers/{id}/json")) {
                        text.push_str(&serde_json::to_string_pretty(&v).unwrap_or_default());
                        text.push('\n');
                    }
                }
                self.mode = Mode::Text(TextView::new(format!("{name} · inspect"), &text));
            }
            _ => {}
        }
    }

    pub fn db_kind(image: &str) -> Option<&'static str> {
        let i = image.to_lowercase();
        let last = i.rsplit('/').next().unwrap_or(&i);
        if DB_PG.iter().any(|p| last.starts_with(p)) {
            Some("postgres")
        } else if DB_MY.iter().any(|p| last.starts_with(p)) {
            Some("mysql")
        } else {
            None
        }
    }

    fn backup_steps(&self, st: &Stack) -> Vec<Step> {
        let (_, _, _, stamp) = util::local(engine::now());
        let mut steps = Vec::new();
        for i in &st.containers {
            let c = &self.snap.containers[*i];
            if !c.running() {
                continue;
            }
            let Some(svc) = &c.service else { continue };
            let script = match App::db_kind(&c.image) {
                Some("postgres") => "pg_dumpall -U \"${POSTGRES_USER:-postgres}\"",
                Some(_) => {
                    "p=\"${MARIADB_ROOT_PASSWORD:-$MYSQL_ROOT_PASSWORD}\"; d=mysqldump; command -v mariadb-dump >/dev/null 2>&1 && d=mariadb-dump; MYSQL_PWD=\"$p\" $d -uroot --all-databases --single-transaction"
                }
                None => continue,
            };
            let file = self
                .cfg
                .backups()
                .join(&st.name)
                .join(format!("{svc}-{stamp}.sql"));
            steps.push(Step::Dump(
                engine::compose(st, &["exec", "-T", svc, "sh", "-c", script]),
                file,
            ));
        }
        steps
    }

    fn open_uninstall(&mut self, st: &Stack) {
        let can_backup = !self.backup_steps(st).is_empty();
        let roots = self.cfg.roots();
        let can_trash = st
            .dir
            .as_ref()
            .is_some_and(|d| config::under_root(d, &roots).is_some());
        self.mode = Mode::Uninstall(Uninstall {
            stack: st.name.clone(),
            row: 0,
            backup: can_backup,
            volumes: false,
            images: false,
            trash: false,
            can_backup,
            can_trash,
            typed: String::new(),
        });
    }

    fn uninstall_key(&mut self, mut u: Uninstall, k: KeyEvent) -> Mode {
        match k.code {
            KeyCode::Esc => return Mode::Normal,
            KeyCode::Up => u.row = u.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => u.row = (u.row + 1).min(3),
            KeyCode::Char(' ') if u.typed.is_empty() => match u.row {
                0 => u.backup = u.can_backup && !u.backup,
                1 => u.volumes = !u.volumes,
                2 => u.images = !u.images,
                _ => u.trash = u.can_trash && !u.trash,
            },
            KeyCode::Backspace => {
                u.typed.pop();
            }
            KeyCode::Char(c) => u.typed.push(c),
            KeyCode::Enter => {
                if u.typed != u.stack {
                    self.warn("Type the stack's name to uninstall it");
                    return Mode::Uninstall(u);
                }
                let Some(st) = self.snap.stacks.iter().find(|s| s.name == u.stack).cloned() else {
                    return Mode::Normal;
                };
                let mut steps = Vec::new();
                if u.backup {
                    steps.extend(self.backup_steps(&st));
                }
                let mut down = vec!["down", "--remove-orphans"];
                if u.volumes {
                    down.push("--volumes");
                }
                if u.images {
                    down.extend(["--rmi", "all"]);
                }
                if !st.containers.is_empty() || model::files_state(&st.files) == model::Files::Ok {
                    steps.push(Step::Run(engine::compose(&st, &down)));
                } else {
                    // No file: remove the containers themselves.
                    for i in &st.containers {
                        let c = &self.snap.containers[*i];
                        steps.push(Step::Run(vec![
                            "docker".into(),
                            "rm".into(),
                            "-f".into(),
                            c.id.clone(),
                        ]));
                    }
                }
                if u.trash {
                    let roots = self.cfg.roots();
                    if let Some(dir) = &st.dir {
                        if let Some(root) = config::under_root(dir, &roots) {
                            steps.push(Step::Trash(dir.clone(), root));
                        }
                    }
                }
                steps.push(Step::Forget(st.name.clone()));
                self.perform(Action::Chain(format!("uninstall {}", st.name), steps));
                return Mode::Normal;
            }
            _ => {}
        }
        Mode::Uninstall(u)
    }

    fn confirm_key(&mut self, mut c: Confirm, k: KeyEvent) -> Mode {
        if let Some(word) = &c.word {
            match k.code {
                KeyCode::Esc => return Mode::Normal,
                KeyCode::Enter if c.typed == *word => {
                    self.perform(c.action);
                    return Mode::Normal;
                }
                KeyCode::Backspace => {
                    c.typed.pop();
                }
                KeyCode::Char(ch) => c.typed.push(ch),
                _ => {}
            }
            return Mode::Confirm(c);
        }
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.perform(c.action);
                Mode::Normal
            }
            KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => Mode::Normal,
            _ => Mode::Confirm(c),
        }
    }

    fn pick_link(&mut self, containers: &[usize], purpose: PickFor) {
        let links = self.links_of(containers);
        match links.len() {
            0 => self.warn("No link: nothing running publishes a web port or a proxy name"),
            1 => self.use_link(&links[0].url, purpose),
            _ => {
                self.mode = Mode::Pick(Picker {
                    title: if purpose == PickFor::Open {
                        "Open"
                    } else {
                        "Copy"
                    }
                    .into(),
                    items: links
                        .into_iter()
                        .map(|l| (format!("{}  ·  {}", l.url, l.from), l.url))
                        .collect(),
                    selected: 0,
                    purpose,
                })
            }
        }
    }

    fn use_link(&mut self, url: &str, purpose: PickFor) {
        if purpose == PickFor::Open {
            match util::open(url) {
                Ok(()) => return self.say(format!("Opening {url}")),
                Err(_) => {
                    util::osc52(url);
                    return self.say(format!("No desktop here: {url} copied to the clipboard"));
                }
            }
        }
        util::osc52(url);
        self.say(format!("Copied {url}"));
    }

    fn pick_shell(&mut self, containers: &[usize]) {
        let running: Vec<&Container> = containers
            .iter()
            .map(|i| &self.snap.containers[*i])
            .filter(|c| c.running())
            .collect();
        match running.len() {
            0 => self.warn("Nothing is running to open a shell in"),
            1 => {
                let id = running[0].id.clone();
                self.shell(&id);
            }
            _ => {
                self.mode = Mode::Pick(Picker {
                    title: "Shell in".into(),
                    items: running
                        .iter()
                        .map(|c| {
                            (
                                format!(
                                    "{}  ·  {}",
                                    c.service.clone().unwrap_or_else(|| c.name.clone()),
                                    c.image
                                ),
                                c.id.clone(),
                            )
                        })
                        .collect(),
                    selected: 0,
                    purpose: PickFor::Shell,
                })
            }
        }
    }

    fn shell(&mut self, id: &str) {
        self.foreground = Some(Foreground::Command(vec![
            "docker".into(),
            "exec".into(),
            "-it".into(),
            id.into(),
            "sh".into(),
            "-c".into(),
            "if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi".into(),
        ]));
    }

    fn pick_key(&mut self, mut p: Picker, k: KeyEvent) -> Mode {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Mode::Normal,
            KeyCode::Up | KeyCode::Char('k') => p.selected = p.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                p.selected = (p.selected + 1).min(p.items.len() - 1)
            }
            KeyCode::Char(c @ '1'..='9') => {
                let i = (c as u8 - b'1') as usize;
                if i < p.items.len() {
                    p.selected = i;
                    return self.pick_accept(p);
                }
            }
            KeyCode::Enter => return self.pick_accept(p),
            _ => {}
        }
        Mode::Pick(p)
    }

    fn pick_accept(&mut self, p: Picker) -> Mode {
        let value = p.items[p.selected].1.clone();
        match p.purpose {
            PickFor::Open | PickFor::Copy => self.use_link(&value, p.purpose),
            PickFor::Shell => self.shell(&value),
        }
        Mode::Normal
    }

    // ── Logs ────────────────────────────────────────────────────────────

    fn open_logs(&mut self, source: LogSource, st: &Stack) {
        let mut services = vec![String::new()];
        services.extend(
            st.containers
                .iter()
                .filter_map(|i| self.snap.containers[*i].service.clone())
                .collect::<std::collections::BTreeSet<_>>(),
        );
        let mut v = LogView {
            title: String::new(),
            lines: VecDeque::new(),
            back: 0,
            wrap: false,
            search: String::new(),
            searching: false,
            timestamps: false,
            services,
            service: 0,
            source,
            follow: None,
        };
        self.restart_logs(&mut v);
        self.mode = Mode::Logs(Box::new(v));
    }

    fn restart_logs(&mut self, v: &mut LogView) {
        v.follow = None;
        v.lines.clear();
        v.back = 0;
        self.log_seq += 1;
        let mut args: Vec<String>;
        match &v.source {
            LogSource::Stack(name) => {
                let Some(st) = self.snap.stacks.iter().find(|s| &s.name == name) else {
                    return;
                };
                let mut a = vec!["logs", "-f", "--tail", "500", "--no-color"];
                if v.timestamps {
                    a.push("--timestamps");
                }
                let svc = v.services.get(v.service).cloned().unwrap_or_default();
                args = engine::compose(st, &a);
                if !svc.is_empty() {
                    args.push(svc.clone());
                }
                v.title = if svc.is_empty() {
                    format!("{name} · all services")
                } else {
                    format!("{name} · {svc}")
                };
            }
            LogSource::Container(id, name) => {
                args = vec![
                    "docker".into(),
                    "logs".into(),
                    "-f".into(),
                    "--tail".into(),
                    "500".into(),
                ];
                if v.timestamps {
                    args.push("--timestamps".into());
                }
                args.push(id.clone());
                v.title = name.clone();
            }
        }
        match LogFollow::start(self.log_seq, args, self.tx.clone()) {
            Ok(f) => v.follow = Some(f),
            Err(e) => v.lines.push_back(format!("! {e}")),
        }
    }

    fn logs_key(&mut self, mut v: Box<LogView>, k: KeyEvent) -> Mode {
        if v.searching {
            match k.code {
                KeyCode::Esc => {
                    v.searching = false;
                    v.search.clear();
                }
                KeyCode::Enter => {
                    v.searching = false;
                    log_find(&mut v, true);
                }
                KeyCode::Backspace => {
                    v.search.pop();
                }
                KeyCode::Char(c) => v.search.push(c),
                _ => {}
            }
            return Mode::Logs(v);
        }
        let page = 20;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Mode::Normal,
            KeyCode::Up | KeyCode::Char('k') => {
                v.back = (v.back + 1).min(v.lines.len().saturating_sub(1))
            }
            KeyCode::Down | KeyCode::Char('j') => v.back = v.back.saturating_sub(1),
            KeyCode::PageUp => v.back = (v.back + page).min(v.lines.len().saturating_sub(1)),
            KeyCode::PageDown => v.back = v.back.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => v.back = v.lines.len().saturating_sub(1),
            KeyCode::End | KeyCode::Char('G') | KeyCode::Char('f') => v.back = 0,
            KeyCode::Char('w') => v.wrap = !v.wrap,
            KeyCode::Char('/') => {
                v.searching = true;
                v.search.clear();
            }
            KeyCode::Char('n') => log_find(&mut v, true),
            KeyCode::Char('N') => log_find(&mut v, false),
            KeyCode::Char('c') => {
                v.lines.clear();
                v.back = 0;
            }
            KeyCode::Char('t') => {
                v.timestamps = !v.timestamps;
                self.restart_logs(&mut v);
            }
            KeyCode::Char('s') if v.services.len() > 1 => {
                v.service = (v.service + 1) % v.services.len();
                self.restart_logs(&mut v);
            }
            _ => {}
        }
        Mode::Logs(v)
    }

    // ── Other tabs ──────────────────────────────────────────────────────

    fn container_key(&mut self, ch: char) {
        let Some(c) = self.current_container().cloned() else {
            return;
        };
        let (id, name) = (c.id.clone(), c.name.clone());
        let path = |op: &str| format!("/containers/{id}/{op}");
        match ch {
            'S' => self.api(format!("start {name}"), "POST", path("start"), 60),
            's' => self.api(format!("stop {name}"), "POST", path("stop"), 90),
            'r' => self.api(format!("restart {name}"), "POST", path("restart"), 90),
            'P' => {
                let op = if c.state == "paused" {
                    "unpause"
                } else {
                    "pause"
                };
                self.api(format!("{op} {name}"), "POST", path(op), 30);
            }
            'K' => self.ask(
                &format!("Kill {name}?"),
                vec!["Sends SIGKILL: the process gets no chance to save.".into()],
                Action::Api(format!("kill {name}"), "POST", path("kill"), 30),
                true,
            ),
            'D' => self.ask(
                &format!("Remove {name}?"),
                vec![
                    if c.running() {
                        "It is running: it is stopped first.".into()
                    } else {
                        "The container goes; its image and named volumes stay.".into()
                    },
                    c.project
                        .as_ref()
                        .map(|p| format!("It belongs to {p}: u on the stack makes it again."))
                        .unwrap_or_default(),
                ],
                Action::Api(
                    format!("remove {name}"),
                    "DELETE",
                    format!("/containers/{id}?force=1"),
                    60,
                ),
                true,
            ),
            'l' => {
                let st = Stack {
                    name: name.clone(),
                    dir: None,
                    files: vec![],
                    containers: vec![],
                    services: vec![],
                    config_error: None,
                };
                self.open_logs(LogSource::Container(id, name), &st);
            }
            'e' => {
                if c.running() {
                    self.shell(&id);
                } else {
                    self.warn(format!("{name} is not running"));
                }
            }
            'o' | 'y' => {
                let i = self.container_rows()[self.sel[1]];
                self.pick_link(
                    &[i],
                    if ch == 'o' {
                        PickFor::Open
                    } else {
                        PickFor::Copy
                    },
                );
            }
            'i' => self.inspect(),
            _ => {}
        }
    }

    fn inspect(&mut self) {
        let (title, path) = match self.tab {
            Tab::Containers => match self.current_container() {
                Some(c) => (c.name.clone(), format!("/containers/{}/json", c.id)),
                None => return,
            },
            Tab::Images => match self
                .image_rows()
                .get(self.sel[2])
                .map(|i| &self.snap.images[*i])
            {
                Some(im) => (im.label(), format!("/images/{}/json", im.id)),
                None => return,
            },
            Tab::Volumes => match self
                .volume_rows()
                .get(self.sel[3])
                .map(|i| &self.snap.volumes[*i])
            {
                Some(v) => (v.name.clone(), format!("/volumes/{}", enc(&v.name))),
                None => return,
            },
            Tab::Networks => match self
                .network_rows()
                .get(self.sel[4])
                .map(|i| &self.snap.networks[*i])
            {
                Some(n) => (n.name.clone(), format!("/networks/{}", n.id)),
                None => return,
            },
            _ => return,
        };
        match self.docker.get(&path) {
            Ok(v) => {
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                self.mode = Mode::Text(TextView::new(format!("{title} · inspect"), &text));
            }
            Err(e) => self.warn(e),
        }
    }

    fn image_key(&mut self, c: char) {
        let im = self
            .image_rows()
            .get(self.sel[2])
            .map(|i| self.snap.images[*i].clone());
        match c {
            'D' => {
                let Some(im) = im else { return };
                let body = if im.used_by > 0 {
                    vec![format!(
                        "{} container(s) use it: remove them first.",
                        im.used_by
                    )]
                } else {
                    vec![format!(
                        "{} · {}",
                        im.short_id(),
                        model::human_bytes(im.size)
                    )]
                };
                if im.used_by > 0 {
                    return self.warn(body[0].clone());
                }
                self.ask(
                    &format!("Remove image {}?", im.label()),
                    body,
                    Action::Api(
                        format!("remove image {}", im.label()),
                        "DELETE",
                        format!("/images/{}", im.id),
                        60,
                    ),
                    true,
                );
            }
            'p' => {
                let Some(im) = im else { return };
                match im.tags.first().filter(|t| !t.contains("<none>")) {
                    Some(tag) => self.run(
                        format!("pull {tag}"),
                        vec!["docker".into(), "pull".into(), tag.clone()],
                    ),
                    None => self.warn("An untagged image cannot be pulled"),
                }
            }
            'P' => {
                let n = self.snap.images.iter().filter(|i| i.dangling()).count();
                let size: i64 = self
                    .snap
                    .images
                    .iter()
                    .filter(|i| i.dangling())
                    .map(|i| i.size)
                    .sum();
                self.ask(
                    "Remove dangling images?",
                    vec![format!(
                        "{n} untagged image(s), about {}.",
                        model::human_bytes(size)
                    )],
                    Action::Api(
                        "prune dangling images".into(),
                        "POST",
                        format!("/images/prune?filters={}", enc(r#"{"dangling":["true"]}"#)),
                        600,
                    ),
                    false,
                );
            }
            'A' => {
                let unused: Vec<&model::Image> =
                    self.snap.images.iter().filter(|i| i.used_by == 0).collect();
                let size: i64 = unused.iter().map(|i| i.size).sum();
                self.ask(
                    "Remove every image no container uses?",
                    vec![
                        format!(
                            "{} image(s), about {}.",
                            unused.len(),
                            model::human_bytes(size)
                        ),
                        "Stopped stacks keep theirs only while their containers exist.".into(),
                    ],
                    Action::Api(
                        "prune unused images".into(),
                        "POST",
                        format!("/images/prune?filters={}", enc(r#"{"dangling":["false"]}"#)),
                        900,
                    ),
                    true,
                );
            }
            'i' => self.inspect(),
            _ => {}
        }
    }

    fn volume_key(&mut self, c: char) {
        let v = self
            .volume_rows()
            .get(self.sel[3])
            .map(|i| self.snap.volumes[*i].clone());
        match c {
            'D' => {
                let Some(v) = v else { return };
                if !v.used_by.is_empty() {
                    return self.warn(format!("{} is used by {}", v.name, v.used_by.join(", ")));
                }
                self.mode = Mode::Confirm(Confirm {
                    title: format!("Delete volume {}?", v.name),
                    body: vec![
                        "Its data is gone for good.".into(),
                        format!("Type {} to delete it.", v.name),
                    ],
                    action: Action::Api(
                        format!("delete volume {}", v.name),
                        "DELETE",
                        format!("/volumes/{}", enc(&v.name)),
                        60,
                    ),
                    danger: true,
                    word: Some(v.name.clone()),
                    typed: String::new(),
                });
            }
            'P' => {
                let unused: Vec<String> = self
                    .snap
                    .volumes
                    .iter()
                    .filter(|v| v.used_by.is_empty())
                    .map(|v| v.name.clone())
                    .collect();
                if unused.is_empty() {
                    return self.say("Every volume is in use");
                }
                let mut body = vec![format!(
                    "{} volume(s) no container uses, with their data:",
                    unused.len()
                )];
                body.extend(unused.iter().take(8).map(|n| format!("  {n}")));
                if unused.len() > 8 {
                    body.push(format!("  … and {} more", unused.len() - 8));
                }
                body.push(
                    "A stack that is down has no containers: its volumes are listed too.".into(),
                );
                body.push("Type prune to delete them.".into());
                self.mode = Mode::Confirm(Confirm {
                    title: "Delete unused volumes?".into(),
                    body,
                    action: Action::Api(
                        "prune volumes".into(),
                        "POST",
                        format!("/volumes/prune?filters={}", enc(r#"{"all":["true"]}"#)),
                        900,
                    ),
                    danger: true,
                    word: Some("prune".into()),
                    typed: String::new(),
                });
            }
            'z' => {
                self.ops.disk_usage(&self.docker);
                self.say("Measuring volume sizes…");
            }
            'i' => self.inspect(),
            _ => {}
        }
    }

    fn network_key(&mut self, c: char) {
        let n = self
            .network_rows()
            .get(self.sel[4])
            .map(|i| self.snap.networks[*i].clone());
        match c {
            'D' => {
                let Some(n) = n else { return };
                if n.builtin() {
                    return self.warn(format!("{} belongs to Docker", n.name));
                }
                if !n.containers.is_empty() {
                    return self.warn(format!("{} is used by {}", n.name, n.containers.join(", ")));
                }
                self.ask(
                    &format!("Remove network {}?", n.name),
                    vec![format!("{} · {}", n.driver, n.scope)],
                    Action::Api(
                        format!("remove network {}", n.name),
                        "DELETE",
                        format!("/networks/{}", n.id),
                        30,
                    ),
                    false,
                );
            }
            'P' => self.ask(
                "Remove unused networks?",
                vec!["Networks no container is connected to.".into()],
                Action::Api(
                    "prune networks".into(),
                    "POST",
                    "/networks/prune".into(),
                    120,
                ),
                false,
            ),
            'i' => self.inspect(),
            _ => {}
        }
    }

    fn system_key(&mut self, c: char) {
        match c {
            'z' => {
                self.ops.disk_usage(&self.docker);
                self.say("Measuring disk usage…");
            }
            'C' => self.ask(
                "Clear the build cache?",
                vec!["Later builds start from scratch.".into()],
                Action::Api(
                    "prune build cache".into(),
                    "POST",
                    "/build/prune?all=1".into(),
                    900,
                ),
                false,
            ),
            'P' => self.image_key('P'),
            'X' => {
                let mut steps = vec![
                    Step::Run(vec![
                        "docker".into(),
                        "container".into(),
                        "prune".into(),
                        "-f".into(),
                    ]),
                    Step::Run(vec![
                        "docker".into(),
                        "image".into(),
                        "prune".into(),
                        "-a".into(),
                        "-f".into(),
                    ]),
                    Step::Run(vec![
                        "docker".into(),
                        "network".into(),
                        "prune".into(),
                        "-f".into(),
                    ]),
                ];
                steps.push(Step::Run(vec![
                    "docker".into(),
                    "builder".into(),
                    "prune".into(),
                    "-a".into(),
                    "-f".into(),
                ]));
                self.mode = Mode::Confirm(Confirm {
                    title: "Clean up Docker?".into(),
                    body: vec![
                        "Removes stopped containers, every image no container uses,".into(),
                        "unused networks and the build cache. Volumes are kept.".into(),
                        "Stopped stacks lose their containers: u makes them again.".into(),
                        "Type prune to go ahead.".into(),
                    ],
                    action: Action::Chain("clean up".into(), steps),
                    danger: true,
                    word: Some("prune".into()),
                    typed: String::new(),
                });
            }
            _ => {}
        }
    }

    // ── Mouse ───────────────────────────────────────────────────────────

    pub fn mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollDown => match &mut self.mode {
                Mode::Logs(v) => v.back = v.back.saturating_sub(3),
                Mode::Text(t) => t.top = (t.top + 3).min(t.lines.len().saturating_sub(1)),
                Mode::Normal => self.move_sel(1),
                _ => {}
            },
            MouseEventKind::ScrollUp => match &mut self.mode {
                Mode::Logs(v) => v.back = (v.back + 3).min(v.lines.len().saturating_sub(1)),
                Mode::Text(t) => t.top = t.top.saturating_sub(3),
                Mode::Normal => self.move_sel(-1),
                _ => {}
            },
            MouseEventKind::Down(MouseButton::Left) => {
                if !matches!(self.mode, Mode::Normal) {
                    return;
                }
                if let Some((_, _, tab)) = self
                    .hit_tabs
                    .iter()
                    .find(|(x0, x1, _)| m.row == 1 && m.column >= *x0 && m.column < *x1)
                {
                    self.tab = *tab;
                    return;
                }
                if let Some((top, bottom, left, offset)) = self.hit_rows {
                    if m.row >= top && m.row < bottom && m.column >= left {
                        let i = offset + (m.row - top) as usize;
                        if i < self.row_count(self.tab) {
                            self.sel[self.tab.index()] = i;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn log_find(v: &mut LogView, older: bool) {
    if v.search.is_empty() || v.lines.is_empty() {
        return;
    }
    let needle = v.search.to_lowercase();
    let n = v.lines.len();
    let cur = n - 1 - v.back.min(n - 1);
    let hit = if older {
        (0..cur)
            .rev()
            .find(|i| v.lines[*i].to_lowercase().contains(&needle))
    } else {
        (cur + 1..n).find(|i| v.lines[*i].to_lowercase().contains(&needle))
    };
    if let Some(i) = hit {
        v.back = n - 1 - i;
    }
}

impl TextView {
    pub fn new(title: String, text: &str) -> Self {
        Self {
            title,
            lines: text.lines().map(engine::clean).collect(),
            top: 0,
            search: String::new(),
            searching: false,
        }
    }
}

fn text_key(mut t: TextView, k: KeyEvent) -> Mode {
    if t.searching {
        match k.code {
            KeyCode::Esc => {
                t.searching = false;
                t.search.clear();
            }
            KeyCode::Enter => {
                t.searching = false;
                text_find(&mut t);
            }
            KeyCode::Backspace => {
                t.search.pop();
            }
            KeyCode::Char(c) => t.search.push(c),
            _ => {}
        }
        return Mode::Text(t);
    }
    let last = t.lines.len().saturating_sub(1);
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => return Mode::Normal,
        KeyCode::Up | KeyCode::Char('k') => t.top = t.top.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => t.top = (t.top + 1).min(last),
        KeyCode::PageUp => t.top = t.top.saturating_sub(20),
        KeyCode::PageDown | KeyCode::Char(' ') => t.top = (t.top + 20).min(last),
        KeyCode::Home | KeyCode::Char('g') => t.top = 0,
        KeyCode::End | KeyCode::Char('G') => t.top = last,
        KeyCode::Char('/') => {
            t.searching = true;
            t.search.clear();
        }
        KeyCode::Char('n') => text_find(&mut t),
        KeyCode::Char('y') => {
            util::osc52(&t.lines.join("\n"));
        }
        _ => {}
    }
    Mode::Text(t)
}

fn text_find(t: &mut TextView) {
    if t.search.is_empty() {
        return;
    }
    let needle = t.search.to_lowercase();
    if let Some(i) =
        (t.top + 1..t.lines.len()).find(|i| t.lines[*i].to_lowercase().contains(&needle))
    {
        t.top = i;
    }
}

/// What a docker command does, without its paths and project flags.
pub fn verb_of(args: &[String]) -> String {
    let mut out = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        if matches!(a, "--project-directory" | "-p" | "-f") {
            i += 2;
            continue;
        }
        if a != "compose" {
            out.push(a);
        }
        i += 1;
    }
    out.join(" ")
}

pub fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn databases_are_recognised_by_image() {
        assert_eq!(App::db_kind("postgres:16-alpine"), Some("postgres"));
        assert_eq!(
            App::db_kind("ghcr.io/immich-app/postgres:14-vectorchord"),
            Some("postgres")
        );
        assert_eq!(
            App::db_kind("tensorchord/pgvecto-rs:pg14"),
            Some("postgres")
        );
        assert_eq!(App::db_kind("mariadb:11"), Some("mysql"));
        assert_eq!(App::db_kind("lscr.io/linuxserver/mariadb"), Some("mysql"));
        assert_eq!(App::db_kind("redis:7"), None);
        assert_eq!(short("abcdef", 4), "abc…");
        let args: Vec<String> = [
            "docker",
            "compose",
            "--project-directory",
            "/x",
            "-p",
            "web",
            "-f",
            "/x/c.yml",
            "down",
            "--volumes",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(verb_of(&args), "down --volumes");
    }

    #[test]
    fn log_search_walks_older_and_newer() {
        let mut v = LogView {
            title: String::new(),
            lines: ["a", "error 1", "b", "error 2", "c"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            back: 0,
            wrap: false,
            search: "error".into(),
            searching: false,
            timestamps: false,
            services: vec![],
            service: 0,
            source: LogSource::Stack(String::new()),
            follow: None,
        };
        log_find(&mut v, true);
        assert_eq!(v.back, 1, "the newest match first");
        log_find(&mut v, true);
        assert_eq!(v.back, 3);
        log_find(&mut v, false);
        assert_eq!(v.back, 1);
    }
}
