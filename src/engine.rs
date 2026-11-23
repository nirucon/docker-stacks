//! Work in the background: snapshots, live stats, Docker events and actions.
//!
//! The UI thread only draws. Everything that waits for Docker runs in a
//! thread and reports on one channel.
use crate::config::{self, Config};
use crate::docker::{enc, Docker};
use crate::model::{self, Container, Image, Network, ServiceDef, Stack, Volume};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub containers: Vec<Container>,
    pub stacks: Vec<Stack>,
    pub images: Vec<Image>,
    pub volumes: Vec<Volume>,
    pub networks: Vec<Network>,
    pub version: String,
    pub info: Value,
    pub took_ms: u128,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub cpu: f64,
    pub mem: u64,
    pub mem_limit: u64,
    pub rx: u64,
    pub tx: u64,
    pub read: u64,
    pub write: u64,
    pub pids: u64,
}

#[derive(Debug)]
pub enum Msg {
    Snapshot(Box<Snapshot>),
    Unreachable(String),
    Stats(String, Sample),
    Event(String),
    /// An action's output line.
    Line(u64, String),
    Done(u64, Result<String, String>),
    /// Lines for the log viewer with this id.
    Logs(u64, Vec<String>),
    DiskUsage(Value),
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `docker compose` for a stack, with its directory and files.
pub fn compose(stack: &Stack, args: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = vec!["docker".into(), "compose".into()];
    if let Some(dir) = &stack.dir {
        v.push("--project-directory".into());
        v.push(dir.display().to_string());
    }
    v.push("-p".into());
    v.push(stack.name.clone());
    for f in &stack.files {
        if f.is_file() {
            v.push("-f".into());
            v.push(f.display().to_string());
        }
    }
    v.extend(args.iter().map(|s| s.to_string()));
    v
}

fn files_key(stack: &Stack) -> String {
    stack
        .files
        .iter()
        .map(|f| {
            let m = std::fs::metadata(f)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("{}@{m}", f.display())
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn read_config(stack: &Stack) -> Result<Vec<ServiceDef>, String> {
    match model::files_state(&stack.files) {
        model::Files::Ok => {}
        model::Files::NoAccess => return Err("no permission to read the compose file".into()),
        model::Files::Missing => return Err("the compose file is gone".into()),
    }
    let args = compose(stack, &["config", "--format", "json"]);
    let out = Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("docker: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err
            .lines()
            .last()
            .unwrap_or("compose config failed")
            .to_string());
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    Ok(model::parse_config(&v))
}

/// The services of every stack's compose file, read again only when a
/// file changed.
#[derive(Default)]
struct ConfigCache {
    entries: HashMap<String, (String, Result<Vec<ServiceDef>, String>)>,
}

impl ConfigCache {
    fn fill(&mut self, stacks: &[Stack]) -> HashMap<String, Result<Vec<ServiceDef>, String>> {
        let todo: Vec<(&Stack, String)> = stacks
            .iter()
            .map(|s| (s, files_key(s)))
            .filter(|(s, k)| self.entries.get(&s.name).map(|e| &e.0) != Some(k))
            .collect();
        let results: Vec<(String, String, Result<Vec<ServiceDef>, String>)> =
            std::thread::scope(|sc| {
                let handles: Vec<_> = todo
                    .iter()
                    .map(|(s, k)| sc.spawn(move || (s.name.clone(), k.clone(), read_config(s))))
                    .collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
        for (name, key, r) in results {
            self.entries.insert(name, (key, r));
        }
        self.entries
            .iter()
            .map(|(k, (_, r))| (k.clone(), r.clone()))
            .collect()
    }
}

pub fn snapshot(
    docker: &Docker,
    cfg: &Config,
    cache: &mut ConfigCacheHandle,
) -> Result<Snapshot, String> {
    let start = Instant::now();
    let containers = model::parse_containers(&docker.get("/containers/json?all=1")?);
    let images = model::parse_images(&docker.get("/images/json")?, &containers);
    let volumes = model::parse_volumes(&docker.get("/volumes")?, &containers);
    let networks = model::parse_networks(&docker.get("/networks")?, &containers);
    let version = docker
        .get("/version")
        .ok()
        .and_then(|v| v["Version"].as_str().map(str::to_string))
        .unwrap_or_default();
    let info = docker.get("/info").unwrap_or(Value::Null);
    let found = model::scan(&cfg.roots());
    let remembered = config::load_known();
    let first = model::build_stacks(&containers, &found, &remembered, &HashMap::new());
    let services = cache.0.fill(&first);
    let stacks = model::build_stacks(&containers, &found, &remembered, &services);
    // Remember every project with files, so one taken down stays listed.
    let mut known: Vec<model::Known> = stacks
        .iter()
        .filter_map(|s| {
            Some(model::Known {
                name: s.name.clone(),
                dir: s.dir.clone()?,
                files: s.files.clone(),
            })
        })
        .filter(|k| k.files.iter().any(|f| f.is_file()))
        .collect();
    known.sort_by(|a, b| a.name.cmp(&b.name));
    if known != remembered {
        config::save_known(&known);
    }
    Ok(Snapshot {
        containers,
        stacks,
        images,
        volumes,
        networks,
        version,
        info,
        took_ms: start.elapsed().as_millis(),
    })
}

pub struct ConfigCacheHandle(ConfigCache);

impl ConfigCacheHandle {
    pub fn new() -> Self {
        Self(ConfigCache::default())
    }
}

/// Snapshots on request (and every `refresh` seconds).
pub fn refresher(docker: Docker, cfg: Config, tx: Sender<Msg>) -> Sender<()> {
    let (poke, rx) = channel::<()>();
    std::thread::spawn(move || {
        let mut cache = ConfigCacheHandle::new();
        loop {
            match snapshot(&docker, &cfg, &mut cache) {
                Ok(s) => {
                    if tx.send(Msg::Snapshot(Box::new(s))).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    if tx.send(Msg::Unreachable(e)).is_err() {
                        return;
                    }
                }
            }
            match rx.recv_timeout(Duration::from_secs(cfg.refresh)) {
                Ok(()) => {
                    // Several events at once make one refresh.
                    std::thread::sleep(Duration::from_millis(250));
                    while rx.try_recv().is_ok() {}
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    poke
}

fn summarise_event(v: &Value) -> Option<String> {
    let kind = v["Type"].as_str()?;
    let action = v["Action"].as_str()?;
    // Health checks and exec runs are noise.
    if action.starts_with("exec_")
        || (action.starts_with("health_status") && !action.contains("unhealthy"))
    {
        return None;
    }
    let attrs = &v["Actor"]["Attributes"];
    let name = attrs["name"]
        .as_str()
        .or_else(|| v["Actor"]["ID"].as_str())
        .unwrap_or("?");
    let name = if kind == "image" || name.len() < 40 {
        name.to_string()
    } else {
        name[..12].to_string()
    };
    Some(format!(
        "{kind} {name}: {}",
        action.split(':').next().unwrap_or(action)
    ))
}

/// Docker's event stream: every change refreshes the view at once.
pub fn events(docker: Docker, tx: Sender<Msg>, poke: Sender<()>) {
    std::thread::spawn(move || loop {
        let filters = enc(r#"{"type":["container","image","volume","network"]}"#);
        let r = docker.stream(&format!("/events?filters={filters}"), |v| {
            if let Some(text) = summarise_event(&v) {
                let _ = tx.send(Msg::Event(text));
            }
            poke.send(()).is_ok()
        });
        if let Err(e) = r {
            if tx.send(Msg::Event(format!("events: {e}"))).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(3));
        if poke.send(()).is_err() {
            return;
        }
    });
}

pub fn sample(v: &Value) -> Option<Sample> {
    let cpu = &v["cpu_stats"];
    let pre = &v["precpu_stats"];
    let total = cpu["cpu_usage"]["total_usage"].as_f64()?;
    let pre_total = pre["cpu_usage"]["total_usage"].as_f64().unwrap_or(0.0);
    let sys = cpu["system_cpu_usage"].as_f64().unwrap_or(0.0);
    let pre_sys = pre["system_cpu_usage"].as_f64().unwrap_or(0.0);
    let cpus = cpu["online_cpus"].as_f64().unwrap_or_else(|| {
        cpu["cpu_usage"]["percpu_usage"]
            .as_array()
            .map(|a| a.len() as f64)
            .unwrap_or(1.0)
    });
    let pct = if sys > pre_sys && total >= pre_total {
        (total - pre_total) / (sys - pre_sys) * cpus * 100.0
    } else {
        0.0
    };
    let mem = &v["memory_stats"];
    let usage = mem["usage"].as_u64().unwrap_or(0);
    let cache = mem["stats"]["inactive_file"]
        .as_u64()
        .or_else(|| mem["stats"]["total_inactive_file"].as_u64())
        .unwrap_or(0);
    let (mut rx, mut tx) = (0, 0);
    for n in v["networks"].as_object().into_iter().flatten() {
        rx += n.1["rx_bytes"].as_u64().unwrap_or(0);
        tx += n.1["tx_bytes"].as_u64().unwrap_or(0);
    }
    let (mut read, mut write) = (0, 0);
    for e in v["blkio_stats"]["io_service_bytes_recursive"]
        .as_array()
        .into_iter()
        .flatten()
    {
        match e["op"].as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("read") => read += e["value"].as_u64().unwrap_or(0),
            Some("write") => write += e["value"].as_u64().unwrap_or(0),
            _ => {}
        }
    }
    Some(Sample {
        cpu: pct,
        mem: usage.saturating_sub(cache),
        mem_limit: mem["limit"].as_u64().unwrap_or(0),
        rx,
        tx,
        read,
        write,
        pids: v["pids_stats"]["current"].as_u64().unwrap_or(0),
    })
}

/// One stats stream for each running container.
#[derive(Default)]
pub struct StatsStreams {
    running: HashMap<String, Arc<AtomicBool>>,
}

impl StatsStreams {
    pub fn follow(&mut self, docker: &Docker, ids: &HashSet<String>, tx: &Sender<Msg>) {
        self.running.retain(|id, stop| {
            let keep = ids.contains(id);
            if !keep {
                stop.store(true, Ordering::Relaxed);
            }
            keep
        });
        for id in ids {
            if self.running.contains_key(id) {
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            self.running.insert(id.clone(), stop.clone());
            let (docker, tx, id) = (docker.clone(), tx.clone(), id.clone());
            std::thread::spawn(move || {
                let _ = docker.stream(&format!("/containers/{id}/stats?stream=true"), |v| {
                    if let Some(s) = sample(&v) {
                        if tx.send(Msg::Stats(id.clone(), s)).is_err() {
                            return false;
                        }
                    }
                    !stop.load(Ordering::Relaxed)
                });
            });
        }
    }
}

/// A command or API call in the background, numbered.
pub struct Ops {
    next: u64,
    tx: Sender<Msg>,
}

/// Lines without terminal colours and carriage-return progress.
pub fn clean(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\r' {
            out.clear();
            continue;
        }
        if c == '\t' {
            out.push_str("    ");
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

impl Ops {
    pub fn new(tx: Sender<Msg>) -> Self {
        Self { next: 1, tx }
    }

    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Run a command, its output line by line.
    pub fn run(&mut self, args: Vec<String>) -> u64 {
        let id = self.id();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let child = Command::new(&args[0])
                .args(&args[1..])
                .env("COMPOSE_ANSI", "never")
                .env("COMPOSE_PROGRESS", "plain")
                .env("BUILDKIT_PROGRESS", "plain")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Msg::Done(id, Err(format!("{}: {e}", args[0]))));
                    return;
                }
            };
            let last = Arc::new(Mutex::new(String::new()));
            let mut readers = Vec::new();
            for pipe in [
                child
                    .stdout
                    .take()
                    .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
                child
                    .stderr
                    .take()
                    .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
            ]
            .into_iter()
            .flatten()
            {
                let (tx, last) = (tx.clone(), last.clone());
                readers.push(std::thread::spawn(move || {
                    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                        let line = clean(&line);
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Ok(mut l) = last.lock() {
                            *l = line.clone();
                        }
                        let _ = tx.send(Msg::Line(id, line));
                    }
                }));
            }
            for r in readers {
                let _ = r.join();
            }
            let status = child.wait();
            let last = last.lock().map(|l| l.clone()).unwrap_or_default();
            let result = match status {
                Ok(s) if s.success() => Ok(last),
                Ok(s) => Err(if last.is_empty() {
                    format!("exit {}", s.code().unwrap_or(-1))
                } else {
                    last
                }),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(Msg::Done(id, result));
        });
        id
    }

    /// An API call.
    pub fn api(
        &mut self,
        docker: &Docker,
        method: &'static str,
        path: String,
        timeout: u64,
    ) -> u64 {
        let id = self.id();
        let (tx, docker) = (self.tx.clone(), docker.clone());
        std::thread::spawn(move || {
            let r = docker.act(method, &path, timeout).map(|v| {
                // Prunes say what they reclaimed.
                match v["SpaceReclaimed"].as_i64() {
                    Some(n) => format!("reclaimed {}", model::human_bytes(n)),
                    None => String::new(),
                }
            });
            let _ = tx.send(Msg::Done(id, r));
        });
        id
    }

    /// A command whose output goes to a file (a database dump).
    pub fn dump_to(&mut self, args: Vec<String>, file: PathBuf) -> u64 {
        let id = self.id();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = (|| -> Result<String, String> {
                if let Some(d) = file.parent() {
                    std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
                }
                let out = {
                    use std::os::unix::fs::OpenOptionsExt;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&file)
                        .map_err(|e| format!("{}: {e}", file.display()))?
                };
                let res = Command::new(&args[0])
                    .args(&args[1..])
                    .stdin(Stdio::null())
                    .stdout(out)
                    .stderr(Stdio::piped())
                    .output()
                    .map_err(|e| e.to_string())?;
                let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                if !res.status.success() || size == 0 {
                    let _ = std::fs::remove_file(&file);
                    let err = String::from_utf8_lossy(&res.stderr);
                    return Err(err.lines().last().unwrap_or("the dump failed").to_string());
                }
                Ok(format!(
                    "{} ({})",
                    file.display(),
                    model::human_bytes(size as i64)
                ))
            })();
            let _ = tx.send(Msg::Done(id, r));
        });
        id
    }

    /// Disk usage per image, container, volume and build cache (slow).
    pub fn disk_usage(&mut self, docker: &Docker) {
        let (tx, docker) = (self.tx.clone(), docker.clone());
        std::thread::spawn(move || {
            if let Ok(v) = docker.get("/system/df") {
                let _ = tx.send(Msg::DiskUsage(v));
            }
        });
    }
}

/// A followed log: the process and its id.
pub struct LogFollow {
    pub id: u64,
    child: Child,
}

impl LogFollow {
    pub fn start(id: u64, args: Vec<String>, tx: Sender<Msg>) -> Result<Self, String> {
        let mut child = Command::new(&args[0])
            .args(&args[1..])
            .env("COMPOSE_ANSI", "never")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", args[0]))?;
        let buffer: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        for pipe in [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let buffer = buffer.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if let Ok(mut b) = buffer.lock() {
                        b.push_back(clean(&line));
                    }
                }
            });
        }
        // Lines go to the UI in batches, a few times a second.
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(120));
            let batch: Vec<String> = match buffer.lock() {
                Ok(mut b) => b.drain(..).collect(),
                Err(_) => return,
            };
            if !batch.is_empty() && tx.send(Msg::Logs(id, batch)).is_err() {
                return;
            }
            if Arc::strong_count(&buffer) == 1 {
                return;
            }
        });
        Ok(Self { id, child })
    }
}

impl Drop for LogFollow {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn channel_pair() -> (Sender<Msg>, Receiver<Msg>) {
    channel()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_stats_sample_has_cpu_and_memory_without_cache() {
        let v = json!({
            "cpu_stats":{"cpu_usage":{"total_usage":2_000_000_000u64},"system_cpu_usage":20_000_000_000u64,"online_cpus":4},
            "precpu_stats":{"cpu_usage":{"total_usage":1_000_000_000u64},"system_cpu_usage":10_000_000_000u64},
            "memory_stats":{"usage":300,"limit":1000,"stats":{"inactive_file":100}},
            "networks":{"eth0":{"rx_bytes":5,"tx_bytes":7}},
            "blkio_stats":{"io_service_bytes_recursive":[{"op":"read","value":9},{"op":"Write","value":3}]},
            "pids_stats":{"current":12}
        });
        let s = sample(&v).unwrap();
        assert!((s.cpu - 40.0).abs() < 1e-6);
        assert_eq!(
            (s.mem, s.mem_limit, s.rx, s.tx, s.read, s.write, s.pids),
            (200, 1000, 5, 7, 9, 3, 12)
        );
    }

    #[test]
    fn output_lines_lose_colours_and_progress() {
        assert_eq!(clean("\x1b[32mok\x1b[0m"), "ok");
        assert_eq!(clean("10%\r50%\r done"), " done");
    }

    #[test]
    fn compose_commands_name_the_project_and_files() {
        let dir = std::env::temp_dir().join(format!("ds-eng-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("compose.yml");
        std::fs::write(&f, "services: {}\n").unwrap();
        let st = Stack {
            name: "web".into(),
            dir: Some(dir.clone()),
            files: vec![f.clone(), dir.join("missing.yml")],
            containers: vec![],
            services: vec![],
            config_error: None,
        };
        let args = compose(&st, &["up", "-d"]);
        assert_eq!(args[..2], ["docker", "compose"]);
        assert!(args.windows(2).any(|w| w[0] == "-p" && w[1] == "web"));
        assert_eq!(
            args.iter().filter(|a| *a == "-f").count(),
            1,
            "a missing file is left out"
        );
        assert_eq!(args[args.len() - 2..], ["up", "-d"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn events_read_well() {
        let v = json!({"Type":"container","Action":"die","Actor":{"ID":"abc","Attributes":{"name":"web-1"}}});
        assert_eq!(summarise_event(&v).unwrap(), "container web-1: die");
        let h = json!({"Type":"container","Action":"health_status: healthy","Actor":{"Attributes":{"name":"x"}}});
        assert!(summarise_event(&h).is_none());
    }
}
