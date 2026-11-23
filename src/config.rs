//! `~/.config/docker-stacks/config.toml` and the remembered projects.
use crate::model::Known;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
}

fn state_home() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/state"))
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/share"))
}

pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Folders whose subfolders hold compose projects.
    pub roots: Vec<String>,
    /// nirux, amber, phosphor, c64, paper or native.
    pub theme: String,
    /// The host in links to published ports; empty: localhost, or this
    /// machine's name over SSH.
    pub url_host: String,
    /// Where database dumps go.
    pub backups: String,
    /// Seconds between quiet refreshes (Docker's events refresh at once).
    pub refresh: u64,
    /// Show containers that do not belong to a stack in the stack list.
    pub show_loose: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roots: vec!["~/Docker".into()],
            theme: "nirux".into(),
            url_host: String::new(),
            backups: "~/.local/share/docker-stacks/backups".into(),
            refresh: 10,
            show_loose: true,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        config_home().join("docker-stacks/config.toml")
    }

    pub fn load() -> Self {
        let mut c: Config = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        // The 0.x Python version read DOCKER_STACKS_DIR.
        if let Ok(dir) = std::env::var("DOCKER_STACKS_DIR") {
            if !dir.is_empty() && !c.roots.contains(&dir) {
                c.roots.insert(0, dir);
            }
        }
        c.refresh = c.refresh.clamp(2, 600);
        c
    }

    /// Keep the theme (the only thing the program changes itself).
    pub fn save_theme(&self) {
        let path = Self::path();
        let mut v: toml::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or_else(|| toml::Value::Table(Default::default()));
        if let Some(t) = v.as_table_mut() {
            t.insert("theme".into(), toml::Value::String(self.theme.clone()));
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string_pretty(&v) {
            let tmp = path.with_extension("toml.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .map(|r| expand(r))
            .filter(|p| p.is_dir())
            .collect()
    }

    pub fn backups(&self) -> PathBuf {
        let p = expand(&self.backups);
        if p.is_absolute() {
            p
        } else {
            data_home().join("docker-stacks/backups")
        }
    }

    pub fn url_host(&self) -> String {
        if !self.url_host.is_empty() {
            return self.url_host.clone();
        }
        if std::env::var_os("SSH_CONNECTION").is_some() {
            return hostname();
        }
        "localhost".into()
    }
}

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid and its length is passed.
    let r = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if r != 0 {
        return "localhost".into();
    }
    let n = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

fn known_path() -> PathBuf {
    state_home().join("docker-stacks/known.json")
}

pub fn load_known() -> Vec<Known> {
    std::fs::read_to_string(known_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_known(known: &[Known]) {
    let path = known_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(known) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Forget a project (after an uninstall).
pub fn forget(name: &str) {
    let mut k = load_known();
    k.retain(|x| x.name != name);
    save_known(&k);
}

/// Whether `dir` is directly inside one of the roots (so moving it to the
/// trash is ours to do).
pub fn under_root(dir: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let parent = dir.parent()?;
    let meta = std::fs::symlink_metadata(dir).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    roots
        .iter()
        .find(|r| r.canonicalize().ok() == parent.canonicalize().ok())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_has_safe_defaults_and_reads_overrides() {
        let c: Config = toml::from_str("theme = \"amber\"\nrefresh = 1\n").unwrap();
        assert_eq!(c.theme, "amber");
        assert_eq!(c.roots, vec!["~/Docker"]);
        assert_eq!(expand("~/x"), home().join("x"));
        let dir = std::env::temp_dir().join(format!("ds-root-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        assert_eq!(
            under_root(&dir.join("proj"), std::slice::from_ref(&dir)),
            Some(dir.clone())
        );
        assert_eq!(under_root(&dir, std::slice::from_ref(&dir)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
