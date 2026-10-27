from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, patch

import docker_stacks as ds


class HelpersTest(unittest.TestCase):
    def test_json_formats(self):
        self.assertEqual(ds.parse_json_rows('[{"Service":"app"}]'), [{"Service": "app"}])
        self.assertEqual(len(ds.parse_json_rows('{"a":1}\n{"a":2}')), 2)
        self.assertEqual(ds.parse_json_rows(""), [])

    def test_compose_preserves_files(self):
        stack = ds.Stack("demo", Path("/stacks/demo"), Path("/stacks/demo/compose.yml"))
        stack.config_files = [stack.compose_file, stack.path / "extra.yml"]
        cmd = ds.compose_cmd(stack, "down")
        self.assertEqual(cmd.count("-f"), 2)
        self.assertNotIn("-v", cmd)
        self.assertEqual(cmd[-1], "down")

    def test_failed_service_is_partial(self):
        stack = ds.Stack("demo", Path("/demo"), Path("/demo/compose.yml"))
        stack.services = [ds.Service("db", "running"), ds.Service("app", "exited", exit_code="1")]
        self.assertEqual(ds.stack_state(stack), "partial")
        stack.services[1].exit_code = "0"
        self.assertEqual(ds.stack_state(stack), "running")

    def test_project_env_precedence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            compose = root / "compose.yml"
            compose.write_text("name: from-compose\n")
            (root / ".env").write_text("COMPOSE_PROJECT_NAME=from-env\n")
            self.assertEqual(ds.read_project_name(compose, root), "from-env")


class AsyncTest(unittest.IsolatedAsyncioTestCase):
    async def test_dump_permissions_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as temp:
            target = Path(temp) / "backup.sql"
            ok, _ = await ds.run_to_file(["/usr/bin/printf", "test"], target)
            self.assertTrue(ok)
            self.assertEqual(target.stat().st_mode & 0o777, 0o600)
            with self.assertRaises(FileExistsError):
                await ds.run_to_file(["/usr/bin/printf", "overwritten"], target)
            self.assertEqual(target.read_text(), "test")

    async def test_failed_dump_removed(self):
        with tempfile.TemporaryDirectory() as temp:
            target = Path(temp) / "backup.sql"
            ok, _ = await ds.run_to_file(["/usr/bin/false"], target)
            self.assertFalse(ok)
            self.assertFalse(target.exists())

    async def test_discover_stopped_and_override(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            project = root / "demo"
            project.mkdir()
            (project / "compose.yaml").write_text("name: demo\n")
            override = project / "compose.override.yaml"
            override.write_text("services: {}\n")
            with patch.object(ds, "ROOT", root):
                stacks = await ds.discover_stacks(False)
            self.assertEqual([s.name for s in stacks], ["demo"])
            self.assertIn(override, stacks[0].config_files)

    async def test_failed_backup_aborts_purge(self):
        app = ds.DockerStacksApp()
        app.notify = lambda *a, **k: None
        app._backup_worker = AsyncMock(return_value=False)
        stack = ds.Stack("demo", Path("/demo"), Path("/demo/compose.yml"))
        with patch.object(ds, "run_cmd", new_callable=AsyncMock) as run:
            await app._purge_worker(stack, {"backup": True})
            run.assert_not_awaited()

    async def test_symlink_project_directory_not_deleted(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "stacks"
            root.mkdir()
            data = Path(temp) / "outside"
            data.mkdir()
            (root / "demo").symlink_to(data, target_is_directory=True)
            stack = ds.Stack("demo", root / "demo", root / "demo" / "compose.yml")
            app = ds.DockerStacksApp()
            app.notify = lambda *a, **k: None
            app.log_line = lambda *a: None
            app.refresh_stacks = AsyncMock()
            with patch.object(ds, "ROOT", root), patch.object(ds, "run_cmd", AsyncMock(return_value=(0, ""))):
                await app._purge_worker(stack, {"backup": False, "permanent": True})
            self.assertTrue(data.is_dir())
            self.assertTrue(stack.path.is_symlink())

    async def test_dialog_survives_background_refresh(self):
        with patch.object(ds, "docker_ok", AsyncMock(return_value=False)), patch.object(
            ds, "discover_stacks", AsyncMock(return_value=[])
        ):
            app = ds.DockerStacksApp()
            async with app.run_test(size=(120, 40)) as pilot:
                await pilot.pause()
                app.push_screen(ds.ConfirmScreen("Shut down", "Keep volumes?"))
                await pilot.pause()
                await app.refresh_stacks()
                await pilot.press("n")
                await pilot.pause()
                self.assertEqual(len(app.screen_stack), 1)


if __name__ == "__main__":
    unittest.main()
