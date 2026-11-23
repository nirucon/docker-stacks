# docker-stacks

A fast terminal manager for Docker Compose stacks, and for everything
around them: containers, images, volumes, networks and the disk they use.
One static binary, no daemon, no web server. It talks to Docker over its
socket, updates the moment Docker changes something, and works the same at
the desk and over SSH on a server.

Part of the [NIRUX](https://github.com/nirucon/nirux) family: it wears the
desktop's NIRU theme, or a retro one (amber, phosphor, C64, paper).

```
▓▒░ DOCKER·STACKS ░▒▓ yggdrasil  docker 29.1   ● 15 up  ○ 0 stopped  ▤ 6 stacks     cpu 5.2%  mem 1.9G  22:17
  1 STACKS   2 CONTAINERS   3 IMAGES   4 VOLUMES   5 NETWORKS   6 SYSTEM
╔ STACKS · 7 ════════════════════════════════════════════╗┌ STACK ──────────────────────────────┐
║  NAME                  UP       CPU    MEM  ACTIVITY    ║│● IMMICH running                     │
║ ● audiobookshelf       1/1      0.0%    75M             ║│─ SERVICES ─                         │
║ ● gitea                2/2      0.3%   129M         ▁▁▁ ║│● database   Up 4 weeks ♥  0.0% 352M │
║ ● immich               4/4 ♥    0.7%   1.2G         ▄▄▂ ║│● server     Up 4 weeks ♥  0.1% 803M │
║ ◇ outside any stack    5/5      3.9%   390M        ▆▇▆  ║│─ LINKS  o open · y copy ─           │
╚═════════════════════════════════════════════════════════╝│↗ http://yggdrasil:2283              │
```

## What it does

**Stacks.** Every Compose project Docker knows, running or stopped, wherever
its files are, plus the projects in your stack folders (default `~/Docker`)
and the ones taken down earlier. For each: whether all its services run
(a finished one-shot job counts as done, a service behind a profile is not
required), health, live CPU and memory with a small history, its links, its
volumes.

- `u` up, `S` start, `s` stop, `r` restart, `d` down (volumes kept).
- `p` pull, `U` update (pull, then up), `b` build and up.
- `l` logs, following, for all services or one (`s` in the log view cycles
  them), with search, wrap and timestamps.
- `e` a shell in a service (bash if it has one).
- `o` open a link in the browser, `y` copy it. Over SSH, where there is no
  browser, the link goes to your terminal's clipboard.
- `E` edit the compose file in `$EDITOR`, `c` the resolved configuration,
  `f` the folder, `i` inspect.
- `B` dump its PostgreSQL, MySQL or MariaDB to
  `~/.local/share/docker-stacks/backups/STACK/` (owner-only files).
- `x` uninstall: choose to dump the databases first, delete the volumes,
  delete the images and move the project folder to `.trash`, then type the
  name to confirm.

**Links** come from published web ports, Traefik router rules
(`Host(...)`), Caddy labels, and a `docker-stacks.url` label of your own:

```yaml
labels:
  docker-stacks.url: "https://photos.example.org/admin"
```

**Containers.** Every container, also those started with `docker run`:
start, stop, restart, pause, kill, remove, logs, shell, links, inspect, and
live CPU, memory, network, disk I/O and processes.

**Images.** Tags, size, age and what uses them; pull again, remove, remove
the dangling ones (`P`) or every unused one (`A`).

**Volumes.** Which stack and containers use each one, and its size (`z`);
delete one (type its name) or every unused one (type `prune`).

**Networks.** Driver, scope and containers; remove, prune.

**System.** Docker, the host, disk use per kind with what can be reclaimed,
the build cache, a clean-up of everything unused (volumes are kept), and
Docker's events as they happen.

Everything that can lose data asks first, and what deletes data for good
wants a typed word. Actions run in the background: the activity panel shows
their progress and result while you keep working.

## Keys

| Key | Where | What |
|---|---|---|
| `1`-`6`, `Tab`, `←` `→` | everywhere | views |
| `↑` `↓`, `j` `k`, `PgUp` `PgDn`, `g` `G` | | move |
| `/`, `Esc` | | filter, clear it |
| `Enter` | | logs (stacks, containers), inspect (the rest) |
| `R` | | refresh now |
| `t` | | next theme |
| `?` | | every key |
| `q`, `Ctrl+C` | | quit |

The mouse works too: click a view or a row, scroll with the wheel.

## Command line

```sh
docker-stacks                    # the terminal view
docker-stacks list [--json]      # stacks, state, services, first link
docker-stacks up|down|stop|start|restart|pull|update|logs NAME
docker-stacks links NAME         # every link of a running stack
docker-stacks open NAME
docker-stacks themes
```

## Install

With NIRUX:

```sh
nirux extras install docker-stacks
```

On its own (any x86_64 Linux; the binary is static):

```sh
git clone https://github.com/nirucon/docker-stacks.git
cd docker-stacks
./install.sh            # the release binary, checked with SHA-256
./install.sh --build    # or build it with cargo
```

It installs `~/.local/bin/docker-stacks`. It needs Docker with Compose v2
and access to the Docker socket (the `docker` group). Docker access is root
access: manage only Compose files you trust.

`./uninstall.sh` removes the program; `--purge` also removes the settings
and the remembered projects. Database dumps are never removed.

## Configuration

`~/.config/docker-stacks/config.toml`, every key optional (see
`config.example.toml`):

```toml
roots = ["~/Docker"]      # folders whose subfolders hold compose projects
theme = "nirux"           # nirux, amber, phosphor, c64, paper, native
url_host = ""             # host in links; empty: localhost, or this host over SSH
backups = "~/.local/share/docker-stacks/backups"
refresh = 10              # seconds between quiet refreshes
show_loose = true         # list containers outside any stack
```

`nirux` follows the desktop: the NIRU theme chosen in NIRUX (Noir, Nord,
Satie, Carl Larsson and the rest) with the colours of `appearance.toml`. It
changes when the desktop's theme changes.

Compose files in a folder you may not read (owned by root) still work for
stop, start, restart, down and logs, which Compose does by project name;
up, pull and build need the file.

## Files

| Path | What |
|---|---|
| `~/.config/docker-stacks/config.toml` | settings |
| `~/.local/state/docker-stacks/known.json` | projects seen before, so one taken down stays listed |
| `~/.local/share/docker-stacks/backups/` | database dumps |
| `ROOT/.trash/` | project folders moved there by an uninstall |

## Building

```sh
cargo build --release
cargo test
sh tests/docker.sh target/release/docker-stacks   # against a real Docker
```

## History

Versions before 1.0 were a Python program (Textual). 1.0 is a rewrite in
Rust: one binary, a tenth of the memory, live updates from Docker's events,
and much more than stacks. `DOCKER_STACKS_DIR` and `--list` still work.

## Status, disclaimer and how it's made

**Status: beta.** In use on my machines against real stacks; the actions
that delete data ask for a typed confirmation.

NIRUX and every niru* project are made by me, Nicklas Rudolfsson
([@nirucon](https://github.com/nirucon)), for me. They are tailored to my own
needs, preferences and workflows, on my own machines. If they fit yours too,
that's a nice bonus.

- **Free and open source.** Anyone is free to use, study, change and share
  it under the [MIT License](LICENSE).
- **No guarantees.** There is no warranty of any kind. Use it at your own
  risk, read what you install and keep backups of anything that matters.
- **How it's made.** Some of these projects I built from scratch by hand. In
  more recent releases I have used Claude, Anthropic's AI, to do much of the
  heavy lifting under my direction: I decide what to build and how it should
  behave, and I test it on my own machines. It saves me a lot of time, and
  the result is quite OK.
- **Alpha, beta, dev, and maybe forever.** Most things here are alpha, beta
  or dev, and may stay that way. Maybe one day there will be a real stable
  release, but given how fast development moves, and how I am as a person,
  maybe never :)

| Label | Meaning |
|---|---|
| dev | Under active construction; may not build, install or work yet. |
| alpha | Works on my machines, but incomplete or rough; expect breaking changes. |
| beta | In daily use on my machines and mostly complete; rough edges remain. |

## License

MIT
