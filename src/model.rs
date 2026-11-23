//! What Docker has: stacks, containers, images, volumes and networks.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const COMPOSE_NAMES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    None,
    Starting,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub ip: String,
    pub private: u16,
    pub public: Option<u16>,
    pub proto: String,
}

#[derive(Clone, Debug)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
    /// running, exited, paused, restarting, created, dead, removing.
    pub state: String,
    /// "Up 3 hours (healthy)", "Exited (1) 2 minutes ago".
    pub status: String,
    pub health: Health,
    pub project: Option<String>,
    pub service: Option<String>,
    pub oneoff: bool,
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
    pub ports: Vec<Port>,
    pub created: i64,
    pub labels: BTreeMap<String, String>,
    pub volumes: Vec<String>,
    pub networks: Vec<String>,
}

impl Container {
    pub fn running(&self) -> bool {
        self.state == "running"
    }

    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(12)]
    }

    /// The exit code of a stopped container, from its status text.
    pub fn exit_code(&self) -> Option<i64> {
        let s = self.status.strip_prefix("Exited (")?;
        s.split(')').next()?.parse().ok()
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

pub fn parse_containers(v: &Value) -> Vec<Container> {
    let mut out = Vec::new();
    for c in v.as_array().into_iter().flatten() {
        let labels: BTreeMap<String, String> = c["Labels"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), s(v))).collect())
            .unwrap_or_default();
        let status = s(&c["Status"]);
        let health = if status.contains("(healthy)") {
            Health::Healthy
        } else if status.contains("(unhealthy)") {
            Health::Unhealthy
        } else if status.contains("health: starting") {
            Health::Starting
        } else {
            Health::None
        };
        let mut ports: Vec<Port> = c["Ports"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| Port {
                ip: s(&p["IP"]),
                private: p["PrivatePort"].as_u64().unwrap_or(0) as u16,
                public: p["PublicPort"].as_u64().map(|x| x as u16),
                proto: s(&p["Type"]),
            })
            .collect();
        ports.sort_by_key(|p| (p.public.unwrap_or(0), p.private));
        // IPv4 and IPv6 bindings of the same port are one port.
        ports.dedup_by(|a, b| a.public == b.public && a.private == b.private && a.proto == b.proto);
        let name = c["Names"][0]
            .as_str()
            .unwrap_or("")
            .trim_start_matches('/')
            .to_string();
        out.push(Container {
            id: s(&c["Id"]),
            name,
            image: s(&c["Image"]),
            image_id: s(&c["ImageID"]),
            state: s(&c["State"]),
            status,
            health,
            project: labels.get("com.docker.compose.project").cloned(),
            service: labels.get("com.docker.compose.service").cloned(),
            oneoff: labels
                .get("com.docker.compose.oneoff")
                .is_some_and(|v| v == "True"),
            working_dir: labels
                .get("com.docker.compose.project.working_dir")
                .cloned(),
            config_files: labels
                .get("com.docker.compose.project.config_files")
                .map(|f| f.split(',').map(str::to_string).collect())
                .unwrap_or_default(),
            ports,
            created: c["Created"].as_i64().unwrap_or(0),
            volumes: c["Mounts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["Type"] == "volume")
                .map(|m| s(&m["Name"]))
                .collect(),
            networks: c["NetworkSettings"]["Networks"]
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            labels,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Clone, Debug)]
pub struct Image {
    pub id: String,
    pub tags: Vec<String>,
    pub size: i64,
    pub created: i64,
    pub used_by: usize,
}

impl Image {
    pub fn dangling(&self) -> bool {
        self.tags.is_empty() || self.tags.iter().all(|t| t == "<none>:<none>")
    }

    pub fn label(&self) -> String {
        if self.dangling() {
            "<none>".into()
        } else {
            self.tags.join(", ")
        }
    }

    pub fn short_id(&self) -> &str {
        let id = self.id.trim_start_matches("sha256:");
        &id[..id.len().min(12)]
    }
}

pub fn parse_images(v: &Value, containers: &[Container]) -> Vec<Image> {
    let mut out: Vec<Image> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            let id = s(&i["Id"]);
            Image {
                used_by: containers.iter().filter(|c| c.image_id == id).count(),
                tags: i["RepoTags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(s)
                    .collect(),
                size: i["Size"].as_i64().unwrap_or(0),
                created: i["Created"].as_i64().unwrap_or(0),
                id,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.dangling()
            .cmp(&b.dangling())
            .then(a.label().cmp(&b.label()))
    });
    out
}

#[derive(Clone, Debug)]
pub struct Volume {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub project: Option<String>,
    pub used_by: Vec<String>,
    pub size: Option<i64>,
}

pub fn parse_volumes(v: &Value, containers: &[Container]) -> Vec<Volume> {
    let mut out: Vec<Volume> = v["Volumes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| {
            let name = s(&x["Name"]);
            Volume {
                used_by: containers
                    .iter()
                    .filter(|c| c.volumes.contains(&name))
                    .map(|c| c.name.clone())
                    .collect(),
                driver: s(&x["Driver"]),
                mountpoint: s(&x["Mountpoint"]),
                project: x["Labels"]["com.docker.compose.project"]
                    .as_str()
                    .map(str::to_string),
                size: x["UsageData"]["Size"].as_i64().filter(|s| *s >= 0),
                name,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Clone, Debug)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub project: Option<String>,
    pub containers: Vec<String>,
}

impl Network {
    pub fn builtin(&self) -> bool {
        matches!(self.name.as_str(), "bridge" | "host" | "none")
    }
}

pub fn parse_networks(v: &Value, containers: &[Container]) -> Vec<Network> {
    let mut out: Vec<Network> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| {
            let name = s(&n["Name"]);
            Network {
                id: s(&n["Id"]),
                containers: containers
                    .iter()
                    .filter(|c| c.networks.contains(&name))
                    .map(|c| c.name.clone())
                    .collect(),
                driver: s(&n["Driver"]),
                scope: s(&n["Scope"]),
                project: n["Labels"]["com.docker.compose.project"]
                    .as_str()
                    .map(str::to_string),
                name,
            }
        })
        .collect();
    out.sort_by(|a, b| a.builtin().cmp(&b.builtin()).then(a.name.cmp(&b.name)));
    out
}

/// A service as its compose file declares it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServiceDef {
    pub name: String,
    pub image: String,
    pub profiles: Vec<String>,
    pub ports: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub build: bool,
}

/// The services of `docker compose config --format json`.
pub fn parse_config(v: &Value) -> Vec<ServiceDef> {
    let mut out: Vec<ServiceDef> = v["services"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, d)| ServiceDef {
            name: name.clone(),
            image: s(&d["image"]),
            profiles: d["profiles"]
                .as_array()
                .into_iter()
                .flatten()
                .map(s)
                .collect(),
            ports: d["ports"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let published = p["published"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| p["published"].as_u64().map(|n| n.to_string()))?;
                    Some(format!("{published}:{}", p["target"].as_u64().unwrap_or(0)))
                })
                .collect(),
            labels: match &d["labels"] {
                Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), s(v))).collect(),
                Value::Array(a) => a
                    .iter()
                    .filter_map(|x| {
                        let t = x.as_str()?;
                        let (k, v) = t.split_once('=')?;
                        Some((k.to_string(), v.to_string()))
                    })
                    .collect(),
                _ => BTreeMap::new(),
            },
            build: !d["build"].is_null(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StackState {
    Running,
    Partial,
    Failed,
    Stopped,
    Down,
}

impl StackState {
    pub fn label(self) -> &'static str {
        match self {
            StackState::Running => "running",
            StackState::Partial => "partial",
            StackState::Failed => "failed",
            StackState::Stopped => "stopped",
            StackState::Down => "down",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Stack {
    pub name: String,
    pub dir: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    /// Indexes into the snapshot's containers (one-off runs left out).
    pub containers: Vec<usize>,
    /// From the compose file, once read.
    pub services: Vec<ServiceDef>,
    pub config_error: Option<String>,
}

impl Stack {
    /// The services that should run: declared ones without a profile, or
    /// what has containers when the file is unknown.
    pub fn expected(&self, all: &[Container]) -> BTreeSet<String> {
        if !self.services.is_empty() {
            return self
                .services
                .iter()
                .filter(|s| s.profiles.is_empty())
                .map(|s| s.name.clone())
                .collect();
        }
        self.containers
            .iter()
            .filter_map(|i| all[*i].service.clone())
            .collect()
    }

    pub fn running(&self, all: &[Container]) -> usize {
        self.containers
            .iter()
            .filter(|i| all[**i].running())
            .count()
    }

    pub fn state(&self, all: &[Container]) -> StackState {
        if self.containers.is_empty() {
            return StackState::Down;
        }
        let running: BTreeSet<String> = self
            .containers
            .iter()
            .map(|i| &all[*i])
            .filter(|c| c.running())
            .filter_map(|c| c.service.clone())
            .collect();
        // A one-shot job that finished (exit 0) did what it should.
        let done: BTreeSet<String> = self
            .containers
            .iter()
            .map(|i| &all[*i])
            .filter(|c| c.exit_code() == Some(0))
            .filter_map(|c| c.service.clone())
            .collect();
        let expected = self.expected(all);
        if running.is_empty() {
            let failed = self.containers.iter().any(|i| {
                all[*i]
                    .exit_code()
                    .is_some_and(|c| c != 0 && c != 137 && c != 143)
            });
            return if failed {
                StackState::Failed
            } else {
                StackState::Stopped
            };
        }
        if expected
            .iter()
            .all(|s| running.contains(s) || done.contains(s))
        {
            StackState::Running
        } else {
            StackState::Partial
        }
    }

    pub fn health(&self, all: &[Container]) -> Health {
        let hs: Vec<Health> = self.containers.iter().map(|i| all[*i].health).collect();
        if hs.contains(&Health::Unhealthy) {
            Health::Unhealthy
        } else if hs.contains(&Health::Starting) {
            Health::Starting
        } else if hs.contains(&Health::Healthy) {
            Health::Healthy
        } else {
            Health::None
        }
    }
}

/// Ports that do not answer a browser.
const NOT_WEB: [u16; 18] = [
    2375, 2376, 22, 25, 53, 110, 143, 465, 587, 993, 995, 1883, 3306, 5432, 5672, 6379, 11211,
    27017,
];

/// A link with where it came from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Link {
    pub url: String,
    pub from: String,
}

/// Hosts in a router rule: Host(`a`) || Host(`b`, `c`).
pub fn rule_hosts(rule: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = rule;
    while let Some(i) = rest.find("Host(") {
        rest = &rest[i + 5..];
        let end = rest.find(')').unwrap_or(rest.len());
        for part in rest[..end].split(',') {
            let h = part
                .trim()
                .trim_matches(|c| c == '`' || c == '"' || c == '\'');
            if !h.is_empty() && !h.contains('{') {
                out.push(h.to_string());
            }
        }
        rest = &rest[end..];
    }
    out
}

/// Links from labels: docker-stacks.url, Traefik router rules, Caddy.
pub fn label_links(labels: &BTreeMap<String, String>, from: &str) -> Vec<Link> {
    let mut out = Vec::new();
    for (k, v) in labels {
        if k == "docker-stacks.url" || k.starts_with("docker-stacks.url.") {
            for u in v.split(',') {
                out.push(Link {
                    url: u.trim().to_string(),
                    from: from.into(),
                });
            }
        } else if k.starts_with("traefik.http.routers.") && k.ends_with(".rule") {
            for h in rule_hosts(v) {
                out.push(Link {
                    url: format!("https://{h}"),
                    from: format!("{from} (Traefik)"),
                });
            }
        } else if k == "caddy" || (k.starts_with("caddy_") && !k.contains('.')) {
            for h in v.split([' ', ',']).filter(|h| !h.is_empty()) {
                let url = if h.starts_with("http") {
                    h.to_string()
                } else {
                    format!("https://{}", h.trim_start_matches(':'))
                };
                out.push(Link {
                    url,
                    from: format!("{from} (Caddy)"),
                });
            }
        }
    }
    out
}

/// Links to a running container: labels, then published TCP ports.
pub fn container_links(c: &Container, host: &str) -> Vec<Link> {
    let mut out = label_links(&c.labels, c.service.as_deref().unwrap_or(&c.name));
    for p in &c.ports {
        let Some(public) = p.public else { continue };
        if p.proto != "tcp" || NOT_WEB.contains(&p.private) {
            continue;
        }
        let ip = match p.ip.as_str() {
            "" | "0.0.0.0" | "::" => host.to_string(),
            ip if ip.contains(':') => format!("[{ip}]"),
            ip => ip.to_string(),
        };
        let scheme = if matches!(p.private, 443 | 8443 | 9443) {
            "https"
        } else {
            "http"
        };
        out.push(Link {
            url: format!("{scheme}://{ip}:{public}"),
            from: c.service.clone().unwrap_or_else(|| c.name.clone()),
        });
    }
    out.dedup();
    out
}

/// Normalise a directory name the way Compose does.
pub fn project_name_from_dir(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// `name:` at the top of a compose file, else COMPOSE_PROJECT_NAME in .env,
/// else the directory.
pub fn read_project_name(compose: &Path, dir: &Path) -> String {
    if let Ok(text) = std::fs::read_to_string(compose) {
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("name:") {
                let v = v
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(['"', '\'']);
                if !v.is_empty() && !v.contains('$') {
                    return v.to_string();
                }
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(dir.join(".env")) {
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix("COMPOSE_PROJECT_NAME=") {
                let v = v.trim().trim_matches(['"', '\'']);
                if !v.is_empty() {
                    return v.to_string();
                }
            }
        }
    }
    project_name_from_dir(dir)
}

/// Whether a stack's compose files can be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Files {
    Ok,
    /// They exist but this user may not read them (a root-owned folder).
    NoAccess,
    Missing,
}

pub fn files_state(files: &[PathBuf]) -> Files {
    let mut state = Files::Missing;
    for f in files {
        match std::fs::File::open(f) {
            Ok(_) => return Files::Ok,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => state = Files::NoAccess,
            Err(_) => {}
        }
    }
    state
}

pub fn file_problem(name: &str, state: Files) -> String {
    match state {
        Files::Ok => String::new(),
        Files::NoAccess => format!(
            "{name}: no permission to read its compose file; stop, start, restart, down and logs still work"
        ),
        Files::Missing => format!("{name}: its compose file is gone; stop, start, restart, down and logs still work"),
    }
}

pub fn compose_file_in(dir: &Path) -> Option<PathBuf> {
    COMPOSE_NAMES
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

/// A project found on disk or remembered: name, directory, files.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Known {
    pub name: String,
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

/// Projects in the scan roots: every directory (one level, and the root
/// itself) with a compose file.
pub fn scan(roots: &[PathBuf]) -> Vec<Known> {
    let mut out = Vec::new();
    for root in roots {
        let mut dirs = vec![root.clone()];
        if let Ok(rd) = std::fs::read_dir(root) {
            let mut sub: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| {
                    !p.file_name()
                        .map(|n| n.to_string_lossy().starts_with('.'))
                        .unwrap_or(true)
                })
                .collect();
            sub.sort();
            dirs.extend(sub);
        }
        for dir in dirs {
            if let Some(file) = compose_file_in(&dir) {
                out.push(Known {
                    name: read_project_name(&file, &dir),
                    dir,
                    files: vec![file],
                });
            }
        }
    }
    out
}

/// Stacks from running and stopped containers, the projects on disk and the
/// ones remembered from earlier runs (so a project taken down stays in
/// the list).
pub fn build_stacks(
    containers: &[Container],
    found: &[Known],
    remembered: &[Known],
    services: &HashMap<String, Result<Vec<ServiceDef>, String>>,
) -> Vec<Stack> {
    let mut map: BTreeMap<String, Stack> = BTreeMap::new();
    for (i, c) in containers.iter().enumerate() {
        let Some(p) = &c.project else { continue };
        let st = map.entry(p.clone()).or_insert_with(|| Stack {
            name: p.clone(),
            dir: None,
            files: Vec::new(),
            containers: Vec::new(),
            services: Vec::new(),
            config_error: None,
        });
        if !c.oneoff {
            st.containers.push(i);
        }
        if st.dir.is_none() {
            st.dir = c.working_dir.as_ref().map(PathBuf::from);
        }
        if st.files.is_empty() {
            st.files = c.config_files.iter().map(PathBuf::from).collect();
        }
    }
    for k in found.iter().chain(remembered) {
        let st = map.entry(k.name.clone()).or_insert_with(|| Stack {
            name: k.name.clone(),
            dir: None,
            files: Vec::new(),
            containers: Vec::new(),
            services: Vec::new(),
            config_error: None,
        });
        if st.dir.is_none() {
            st.dir = Some(k.dir.clone());
        }
        if st.files.is_empty() {
            st.files = k.files.clone();
        }
    }
    let mut out: Vec<Stack> = map.into_values().collect();
    // A remembered project whose files are gone and that has no containers
    // left is not a stack any more.
    out.retain(|s| !s.containers.is_empty() || s.files.iter().any(|f| f.is_file()));
    for st in &mut out {
        match services.get(&st.name) {
            Some(Ok(defs)) => st.services = defs.clone(),
            Some(Err(e)) => st.config_error = Some(e.clone()),
            None => {}
        }
    }
    out
}

pub fn human_bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    if n >= 1e12 {
        format!("{:.1}T", n / 1e12)
    } else if n >= 1e9 {
        format!("{:.1}G", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.0}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{n}B")
    }
}

pub fn ago(epoch: i64, now: i64) -> String {
    let d = (now - epoch).max(0);
    if d < 60 {
        format!("{d}s")
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86400 {
        format!("{}h", d / 3600)
    } else if d < 86400 * 60 {
        format!("{}d", d / 86400)
    } else {
        format!("{}mo", d / (86400 * 30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn containers() -> Vec<Container> {
        parse_containers(&json!([
            {"Id":"aaaaaaaaaaaaaaaa","Names":["/web-1"],"Image":"nginx","ImageID":"sha256:1","State":"running",
             "Status":"Up 2 hours (healthy)","Created":1,
             "Labels":{"com.docker.compose.project":"web","com.docker.compose.service":"app",
                       "com.docker.compose.project.working_dir":"/srv/web",
                       "com.docker.compose.project.config_files":"/srv/web/compose.yml",
                       "traefik.http.routers.app.rule":"Host(`web.example.org`) || Host(`www.example.org`)"},
             "Ports":[{"IP":"0.0.0.0","PrivatePort":80,"PublicPort":8080,"Type":"tcp"},
                      {"IP":"::","PrivatePort":80,"PublicPort":8080,"Type":"tcp"},
                      {"PrivatePort":443,"Type":"tcp"}],
             "Mounts":[{"Type":"volume","Name":"web_data"},{"Type":"bind","Source":"/x"}],
             "NetworkSettings":{"Networks":{"web_default":{}}}},
            {"Id":"bbbb","Names":["/web-db-1"],"Image":"postgres:16","ImageID":"sha256:2","State":"exited",
             "Status":"Exited (1) 5 minutes ago","Created":1,
             "Labels":{"com.docker.compose.project":"web","com.docker.compose.service":"db"},
             "Ports":[{"IP":"127.0.0.1","PrivatePort":5432,"PublicPort":5432,"Type":"tcp"}]},
            {"Id":"cccc","Names":["/loose"],"Image":"redis","ImageID":"sha256:3","State":"running",
             "Status":"Up 1 minute","Created":1,"Labels":{},"Ports":[]}
        ]))
    }

    #[test]
    fn containers_are_read_with_health_ports_and_volumes() {
        let c = containers();
        let web = c.iter().find(|c| c.name == "web-1").unwrap();
        assert_eq!(web.health, Health::Healthy);
        assert_eq!(web.ports.len(), 2, "IPv4 and IPv6 of one port are one");
        assert_eq!(web.volumes, vec!["web_data"]);
        assert_eq!(web.networks, vec!["web_default"]);
        let db = c.iter().find(|c| c.name == "web-db-1").unwrap();
        assert_eq!(db.exit_code(), Some(1));
    }

    #[test]
    fn stacks_come_from_labels_disk_and_memory() {
        let c = containers();
        let dir = std::env::temp_dir().join(format!("ds-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("compose.yml");
        std::fs::write(&file, "name: blog\nservices:\n  a:\n    image: x\n").unwrap();
        let found = scan(std::slice::from_ref(&dir));
        assert_eq!(found[0].name, "blog");
        let gone = Known {
            name: "old".into(),
            dir: PathBuf::from("/nowhere"),
            files: vec![PathBuf::from("/nowhere/compose.yml")],
        };
        let mut services = HashMap::new();
        services.insert(
            "web".to_string(),
            Ok(vec![
                ServiceDef {
                    name: "app".into(),
                    ..Default::default()
                },
                ServiceDef {
                    name: "db".into(),
                    ..Default::default()
                },
                ServiceDef {
                    name: "tools".into(),
                    profiles: vec!["debug".into()],
                    ..Default::default()
                },
            ]),
        );
        let stacks = build_stacks(&c, &found, &[gone], &services);
        let names: Vec<&str> = stacks.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["blog", "web"], "a vanished project is dropped");
        let web = &stacks[1];
        assert_eq!(web.dir, Some(PathBuf::from("/srv/web")));
        assert_eq!(
            web.expected(&c).len(),
            2,
            "profile services are not required"
        );
        assert_eq!(web.state(&c), StackState::Partial);
        assert_eq!(stacks[0].state(&c), StackState::Down);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_come_from_ports_and_proxies() {
        let c = containers();
        let web = &c.iter().find(|c| c.name == "web-1").unwrap();
        let urls: Vec<String> = container_links(web, "ygg")
            .into_iter()
            .map(|l| l.url)
            .collect();
        assert!(urls.contains(&"http://ygg:8080".to_string()));
        assert!(urls.contains(&"https://web.example.org".to_string()));
        assert!(urls.contains(&"https://www.example.org".to_string()));
        let db = &c.iter().find(|c| c.name == "web-db-1").unwrap();
        assert!(
            container_links(db, "ygg").is_empty(),
            "a database port is no link"
        );
        let mut l = BTreeMap::new();
        l.insert("caddy".to_string(), "photos.example.org".to_string());
        l.insert(
            "docker-stacks.url".to_string(),
            "http://nas:2283/admin".to_string(),
        );
        let urls: Vec<String> = label_links(&l, "x").into_iter().map(|l| l.url).collect();
        assert_eq!(
            urls,
            vec!["https://photos.example.org", "http://nas:2283/admin"]
        );
    }

    #[test]
    fn a_compose_config_gives_services() {
        let v = json!({"services":{
            "web":{"image":"nginx","ports":[{"published":"8080","target":80}],"labels":{"a":"b"}},
            "dbg":{"image":"busybox","profiles":["debug"],"labels":["x=y"]},
            "api":{"build":{"context":"."}}}});
        let defs = parse_config(&v);
        assert_eq!(
            defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["api", "dbg", "web"]
        );
        assert!(defs[0].build);
        assert_eq!(defs[1].labels.get("x").map(String::as_str), Some("y"));
        assert_eq!(defs[2].ports, vec!["8080:80"]);
    }

    #[test]
    fn project_names_follow_compose() {
        assert_eq!(project_name_from_dir(Path::new("/x/My App.v2")), "myappv2");
        assert_eq!(rule_hosts("Host(`a.b`) && PathPrefix(`/x`)"), vec!["a.b"]);
        assert_eq!(human_bytes(1_500_000_000), "1.5G");
        assert_eq!(ago(0, 7200), "2h");
    }
}
