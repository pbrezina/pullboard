"""Container tooling checks; never start services or access user credentials."""
import configparser
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
QUADLET = ROOT / 'deploy/systemd/pullboard.container'
GENERATOR = next((path for path in [
    '/usr/lib/systemd/system-generators/podman-system-generator',
    '/usr/libexec/podman/quadlet',
    '/usr/lib/podman/quadlet',
] if os.access(path, os.X_OK)), None)


class ContainerRunTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='pullboard-tooling-')
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        for name in ['podman', 'docker']:
            engine = self.directory / name
            engine.write_text('''#!/usr/bin/env python3
import json
from pathlib import Path
import sys
if sys.argv[1:] == ['--version']:
    print(Path(sys.argv[0]).name + ' version 6.0.0')
else:
    print(json.dumps(sys.argv[1:]))
''')
            engine.chmod(0o700)
        self.environment = dict(os.environ, PATH=str(self.directory) + os.pathsep + os.environ['PATH'])

    def helper(self, *arguments):
        return subprocess.run(
            ['sh', str(ROOT / 'scripts/container-run.sh'), *arguments],
            cwd=ROOT, env=self.environment, capture_output=True, text=True,
        )

    def test_helper_defaults_to_loopback_3333_and_hardened_podman(self):
        result = self.helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        arguments = json.loads(result.stdout)
        self.assertEqual(arguments[arguments.index('-p') + 1], '127.0.0.1:3333:8080')
        for option in ['--read-only', '--cap-drop=ALL', '--security-opt=no-new-privileges',
                       '--userns=keep-id']:
            self.assertIn(option, arguments)
        self.assertEqual(arguments[arguments.index('--user') + 1], f'{os.getuid()}:{os.getgid()}')
        self.assertEqual(arguments[arguments.index('--tmpfs') + 1],
                         '/data:rw,nosuid,nodev,noexec,size=64m,mode=0700,U')
        self.assertEqual(arguments[-1], 'localhost/pullboard:latest')
        self.assertNotIn('-v', arguments)

    @unittest.skipUnless(shutil.which('make'), 'GNU Make unavailable')
    def test_make_run_uses_the_same_default_mapping(self):
        environment = self.environment.copy()
        for variable in ['PORT', 'MAKEFLAGS', 'MFLAGS']:
            environment.pop(variable, None)
        result = subprocess.run(
            ['make', '--no-print-directory', '-s', 'run',
             f'CONTAINER_ENGINE={self.directory / "podman"}',
             'IMAGE=localhost/pullboard:test', 'CONFIG=', 'RUN_NAME='],
            cwd=ROOT, env=environment, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        arguments = json.loads(result.stdout)
        self.assertEqual(arguments[arguments.index('-p') + 1], '127.0.0.1:3333:8080')

    def test_custom_port_and_read_only_config_with_spaces(self):
        config = self.directory / 'local config.yaml'
        config.write_text('jira: null\nforges: []\n')
        config.chmod(0o600)
        result = self.helper('podman', 'localhost/pullboard:test', str(config), '8090', 'test-name')
        self.assertEqual(result.returncode, 0, result.stderr)
        arguments = json.loads(result.stdout)
        self.assertEqual(arguments[arguments.index('-p') + 1], '127.0.0.1:8090:8080')
        self.assertEqual(arguments[arguments.index('-v') + 1], f'{config}:/app/config.yaml:ro,Z')
        self.assertEqual(arguments[arguments.index('--name') + 1], 'test-name')
        self.assertEqual(arguments[-1], 'localhost/pullboard:test')

    def test_existing_docker_helper_still_maps_tmpfs_ownership(self):
        result = self.helper('docker')
        self.assertEqual(result.returncode, 0, result.stderr)
        arguments = json.loads(result.stdout)
        self.assertNotIn('--userns=keep-id', arguments)
        self.assertEqual(arguments[arguments.index('-p') + 1], '127.0.0.1:3333:8080')
        self.assertEqual(arguments[arguments.index('--tmpfs') + 1],
                         f'/data:rw,nosuid,nodev,noexec,size=64m,uid={os.getuid()},gid={os.getgid()},mode=0700')

    def test_missing_config_fails_without_running_container(self):
        result = self.helper('podman', 'localhost/pullboard:test', str(self.directory / 'missing.yaml'))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, '')
        self.assertIn('Configuration file does not exist.', result.stderr)


class QuadletTests(unittest.TestCase):
    def assert_container_command(self, command):
        arguments = []
        for argument in shlex.split(command):
            # Podman 4.x emits --user=UID:GID; newer generators split the value.
            arguments.extend(argument.split('=', 1) if argument.startswith('--') else [argument])
        self.assertEqual(arguments[arguments.index('--pull') + 1], 'newer')
        self.assertEqual(arguments[arguments.index('--publish') + 1], '127.0.0.1:3333:8080')
        self.assertEqual(arguments[arguments.index('--user') + 1], '%U:%G')

    def test_generated_command_accepts_split_and_equals_options(self):
        for separator in [' ', '=']:
            with self.subTest(separator=separator):
                self.assert_container_command(
                    f'/usr/bin/podman run --pull{separator}newer '
                    f'--publish{separator}127.0.0.1:3333:8080 --user{separator}%U:%G '
                    'ghcr.io/pbrezina/pullboard:latest')

    def test_graphical_session_policy_and_no_keyring_integration(self):
        unit = configparser.ConfigParser(interpolation=None)
        unit.optionxform = str
        unit.read(QUADLET)
        self.assertEqual(unit.sections(), ['Unit', 'Container', 'Service', 'Install'])
        self.assertEqual(dict(unit['Unit']), {
            'Description': 'PullBoard local dashboard',
            'PartOf': 'graphical-session.target',
            'AssertPathExists': '%h/.config/pullboard/config.yaml',
        })
        self.assertEqual(dict(unit['Install']), {'WantedBy': 'graphical-session.target'})
        self.assertEqual(dict(unit['Service']), {
            'Restart': 'on-failure', 'RestartSec': '10', 'TimeoutStartSec': '900',
        })
        # An exact allowlist also guards against adding session bus mounts/env or auto-update timers.
        self.assertEqual(dict(unit['Container']), {
            'Image': 'ghcr.io/pbrezina/pullboard:latest',
            'Pull': 'newer',
            'PublishPort': '127.0.0.1:3333:8080',
            'Volume': '%h/.config/pullboard/config.yaml:/app/config.yaml:ro,Z',
            'UserNS': 'keep-id', 'User': '%U', 'Group': '%G',
            'ReadOnly': 'true', 'DropCapability': 'all', 'NoNewPrivileges': 'true',
            'Tmpfs': '/data:rw,nosuid,nodev,noexec,size=64m,mode=0700,U',
        })

    @unittest.skipUnless(GENERATOR, 'Podman Quadlet generator unavailable')
    def test_generator_emits_graphical_session_service(self):
        with tempfile.TemporaryDirectory(prefix='pullboard-generator-') as temporary:
            output = Path(temporary)
            result = subprocess.run(
                [GENERATOR, '--user', temporary, temporary, temporary],
                env=dict(os.environ, QUADLET_UNIT_DIRS=str(QUADLET.parent)),
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            service = output / 'pullboard.service'
            self.assertTrue(service.is_file(), result.stderr)
            self.assertTrue((output / 'graphical-session.target.wants/pullboard.service').is_symlink())
            self.assertFalse((output / 'default.target.wants').exists())
            generated = service.read_text()
            command = next(line.removeprefix('ExecStart=') for line in generated.splitlines()
                           if line.startswith('ExecStart='))
            self.assert_container_command(command)
            self.assertIn('PartOf=graphical-session.target', generated)
            self.assertNotIn('DBUS_SESSION_BUS_ADDRESS', generated)


if __name__ == '__main__':
    unittest.main()
