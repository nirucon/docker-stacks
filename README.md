# docker-stacks

A lightweight terminal UI for Docker Compose projects. No daemon or web server.
**Experimental — review backups before using destructive actions.**

## Install and run

Requires Python 3.10+, Docker with Compose v2, and access to the Docker daemon.
Docker access is effectively root access; only manage trusted Compose files.

On Debian 13:

```sh
sudo apt install python3-textual python3-rich git
git clone https://github.com/nirucon/docker-stacks.git
cd docker-stacks
python3 docker_stacks.py
```

Elsewhere, clone the repo and install dependencies in a virtual environment:

```sh
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/python docker_stacks.py
```

Tested on Linux with Textual 2.1.2 and Compose 2.26.1.

Discovers `~/Docker/<project>/compose.y*ml` and `docker-compose.y*ml`, including
stopped projects, plus projects reported by `docker compose ls -a` whose files
still exist. Use unique, explicit project names (`name: my-project` or
`COMPOSE_PROJECT_NAME` in each project's `.env`). Profile-only services are not
counted as required. Fully removed projects outside the scan root cannot be
rediscovered automatically.

```sh
DOCKER_STACKS_DIR=/path/to/stacks python3 docker_stacks.py
python3 docker_stacks.py --list
```

## Keys

| Key | Action |
| --- | --- |
| `u` | Start (`up -d`) |
| `d` | Shut down; keep volumes |
| `s` | Stop; keep containers and volumes |
| `r` | Restart existing containers |
| `l` | Logs |
| `b` | PostgreSQL dump |
| `x` | Uninstall (type project name to confirm) |
| `R` | Refresh |
| `q` | Quit |

## Data safety

- **Uninstall runs `docker compose down -v`: project volumes are permanently
  deleted.** External volumes and images remain; bind-mounted data is not
  removed by Compose.
- Project directories directly under the scan root are moved to `.trash` by
  default. Permanent directory deletion is a separate option. Other paths and
  symlinked project directories are left untouched.
- Optional backup uses `pg_dump` in a **running PostgreSQL service**, with
  `POSTGRES_USER` and `POSTGRES_DB`. Dumps are owner-only files in the scan
  root's `.backups`. A failed requested backup aborts uninstall. Other database
  engines, stopped databases and external data need manual backups.
- Volume names shown are prefix-based hints, not a complete deletion inventory.
  Review your Compose configuration before uninstalling. A successful dump is
  not a substitute for a tested restore.

MIT licensed. Run tests with `python3 -m unittest discover -s tests`.
