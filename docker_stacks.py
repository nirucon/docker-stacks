#!/usr/bin/env python3
"""docker-stacks — a small TUI for Docker Compose stacks.

Manages projects (not individual containers): status, up/down/stop/restart,
logs, DB backup and safe uninstall. Container details can still be handled
with lazydocker.
"""
from __future__ import annotations

import asyncio
import json
import os
import re
import shutil
import sys
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

from rich.text import Text
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.css.query import NoMatches
from textual.screen import ModalScreen
from textual.widgets import (
    Button,
    Checkbox,
    DataTable,
    Footer,
    Header,
    Input,
    Label,
    RichLog,
    Static,
)

VERSION = "0.1.0"
DOCKER = shutil.which("docker") or "docker"
ROOT = Path(os.environ.get("DOCKER_STACKS_DIR", Path.home() / "Docker")).expanduser()
COMPOSE_NAMES = ("compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml")
TRASH_DIR = ROOT / ".trash"
BACKUP_DIR = ROOT / ".backups"
_EXPECTED_TTL = 30.0
_EXPECTED_CACHE: dict[str, tuple[float, list[str]]] = {}

C_RUN = "#8fbf7f"
C_PARTIAL = "#d7b36a"
C_STOP = "#a8a8a8"
C_ERR = "#d77a7a"
C_TEXT = "#e5e5e5"
C_MUTED = "#a8a8a8"


@dataclass
class Service:
    name: str
    state: str = ""
    health: str = ""
    ports: str = ""
    image: str = ""
    exit_code: str = ""


@dataclass
class Stack:
    name: str
    path: Path
    compose_file: Path
    services: list[Service] = field(default_factory=list)
    volumes: list[str] = field(default_factory=list)
    expected: list[str] = field(default_factory=list)
    error: str = ""
    config_files: list[Path] = field(default_factory=list)


def read_project_name(compose: Path, path: Path) -> str:
    try:
        text = compose.read_text(errors="replace")
    except OSError:
        text = ""
    env = path / ".env"
    if env.is_file():
        try:
            m = re.search(
                r"(?m)^COMPOSE_PROJECT_NAME\s*=\s*['\"]?([A-Za-z0-9_.-]+)",
                env.read_text(errors="replace"),
            )
            if m:
                return m.group(1)
        except OSError:
            pass
    m = re.search(r"(?m)^name:\s*['\"]?([a-z0-9][a-z0-9_-]*)['\"]?\s*(?:#.*)?$", text)
    if m:
        return m.group(1)
    name = re.sub(r"[^a-z0-9_-]", "-", path.name.lower()).strip("-")
    return name or "stack"


def compose_cmd(stack: Stack, *args: str) -> list[str]:
    command = [
        DOCKER,
        "compose",
        "--project-directory",
        str(stack.path),
        "-p",
        stack.name,
    ]
    for config in stack.config_files or [stack.compose_file]:
        command.extend(["-f", str(config)])
    return [*command, *args]


async def run_cmd(args: list[str], timeout: float = 20) -> tuple[int, str]:
    try:
        proc = await asyncio.create_subprocess_exec(
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
        )
    except FileNotFoundError:
        return 127, f"{args[0]} not found"
    try:
        out, _ = await asyncio.wait_for(proc.communicate(), timeout=timeout)
    except asyncio.TimeoutError:
        proc.kill()
        await proc.wait()
        return 124, f"timeout after {timeout:.0f}s"
    except asyncio.CancelledError:
        if proc.returncode is None:
            proc.kill()
        await proc.wait()
        raise
    return proc.returncode or 0, out.decode(errors="replace")


async def run_to_file(args: list[str], path: Path, timeout: float = 120) -> tuple[bool, str]:
    try:
        with path.open("xb") as fh:
            path.chmod(0o600)
            proc = await asyncio.create_subprocess_exec(
                *args, stdout=fh, stderr=asyncio.subprocess.PIPE
            )
            try:
                _, err = await asyncio.wait_for(proc.communicate(), timeout=timeout)
            except asyncio.TimeoutError:
                proc.kill()
                await proc.wait()
                path.unlink(missing_ok=True)
                return False, "timeout"
            except asyncio.CancelledError:
                if proc.returncode is None:
                    proc.kill()
                await proc.wait()
                path.unlink(missing_ok=True)
                raise
    except FileNotFoundError:
        path.unlink(missing_ok=True)
        return False, f"{args[0]} not found"
    if proc.returncode != 0:
        path.unlink(missing_ok=True)
        return False, err.decode(errors="replace").strip()[:400]
    return True, ""


async def docker_ok() -> bool:
    rc, _ = await run_cmd([DOCKER, "info"], timeout=6)
    return rc == 0


def parse_json_rows(out: str) -> list[dict]:
    out = out.strip()
    if not out:
        return []
    try:
        data = json.loads(out)
        if isinstance(data, list):
            return [x for x in data if isinstance(x, dict)]
        if isinstance(data, dict):
            return [data]
    except json.JSONDecodeError:
        pass
    rows: list[dict] = []
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            value = json.loads(line)
            if isinstance(value, dict):
                rows.append(value)
        except json.JSONDecodeError:
            continue
    return rows


def fmt_ports(publishers: object) -> str:
    if not isinstance(publishers, list):
        return ""
    out: list[str] = []
    for pub in publishers:
        if not isinstance(pub, dict):
            continue
        host = pub.get("PublishedPort")
        target = pub.get("TargetPort")
        proto = pub.get("Protocol") or "tcp"
        if host:
            out.append(f"{host}→{target}/{proto}")
    return ", ".join(dict.fromkeys(out))


async def expected_services(stack: Stack) -> list[str]:
    key = str(stack.compose_file)
    now = time.monotonic()
    cached = _EXPECTED_CACHE.get(key)
    if cached and now - cached[0] < _EXPECTED_TTL:
        return list(cached[1])

    result: list[str] = []
    rc, out = await run_cmd(compose_cmd(stack, "config", "--format", "json"), timeout=15)
    if rc == 0:
        try:
            cfg = json.loads(out)
            services = cfg.get("services") or {}
            result = [
                name
                for name, svc in services.items()
                if not (isinstance(svc, dict) and svc.get("profiles"))
            ]
            _EXPECTED_CACHE[key] = (now, result)
            return list(result)
        except json.JSONDecodeError:
            pass
    rc, out = await run_cmd(compose_cmd(stack, "config", "--services"), timeout=15)
    if rc == 0:
        result = [x.strip() for x in out.splitlines() if x.strip()]
        _EXPECTED_CACHE[key] = (now, result)
    return list(result)


async def fill_status(
    stack: Stack, docker_available: bool, volume_names: Optional[list[str]] = None
) -> None:
    stack.services = []
    stack.volumes = []
    if stack.error:
        return
    if not docker_available:
        stack.error = "Docker is not accessible — log out/in or run through the docker group."
        return

    stack.expected = await expected_services(stack)
    rc, out = await run_cmd(compose_cmd(stack, "ps", "--all", "--format", "json"), timeout=20)
    if rc != 0:
        stack.error = out.strip()[:300] or "docker compose ps failed"
    else:
        for row in parse_json_rows(out):
            stack.services.append(
                Service(
                    name=str(row.get("Service") or row.get("Name") or "?"),
                    state=str(row.get("State") or "").lower(),
                    health=str(row.get("Health") or "").lower(),
                    ports=fmt_ports(row.get("Publishers")),
                    image=str(row.get("Image") or ""),
                    exit_code=str(row.get("ExitCode") if row.get("ExitCode") is not None else ""),
                )
            )

    have = {s.name for s in stack.services}
    running = any(s.state == "running" for s in stack.services)
    for name in stack.expected:
        if name in have:
            continue
        stack.services.append(Service(name=name, state="missing" if running else "stopped"))
    stack.services.sort(key=lambda s: (s.state != "running", s.name))

    if volume_names is None:
        rc, out = await run_cmd([DOCKER, "volume", "ls", "--format", "{{.Name}}"], timeout=10)
        volume_names = out.splitlines() if rc == 0 else []
    prefix = f"{stack.name}_"
    stack.volumes = sorted(v for v in volume_names if v.startswith(prefix))


def stack_state(stack: Stack) -> str:
    if stack.error:
        return "error"
    running = [s for s in stack.services if s.state == "running"]
    if not running:
        return "stopped"
    if any(s.state == "missing" for s in stack.services):
        return "partial"
    if any(s.state == "exited" and s.exit_code not in ("", "0") for s in stack.services):
        return "partial"
    if any(s.health == "unhealthy" for s in running):
        return "partial"
    return "running"


def state_text(value: str) -> Text:
    styles = {
        "running": (C_RUN, "●"),
        "partial": (C_PARTIAL, "◐"),
        "stopped": (C_STOP, "○"),
        "error": (C_ERR, "✖"),
    }
    color, symbol = styles.get(value, (C_MUTED, "?"))
    return Text(f"{symbol} {value}", style=color)


def service_state_text(svc: Service) -> Text:
    if svc.state == "missing":
        return Text("missing", style=C_ERR)
    if svc.state == "running":
        return Text("running", style=C_RUN)
    if svc.state == "exited":
        label = f"exited({svc.exit_code})" if svc.exit_code != "" else "exited"
        return Text(label, style=C_PARTIAL)
    return Text(svc.state or "–", style=C_MUTED)


def health_text(value: str) -> Text:
    if value in ("healthy",):
        return Text(value, style=C_RUN)
    if value in ("starting",):
        return Text(value, style=C_PARTIAL)
    if value in ("unhealthy",):
        return Text(value, style=C_ERR)
    return Text("–", style=C_MUTED)


def ports_text(stack: Stack) -> str:
    ports: list[str] = []
    for svc in stack.services:
        if svc.ports:
            ports.extend(p.strip() for p in svc.ports.split(",") if p.strip())
    return ", ".join(dict.fromkeys(ports)) or "–"


async def discover_stacks(docker_available: bool) -> list[Stack]:
    found: dict[str, Stack] = {}

    if ROOT.is_dir():
        for entry in sorted(ROOT.iterdir()):
            if not entry.is_dir() or entry.name.startswith("."):
                continue
            compose = next((entry / n for n in COMPOSE_NAMES if (entry / n).is_file()), None)
            if not compose:
                continue
            name = read_project_name(compose, entry)
            if name in found:
                found[name].error = "Duplicate project name; use unique Compose project names."
                continue
            overrides = ("compose.override.yaml", "compose.override.yml")
            if compose.name.startswith("docker-compose"):
                overrides = ("docker-compose.override.yaml", "docker-compose.override.yml")
            override = next((entry / n for n in overrides if (entry / n).is_file()), None)
            found[name] = Stack(
                name=name, path=entry, compose_file=compose,
                config_files=[compose, *([override] if override else [])],
            )

    if docker_available:
        rc, out = await run_cmd([DOCKER, "compose", "ls", "-a", "--format", "json"], timeout=15)
        if rc == 0:
            for project in parse_json_rows(out):
                name = project.get("Name") or project.get("name")
                if not name:
                    continue
                config = project.get("ConfigFiles") or project.get("configFiles") or ""
                paths = config if isinstance(config, list) else str(config).split(",")
                configs = [Path(str(p).strip()) for p in paths if str(p).strip()]
                if not configs or not all(p.is_file() for p in configs):
                    continue
                compose = configs[0]
                if name in found:
                    if found[name].compose_file.resolve() == compose.resolve():
                        found[name].config_files = configs
                    else:
                        found[name].error = "Duplicate project name; use unique Compose project names."
                    continue
                found[str(name)] = Stack(
                    name=str(name), path=compose.parent, compose_file=compose,
                    config_files=configs,
                )

    volume_names: list[str] = []
    if docker_available:
        rc, out = await run_cmd(
            [DOCKER, "volume", "ls", "--format", "{{.Name}}"], timeout=10
        )
        if rc == 0:
            volume_names = out.splitlines()

    stacks = sorted(found.values(), key=lambda s: s.name.lower())
    for stack in stacks:
        await fill_status(stack, docker_available, volume_names)
    return stacks


# ── modals ─────────────────────────────────────────────────────────────────


class ConfirmScreen(ModalScreen[bool]):
    BINDINGS = [
        Binding("y", "confirm", "Yes"),
        Binding("n", "cancel", "No"),
        Binding("escape", "cancel", "Cancel"),
    ]

    def __init__(self, title: str, message: str, danger: bool = False) -> None:
        super().__init__()
        self.title_text = title
        self.message = message
        self.danger = danger

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            yield Label(self.title_text, classes="dialog-title", markup=False)
            yield Static(self.message, markup=False, classes="dialog-body")
            with Horizontal(classes="dialog-buttons"):
                yield Button(
                    "Yes",
                    id="yes",
                    variant="error" if self.danger else "primary",
                )
                yield Button("No", id="no")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "yes")

    def action_confirm(self) -> None:
        self.dismiss(True)

    def action_cancel(self) -> None:
        self.dismiss(False)


class PurgeScreen(ModalScreen[Optional[dict]]):
    BINDINGS = [Binding("escape", "cancel", "Cancel")]

    def __init__(self, stack: Stack) -> None:
        super().__init__()
        self.stack = stack

    def compose(self) -> ComposeResult:
        has_db = any(
            svc.name in ("db", "postgres", "database")
            or "postgres" in svc.image.lower()
            for svc in self.stack.services
        )
        volumes = ", ".join(self.stack.volumes) if self.stack.volumes else "–"
        message = (
            f"This uninstalls {self.stack.name} completely:\n\n"
            f"  • docker compose down -v  (containers, networks and VOLUMES are removed)\n"
            f"  • the project directory is handled by the option below\n\n"
            f"Project: {self.stack.path}\n"
            f"Volume name hints (review Compose config): {volumes}\n\n"
            "This cannot be undone unless you take a backup first."
        )
        with Vertical(id="dialog"):
            yield Label(f"Uninstall {self.stack.name}", classes="dialog-title", markup=False)
            yield Static(message, markup=False, classes="dialog-body")
            yield Label("Type the project name to confirm:", markup=False)
            yield Input(placeholder=self.stack.name, id="confirm-name")
            yield Checkbox("Take a database backup first (recommended)", value=has_db, id="backup")
            yield Checkbox(
                f"Delete the project directory permanently (otherwise move to {TRASH_DIR})",
                value=False,
                id="permanent",
            )
            with Horizontal(classes="dialog-buttons"):
                yield Button("Uninstall", id="purge", variant="error")
                yield Button("Cancel", id="cancel")

    def on_mount(self) -> None:
        self.query_one("#confirm-name", Input).focus()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "cancel":
            self.dismiss(None)
            return
        name = self.query_one("#confirm-name", Input).value.strip()
        if name != self.stack.name:
            self.app.notify("Wrong project name — cancelled.", severity="error")
            return
        self.dismiss(
            {
                "backup": self.query_one("#backup", Checkbox).value,
                "permanent": self.query_one("#permanent", Checkbox).value,
            }
        )

    def action_cancel(self) -> None:
        self.dismiss(None)


class LogScreen(ModalScreen[None]):
    BINDINGS = [
        Binding("escape", "close", "Close"),
        Binding("r", "reload", "Refresh"),
    ]

    def __init__(self, stack: Stack) -> None:
        super().__init__()
        self.stack = stack

    def compose(self) -> ComposeResult:
        with Vertical(id="logdialog"):
            yield Label(f"Logs · {self.stack.name}", classes="dialog-title", markup=False)
            yield RichLog(id="logview", highlight=False, markup=False)
            yield Static("r = refresh · Esc = close", classes="hint", markup=False)

    def on_mount(self) -> None:
        self.run_worker(self.load_logs(), exclusive=True)

    async def load_logs(self) -> None:
        view = self.query_one("#logview", RichLog)
        view.clear()
        rc, out = await run_cmd(
            compose_cmd(self.stack, "logs", "--tail=400", "--no-color"), timeout=40
        )
        if rc != 0:
            view.write(out.strip() or "(could not read logs)")
        else:
            view.write(out.strip()[-30000:] or "(no logs)")

    def action_reload(self) -> None:
        self.run_worker(self.load_logs(), exclusive=True)

    def action_close(self) -> None:
        self.dismiss(None)


# ── app ────────────────────────────────────────────────────────────────────


class DockerStacksApp(App):
    TITLE = "Docker Stacks"
    SUB_TITLE = "compose projects · up/down · safe purge"
    CSS = """
    Screen { background: #0f0f10; color: #e5e5e5; }
    Header { background: #161618; color: #e5e5e5; }
    Footer { background: #161618; color: #a8a8a8; }
    #main { height: 1fr; }
    #left { width: 48; border: round #2a2a2d; }
    #right { border: round #2a2a2d; }
    .panel-title { background: #26262a; color: #e5e5e5; padding: 0 1; }
    #stacks { height: 1fr; background: #161618; }
    #services { height: 1fr; background: #161618; }
    #summary { height: auto; padding: 1 2; background: #161618; }
    #log { height: 7; border: round #2a2a2d; background: #0f0f10; color: #a8a8a8; }
    ConfirmScreen, PurgeScreen { align: center middle; background: #0f0f10; }
    #dialog { width: 82; height: auto; max-height: 90%; border: round #2a2a2d; background: #161618; padding: 1 2; }
    .dialog-title { text-style: bold; color: #e5e5e5; padding-bottom: 1; }
    .dialog-body { padding-bottom: 1; color: #e5e5e5; }
    .dialog-buttons { height: auto; align-horizontal: right; padding-top: 1; }
    .dialog-buttons Button { margin-left: 1; }
    .hint { color: #a8a8a8; padding-top: 1; }
    LogScreen { align: center middle; background: #0f0f10; }
    #logdialog { width: 92%; height: 88%; border: round #2a2a2d; background: #0f0f10; padding: 1 2; }
    #logview { height: 1fr; background: #0f0f10; color: #e5e5e5; }
    """

    BINDINGS = [
        Binding("u", "up", "Start"),
        Binding("d", "down", "Shut down"),
        Binding("s", "stop", "Stop"),
        Binding("r", "restart", "Restart"),
        Binding("l", "logs", "Logs"),
        Binding("b", "backup", "DB backup"),
        Binding("x", "purge", "Uninstall"),
        Binding("R", "refresh_now", "Refresh"),
        Binding("q", "quit", "Quit"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self.stacks: list[Stack] = []
        self._refreshing = False
        self._docker_available = True
        self._busy = False

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        with Horizontal(id="main"):
            with Vertical(id="left"):
                yield Label("Stacks", classes="panel-title")
                yield DataTable(id="stacks", cursor_type="row", zebra_stripes=True)
            with Vertical(id="right"):
                yield Static(id="summary", markup=False)
                yield Label("Services", classes="panel-title")
                yield DataTable(id="services", cursor_type="row", zebra_stripes=True)
        yield RichLog(id="log", highlight=False, markup=False)
        yield Footer()

    def on_mount(self) -> None:
        stacks_table = self.query_one("#stacks", DataTable)
        stacks_table.add_columns("Stack", "Status", "Up", "Ports")
        services_table = self.query_one("#services", DataTable)
        services_table.add_columns("Service", "Status", "Health", "Ports", "Image")
        self.log_line("docker-stacks ready · u=start  d=shut down  s=stop  r=restart")
        self.log_line("l=logs  b=DB backup  x=uninstall  R=refresh  q=quit")
        self.set_interval(3.0, self._timer_refresh)
        self.run_worker(self.refresh_stacks(), exclusive=True)
        stacks_table.focus()

    def log_line(self, message: str) -> None:
        try:
            self.query_one("#log", RichLog).write(message)
        except Exception:
            pass

    def _timer_refresh(self) -> None:
        if not self._refreshing and not self._busy and len(self.screen_stack) == 1:
            self.run_worker(self.refresh_stacks(), exclusive=False, group="refresh")

    async def refresh_stacks(self) -> None:
        if self._refreshing:
            return
        self._refreshing = True
        try:
            available = await docker_ok()
            stacks = await discover_stacks(available)
            self._docker_available = available
        except Exception as exc:  # noqa: BLE001
            available = False
            self._docker_available = False
            stacks = []
            self.log_line(f"Refresh error: {exc}")
        finally:
            self._refreshing = False

        selected = self.selected_stack()
        selected_name = selected.name if selected else None
        self.stacks = stacks

        # A modal dialog may be open while a background refresh finishes.
        # Never touch widgets from the main screen in that case.
        try:
            table = self.query_one("#stacks", DataTable)
        except NoMatches:
            return
        table.clear(columns=False)
        for stack in stacks:
            running = sum(1 for s in stack.services if s.state == "running")
            total = len(stack.services)
            table.add_row(
                Text(stack.name),
                state_text(stack_state(stack)),
                f"{running}/{total}",
                ports_text(stack),
                key=stack.name,
            )
        if selected_name:
            for index, stack in enumerate(stacks):
                if stack.name == selected_name:
                    table.move_cursor(row=index)
                    break
        if stacks and table.cursor_row < 0:
            table.move_cursor(row=0)
        if not available:
            self.log_line("Warning: Docker is not accessible in this session.")
        self.update_details()

    def selected_stack(self) -> Optional[Stack]:
        if not self.stacks:
            return None
        try:
            row = self.query_one("#stacks", DataTable).cursor_row
        except Exception:
            return None
        if row is None or row < 0 or row >= len(self.stacks):
            return None
        return self.stacks[row]

    def update_details(self) -> None:
        try:
            summary = self.query_one("#summary", Static)
            table = self.query_one("#services", DataTable)
        except NoMatches:
            # Modal screen (or app still starting): main widgets are absent.
            return
        table.clear(columns=False)
        stack = self.selected_stack()
        if stack is None:
            summary.update(
                Text(
                    "No stacks found.\n"
                    f"Put a compose file in {ROOT}/<name>/.",
                    style=C_MUTED,
                )
            )
            return

        state = stack_state(stack)
        running = sum(1 for s in stack.services if s.state == "running")
        text = Text()
        text.append(stack.name, style=f"bold {C_TEXT}")
        text.append("  ")
        text.append(state_text(state))
        text.append("\n")
        text.append(f"Path: {stack.path}\n", style=C_MUTED)
        text.append(f"Services: {running}/{len(stack.services)} running\n", style=C_MUTED)
        if stack.volumes:
            text.append(f"Volumes: {', '.join(stack.volumes)}\n", style=C_MUTED)
        if stack.error:
            text.append(f"Error: {stack.error}\n", style=C_ERR)
        summary.update(text)

        for svc in stack.services:
            table.add_row(
                Text(svc.name),
                service_state_text(svc),
                health_text(svc.health),
                Text(svc.ports or "–", style=C_MUTED),
                Text(svc.image, style=C_MUTED),
            )
        if not stack.services:
            table.add_row(Text("–", style=C_MUTED), "", "", "", "")

    def on_data_table_row_highlighted(self, event: DataTable.RowHighlighted) -> None:
        if event.data_table.id == "stacks":
            self.update_details()

    def _operation(self, coroutine) -> None:
        if self._busy:
            coroutine.close()
            self.notify("An operation is already running.", severity="warning")
            return
        self._busy = True
        self.run_worker(self._operation_worker(coroutine), group="operation")

    async def _operation_worker(self, coroutine) -> None:
        try:
            await coroutine
        except (OSError, shutil.Error) as exc:
            self.notify(f"Operation failed: {exc}", severity="error")
        finally:
            self._busy = False

    # ── actions ────────────────────────────────────────────────────────────

    def action_up(self) -> None:
        stack = self.selected_stack()
        if stack:
            self._compose(stack, ["up", "-d"], "Starting")

    def action_down(self) -> None:
        stack = self.selected_stack()
        if not stack:
            return
        self.push_screen(
            ConfirmScreen(
                "Shut down stack",
                f"Run docker compose down for {stack.name}?\n\n"
                "Containers and networks are removed, but volumes are kept.",
                danger=True,
            ),
            lambda ok: self._compose(stack, ["down"], "Shutting down") if ok else None,
        )

    def action_stop(self) -> None:
        stack = self.selected_stack()
        if not stack:
            return
        self.push_screen(
            ConfirmScreen(
                "Stop stack",
                f"Stop all services in {stack.name}?\n\n"
                "Containers remain and can be started with u.",
            ),
            lambda ok: self._compose(stack, ["stop"], "Stopping") if ok else None,
        )

    def action_restart(self) -> None:
        stack = self.selected_stack()
        if stack:
            self._compose(stack, ["restart"], "Restarting")

    def action_logs(self) -> None:
        stack = self.selected_stack()
        if stack:
            self.push_screen(LogScreen(stack))

    def action_backup(self) -> None:
        stack = self.selected_stack()
        if stack:
            self._operation(self._backup_worker(stack))

    def action_purge(self) -> None:
        stack = self.selected_stack()
        if not stack:
            return
        self.push_screen(PurgeScreen(stack), lambda result: self._purge(stack, result))

    def action_refresh_now(self) -> None:
        if not self._busy:
            self.run_worker(self.refresh_stacks(), group="refresh")

    def _compose(self, stack: Stack, args: list[str], label: str) -> None:
        if stack.error:
            self.notify(stack.error, severity="error")
            return
        self._operation(self._compose_worker(stack, args, label))

    async def _compose_worker(self, stack: Stack, args: list[str], label: str) -> None:
        self.log_line(f"$ docker compose -p {stack.name} {' '.join(args)}")
        self.notify(f"{label} {stack.name}…")
        rc, out = await run_cmd(compose_cmd(stack, *args), timeout=900)
        if out.strip():
            self.log_line(out.strip()[-4000:])
        if rc == 0:
            self.notify(f"{label}: OK", severity="information")
        else:
            self.notify(f"{label}: ERROR (rc={rc})", severity="error")
        await self.refresh_stacks()

    def _db_service(self, stack: Stack) -> Optional[Service]:
        for svc in stack.services:
            if svc.state == "running" and (
                svc.name in ("db", "postgres", "database") or "postgres" in svc.image.lower()
            ):
                return svc
        return None

    async def _backup_worker(self, stack: Stack) -> bool:
        db = self._db_service(stack)
        if db is None:
            self.notify("No running PostgreSQL service found.", severity="error")
            return False
        BACKUP_DIR.mkdir(parents=True, exist_ok=True, mode=0o700)
        stamp = time.strftime("%Y%m%d-%H%M%S")
        target = BACKUP_DIR / f"{stack.name}-{stamp}-{uuid.uuid4().hex[:8]}.sql"
        cmd = compose_cmd(
            stack,
            "exec",
            "-T",
            db.name,
            "sh",
            "-c",
            'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB"',
        )
        ok, error = await run_to_file(cmd, target, timeout=180)
        if ok:
            self.log_line(f"Backup complete: {target}")
            self.notify(f"Backup: {target}")
            return True
        self.log_line(f"Backup failed: {error}")
        self.notify(f"Backup failed: {error}", severity="error")
        return False

    def _purge(self, stack: Stack, result: Optional[dict]) -> None:
        if not result:
            return
        if stack.error:
            self.notify(stack.error, severity="error")
            return
        self._operation(self._purge_worker(stack, result))

    async def _purge_worker(self, stack: Stack, result: dict) -> None:
        resolved = stack.path.resolve()
        # Only direct, non-symlink project directories may be moved or deleted.
        # Never move a configured root, a backup directory or a parent project.
        managed_directory = (
            stack.path.absolute() == resolved
            and resolved.parent == ROOT.resolve()
            and not resolved.name.startswith(".")
            and BACKUP_DIR.resolve() != resolved
            and TRASH_DIR.resolve() != resolved
        )
        if result.get("backup"):
            if not await self._backup_worker(stack):
                self.notify("Aborting: backup failed.", severity="error")
                return
        self.log_line(f"$ docker compose -p {stack.name} down -v")
        rc, out = await run_cmd(compose_cmd(stack, "down", "-v"), timeout=600)
        if out.strip():
            self.log_line(out.strip()[-4000:])
        if rc != 0:
            self.notify("Uninstall failed — nothing else was done.", severity="error")
            return

        if managed_directory and resolved.is_dir():
            if result.get("permanent"):
                shutil.rmtree(resolved)
                self.log_line(f"Project directory deleted: {resolved}")
            else:
                TRASH_DIR.mkdir(parents=True, exist_ok=True)
                stamp = time.strftime("%Y%m%d-%H%M%S")
                target = TRASH_DIR / f"{stack.name}-{stamp}-{uuid.uuid4().hex[:8]}"
                shutil.move(str(resolved), str(target))
                self.log_line(f"Project directory moved to: {target}")
        else:
            self.log_line("Project directory is not a managed direct child; left untouched.")

        self.notify(f"{stack.name} uninstalled (volumes deleted).")
        await self.refresh_stacks()


async def list_main() -> int:
    available = await docker_ok()
    stacks = await discover_stacks(available)
    if not available:
        print("Warning: Docker is not accessible in this session.", file=sys.stderr)
    if not stacks:
        print(f"No stacks found under {ROOT}.", file=sys.stderr)
        return 1
    for stack in stacks:
        running = sum(1 for s in stack.services if s.state == "running")
        total = len(stack.services)
        print(
            f"{stack.name:20} {stack_state(stack):10} {running}/{total:<3} "
            f"{ports_text(stack):24} {stack.path}"
        )
    return 0


def main() -> int:
    if "--version" in sys.argv:
        print(f"docker-stacks {VERSION}")
        return 0
    if "--list" in sys.argv or "-l" in sys.argv:
        return asyncio.run(list_main())
    if "--help" in sys.argv or "-h" in sys.argv:
        print(
            "docker-stacks — TUI for Docker Compose stacks\n"
            "  --list     list stacks and status\n"
            "  --version  version\n"
            "Run without arguments for the TUI."
        )
        return 0
    DockerStacksApp().run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
