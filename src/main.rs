//! docker-stacks: Docker Compose stacks, containers, images, volumes and
//! networks in one fast terminal view, in the NIRUX style.
mod config;
mod docker;
mod engine;
mod model;
mod theme;
mod ui;
mod util;

use config::Config;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
docker-stacks                 the terminal view (? in it lists every key)
docker-stacks list [--json]   stacks with their state, services and links
docker-stacks up|down|stop|start|restart|pull|update|logs NAME
                              run that for a stack, in this terminal
docker-stacks links NAME      the links of a running stack
docker-stacks open NAME       open its first link
docker-stacks themes          the colour themes (theme = \"…\" in the config)
docker-stacks --version

Stacks are found from Docker itself (running and stopped projects), from
the folders in roots (default ~/Docker) and from projects seen before.
Config: ~/.config/docker-stacks/config.toml";

fn snapshot(cfg: &Config) -> Result<engine::Snapshot, String> {
    let d = docker::Docker::new()?;
    let mut cache = engine::ConfigCacheHandle::new();
    engine::snapshot(&d, cfg, &mut cache)
}

fn find<'a>(snap: &'a engine::Snapshot, name: &str) -> Result<&'a model::Stack, String> {
    snap.stacks
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("no stack called {name} (docker-stacks list shows them)"))
}

fn links(snap: &engine::Snapshot, st: &model::Stack, host: &str) -> Vec<model::Link> {
    let mut out: Vec<model::Link> = Vec::new();
    for i in &st.containers {
        let c = &snap.containers[*i];
        if c.running() {
            for l in model::container_links(c, host) {
                let key = l.url.trim_end_matches('/');
                if !out.iter().any(|o| o.url.trim_end_matches('/') == key) {
                    out.push(l);
                }
            }
        }
    }
    out
}

fn list(cfg: &Config, json: bool) -> Result<(), String> {
    let snap = snapshot(cfg)?;
    let host = cfg.url_host();
    let c = &snap.containers;
    if json {
        let v: Vec<serde_json::Value> = snap
            .stacks
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "state": s.state(c).label(),
                    "running": s.running(c),
                    "expected": s.expected(c).len(),
                    "dir": s.dir,
                    "files": s.files,
                    "links": links(&snap, s, &host).into_iter().map(|l| l.url).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(());
    }
    for s in &snap.stacks {
        let first = links(&snap, s, &host)
            .into_iter()
            .next()
            .map(|l| l.url)
            .unwrap_or_default();
        println!(
            "{:<24} {:<8} {:>2}/{:<2} {:<40} {}",
            s.name,
            s.state(c).label(),
            s.running(c),
            s.expected(c).len(),
            s.dir
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            first
        );
    }
    Ok(())
}

fn compose_cli(cfg: &Config, verb: &str, name: &str) -> Result<(), String> {
    let snap = snapshot(cfg)?;
    let st = find(&snap, name)?;
    let runs: Vec<Vec<String>> = match verb {
        "up" => vec![engine::compose(st, &["up", "-d", "--remove-orphans"])],
        "down" => vec![engine::compose(st, &["down", "--remove-orphans"])],
        "stop" | "start" | "restart" | "pull" => vec![engine::compose(st, &[verb])],
        "update" => vec![
            engine::compose(st, &["pull"]),
            engine::compose(st, &["up", "-d", "--remove-orphans"]),
        ],
        "logs" => vec![engine::compose(st, &["logs", "-f", "--tail", "200"])],
        _ => return Err(format!("unknown command {verb}")),
    };
    for args in runs {
        let status = std::process::Command::new(&args[0])
            .args(&args[1..])
            .status()
            .map_err(|e| format!("docker: {e}"))?;
        if !status.success() {
            return Err(format!("{verb} {name} failed"));
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    // Output piped into head or grep -q ends quietly when the reader stops,
    // as for any Unix tool, not with a panic about a broken pipe.
    // SAFETY: setting a signal disposition before any thread starts.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = Config::load();
    let result = match args.first().map(String::as_str) {
        None => ui::run(cfg),
        Some("-h" | "--help" | "help") => {
            println!("{HELP}");
            Ok(())
        }
        Some("-V" | "--version" | "version") => {
            println!("docker-stacks {VERSION}");
            Ok(())
        }
        // The 0.x Python version's flag.
        Some("list" | "--list" | "ls") => list(&cfg, args.iter().any(|a| a == "--json")),
        Some("themes") => {
            for t in theme::NAMES {
                let mark = if t == cfg.theme { "*" } else { " " };
                println!("{mark} {t:<10} {}", theme::palette(t).name);
            }
            Ok(())
        }
        Some(verb @ ("links" | "open")) => match args.get(1) {
            None => Err(format!("{verb} needs a stack name")),
            Some(name) => snapshot(&cfg).and_then(|snap| {
                let st = find(&snap, name)?;
                let l = links(&snap, st, &cfg.url_host());
                if l.is_empty() {
                    return Err(format!("{name} has no link (is it running?)"));
                }
                if verb == "open" {
                    util::open(&l[0].url)
                } else {
                    for x in l {
                        println!("{}  {}", x.url, x.from);
                    }
                    Ok(())
                }
            }),
        },
        Some(
            verb @ ("up" | "down" | "stop" | "start" | "restart" | "pull" | "update" | "logs"),
        ) => match args.get(1) {
            Some(name) => compose_cli(&cfg, verb, name),
            None => Err(format!("{verb} needs a stack name")),
        },
        Some(other) => Err(format!("unknown command {other}; docker-stacks --help")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("docker-stacks: {e}");
            ExitCode::FAILURE
        }
    }
}
