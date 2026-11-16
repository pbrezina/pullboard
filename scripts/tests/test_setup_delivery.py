"""Offline launcher/release packaging checks; no Cargo, downloads or services."""
import errno
import fcntl
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pty
import signal
import subprocess
import tempfile
import termios
import textwrap
import time
import unittest


ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'scripts/pullboard-setup.sh'
ASSET = 'pullboard-setup-linux-x86_64.gz'
SPEC = importlib.util.spec_from_file_location(
    'setup_dependencies', ROOT / 'scripts/check-setup-dependencies.py')
DEPENDENCIES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DEPENDENCIES)


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='pullboard-delivery-')
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.commands = self.directory / 'commands'
        self.commands.mkdir()
        self.downloads = self.directory / 'downloads'
        self.downloads.mkdir()
        self.fixtures = self.directory / 'fixtures'
        self.fixtures.mkdir()
        self.result = self.directory / 'result.json'
        self.requests = self.directory / 'requests.jsonl'
        self.environment = dict(
            os.environ, PATH=str(self.commands) + os.pathsep + os.environ['PATH'],
            TMPDIR=str(self.downloads), MOCK_FIXTURES=str(self.fixtures),
            MOCK_RESULT=str(self.result), MOCK_REQUESTS=str(self.requests),
            MOCK_EXIT='0', MOCK_SYSTEM='Linux', MOCK_ARCH='x86_64',
            MOCK_DOWNLOAD_FAIL='', MOCK_WAIT='',
        )
        self.command('uname', '''import os, sys
print(os.environ['MOCK_SYSTEM' if sys.argv[1] == '-s' else 'MOCK_ARCH'])
''')
        self.command('curl', '''import json, os, shutil, sys
from pathlib import Path
args = sys.argv[1:]
assert args[0] == '--disable'
assert args[args.index('--proto') + 1] == '=https'
assert args[args.index('--proto-redir') + 1] == '=https'
assert '--fail' in args and '--location' in args
url = args[-1]
with open(os.environ['MOCK_REQUESTS'], 'a') as log:
    log.write(json.dumps(url) + '\\n')
if os.environ['MOCK_DOWNLOAD_FAIL']:
    sys.exit(22)
source = Path(os.environ['MOCK_FIXTURES']) / url.rsplit('/', 1)[-1]
shutil.copyfile(source, args[args.index('--output') + 1])
''')
        self.payload = b'''#!/usr/bin/env python3
import json, os, stat, sys, time
from pathlib import Path
binary = Path(sys.argv[0])
Path(os.environ['MOCK_RESULT']).write_text(json.dumps({
    'args': sys.argv[1:], 'tty': os.isatty(0), 'binary': str(binary),
    'mode': stat.S_IMODE(binary.stat().st_mode),
    'directory_mode': stat.S_IMODE(binary.parent.stat().st_mode),
}))
if os.environ['MOCK_WAIT']:
    time.sleep(20)
sys.exit(int(os.environ['MOCK_EXIT']))
'''
        self.set_payload(self.payload)

    def set_payload(self, payload):
        self.asset = gzip.compress(payload, mtime=0)
        (self.fixtures / ASSET).write_bytes(self.asset)
        self.manifest(hashlib.sha256(self.asset).hexdigest())

    def command(self, name, program):
        path = self.commands / name
        path.write_text('#!/usr/bin/env python3\n' + program)
        path.chmod(0o700)

    def manifest(self, checksum):
        (self.fixtures / 'SHA256SUMS').write_text(f'{checksum}  {ASSET}\n')

    def launch(self, *arguments, terminal=True, shell='sh', piped=False):
        if not terminal:
            return subprocess.run(['sh', str(LAUNCHER), *arguments],
                                  env=self.environment, stdin=subprocess.DEVNULL,
                                  capture_output=True, text=True, timeout=10)
        process, master = self.start(*arguments, shell=shell, piped=piped)
        self.addCleanup(os.close, master)
        process.wait(timeout=10)
        output = bytearray()
        while True:
            try:
                part = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not part:
                break
            output.extend(part)
        return subprocess.CompletedProcess(process.args, process.returncode,
                                           output.decode(), '')

    def start(self, *arguments, shell='sh', piped=False):
        master, slave = pty.openpty()

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(1, termios.TIOCSCTTY, 0)

        # Deliberately start with /dev/null stdin, like curl | sh. The launcher
        # must reopen /dev/tty rather than inherit its consumed script input.
        try:
            process = subprocess.Popen(
                ([shell, '-s', '--', *arguments] if piped else [shell, str(LAUNCHER), *arguments]),
                env=self.environment,
                stdin=subprocess.PIPE if piped else subprocess.DEVNULL, stdout=slave, stderr=slave,
                preexec_fn=controlling_terminal,
            )
            if piped:
                process.stdin.write(LAUNCHER.read_bytes())
                process.stdin.close()
        finally:
            os.close(slave)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        return process, master

    def assert_cleaned(self):
        self.assertEqual(list(self.downloads.iterdir()), [])

    def test_stable_checksums_private_downloads_tty_and_quoted_arguments(self):
        arguments = ['--jira-cloud-url', 'https://example.atlassian.net', '--no-color']
        result = self.launch(*arguments)
        self.assertEqual(result.returncode, 0, result.stdout)
        executed = json.loads(self.result.read_text())
        self.assertEqual(executed['args'], arguments)
        self.assertTrue(executed['tty'])
        self.assertEqual(executed['mode'], 0o700)
        self.assertEqual(executed['directory_mode'], 0o700)
        requests = [json.loads(line) for line in self.requests.read_text().splitlines()]
        self.assertEqual(requests, [
            'https://github.com/pbrezina/pullboard/releases/latest/download/SHA256SUMS',
            f'https://github.com/pbrezina/pullboard/releases/latest/download/{ASSET}',
        ])
        self.assertFalse(Path(executed['binary']).exists())
        self.assert_cleaned()

    def test_nightly_selects_prerelease_and_forwards_flag(self):
        result = self.launch('--nightly', '--jira-cloud-url=https://example.atlassian.net')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('/releases/download/nightly/', self.requests.read_text())
        self.assertEqual(json.loads(self.result.read_text())['args'],
                         ['--nightly', '--jira-cloud-url=https://example.atlassian.net'])
        self.assert_cleaned()

    def test_all_team_jira_flags_forward_exact_argv_in_both_forms(self):
        flags = [
            ('--jira-url', 'https://example.atlassian.net'),
            ('--jira-projects', 'TWO, ONE,TWO'),
            ('--jira-board-id', '42'),
            ('--jira-board-id', '43'),
            ('--jira-groups', 'engineering,platform team'),
            ('--jira-team-field', 'Assigned Team'),
            ('--jira-team', 'Platform Team'),
            ('--jira-sprint-name-pattern', r'(?i)^Platform \d+ $'),
            ('--jira-sprint-name-pattern', r'^Other[0-9]{1,3}$'),
        ]
        for equals in [False, True]:
            with self.subTest(equals=equals):
                arguments = ([f'{flag}={value}' for flag, value in flags] if equals
                             else [arg for pair in flags for arg in pair])
                result = self.launch(*arguments)
                self.assertEqual(result.returncode, 0, result.stdout)
                self.assertEqual(json.loads(self.result.read_text())['args'], arguments)
                self.assert_cleaned()

    def test_bash_stdin_launcher_restores_tty_and_preserves_team_arguments(self):
        marker = self.directory / 'injected'
        arguments = ['--jira-url=https://example.atlassian.net', '--jira-projects=DEV,OPS',
                     '--jira-board-id=42', '--jira-board-id', '43',
                     '--jira-groups=engineering,reviewers',
                     '--jira-team-field=AssignedTeam', '--jira-team', 'Platform Team',
                     '--jira-sprint-name-pattern', f'^Team $(touch "{marker}")',
                     '--jira-sprint-name-pattern=^Other[0-9]{1,3}$']
        result = self.launch(*arguments, shell='bash', piped=True)
        self.assertEqual(result.returncode, 0, result.stdout)
        executed = json.loads(self.result.read_text())
        self.assertEqual(executed['args'], arguments)
        self.assertTrue(executed['tty'])
        self.assertFalse(marker.exists())
        self.assert_cleaned()

    def test_team_jira_options_reject_missing_empty_and_conflicting_flags(self):
        flags = ['--jira-url', '--jira-projects', '--jira-board-id', '--jira-groups',
                 '--jira-team-field', '--jira-team', '--jira-sprint-name-pattern']
        for flag in flags:
            invalid = [(flag,), (flag + '=',), (flag, '--nightly'), (flag, '')]
            if flag not in ['--jira-board-id', '--jira-sprint-name-pattern']:
                invalid.append((flag + '=value', flag, 'value'))
            for arguments in invalid:
                with self.subTest(arguments=arguments):
                    result = self.launch(*arguments, terminal=False)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertNotIn('value', result.stderr.replace('nonempty value', ''))
        for arguments in [('--jira-url=a', '--jira-cloud-url=b'),
                          ('--jira-cloud-url=a', '--jira-url=b')]:
            self.assertNotEqual(self.launch(*arguments, terminal=False).returncode, 0)
        self.assertFalse(self.requests.exists())
        self.assertFalse(self.result.exists())

    def test_repeatable_jira_options_still_require_each_value(self):
        for flag, value in [('--jira-board-id', '42'),
                            ('--jira-sprint-name-pattern', '^Team[0-9]{1,3}$')]:
            for suffix in [(flag,), (flag + '=',), (flag, ''), (flag, '--nightly')]:
                with self.subTest(flag=flag, suffix=suffix):
                    result = self.launch(flag, value, *suffix, terminal=False)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('nonempty value', result.stderr)
        self.assertFalse(self.requests.exists())
        self.assertFalse(self.result.exists())

    def test_argument_is_never_evaluated_by_shell(self):
        marker = self.directory / 'injected'
        argument = f'--jira-cloud-url=https://example.atlassian.net/$(touch "{marker}"); echo no'
        result = self.launch(argument)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(json.loads(self.result.read_text())['args'], [argument])
        self.assertFalse(marker.exists())

    def test_wizard_exit_code_is_propagated(self):
        self.environment['MOCK_EXIT'] = '37'
        result = self.launch()
        self.assertEqual(result.returncode, 37, result.stdout)
        self.assert_cleaned()

    def test_bad_checksum_never_chmods_or_executes(self):
        self.manifest('0' * 64)
        self.command('chmod', "raise AssertionError('must not chmod unverified data')\n")
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Checksum mismatch', result.stdout)
        self.assertFalse(self.result.exists())
        self.assert_cleaned()

    def test_malformed_missing_duplicate_and_path_checksum_entries_fail_closed(self):
        good = hashlib.sha256(self.asset).hexdigest()
        manifests = [f'not-a-hash  {ASSET}\n', f'{good}  ../{ASSET}\n',
                     f'{good}  {ASSET}\n{good}  {ASSET}\n', '']
        for manifest in manifests:
            with self.subTest(manifest=manifest):
                (self.fixtures / 'SHA256SUMS').write_text(manifest)
                result = self.launch()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('Invalid release checksum manifest', result.stdout)
                self.assertFalse(self.result.exists())
                self.assert_cleaned()

    def test_download_failure_cleans_up(self):
        self.environment['MOCK_DOWNLOAD_FAIL'] = 'yes'
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Download failed', result.stdout)
        self.assertFalse(self.result.exists())
        self.assert_cleaned()

    def test_verified_but_invalid_gzip_is_not_executed(self):
        (self.fixtures / ASSET).write_bytes(b'not gzip')
        self.manifest(hashlib.sha256(b'not gzip').hexdigest())
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Cannot decompress', result.stdout)
        self.assertFalse(self.result.exists())
        self.assert_cleaned()

    def test_help_and_invalid_options_do_not_download_or_echo_secrets(self):
        result = self.launch('--help', terminal=False)
        self.assertEqual(result.returncode, 0)
        self.assertIn('--nightly', result.stdout)
        self.assertIn('My board first', result.stdout)
        self.assertIn('My pattern first', result.stdout)
        self.assertIn('never comma-split', result.stdout)
        for arguments in [('--token=DO_NOT_ECHO',), ('--jira-cloud-url',),
                          ('--jira-cloud-url=',), ('--jira-cloud-url', '--nightly')]:
            with self.subTest(arguments=arguments):
                result = self.launch(*arguments, terminal=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('DO_NOT_ECHO', result.stderr)
        self.assertFalse(self.requests.exists())

    def test_unsupported_platform_and_noninteractive_terminal_fail_before_download(self):
        for system, arch in [('Darwin', 'x86_64'), ('Linux', 'aarch64')]:
            with self.subTest(system=system, arch=arch):
                self.environment.update(MOCK_SYSTEM=system, MOCK_ARCH=arch)
                result = self.launch()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('Only Linux x86-64', result.stdout)
        self.environment.update(MOCK_SYSTEM='Linux', MOCK_ARCH='x86_64')
        result = self.launch(terminal=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('interactive terminal', result.stderr)
        self.assertFalse(self.requests.exists())

    def assert_graceful_cancellation(self, requested_signal, *, process_group):
        self.result.unlink(missing_ok=True)
        cancellation = self.directory / ('cancel-' + requested_signal.name)
        cancellation.mkdir()
        self.environment['MOCK_CANCELLATION'] = str(cancellation)
        self.environment['MOCK_EXIT'] = '23'
        self.set_payload(b'''#!/usr/bin/env python3
import json, os, signal, sys, time
from pathlib import Path
binary = Path(sys.argv[0])
cancellation = Path(os.environ['MOCK_CANCELLATION'])
requested = None
def cancel(signum, frame):
    global requested
    if requested is None:
        requested = signum
        (cancellation / 'requested').write_text(str(signum))
for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
    signal.signal(signum, cancel)
result = Path(os.environ['MOCK_RESULT'])
staged = result.with_suffix('.tmp')
staged.write_text(json.dumps({
    'pid': os.getpid(), 'binary': str(binary), 'tty': os.isatty(0),
}))
staged.replace(result)
while requested is None or not (cancellation / 'finish').exists():
    time.sleep(0.01)
(cancellation / 'completed').write_text(json.dumps({
    'binary_exists': binary.exists(), 'exit': int(os.environ['MOCK_EXIT']),
}))
sys.exit(int(os.environ['MOCK_EXIT']))
''')
        process, master = self.start()
        self.addCleanup(os.close, master)

        def wait_for(path):
            deadline = time.monotonic() + 5
            while not path.exists() and process.poll() is None and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(path.exists(), f'Wizard did not reach {path.name}')

        try:
            wait_for(self.result)
            child = json.loads(self.result.read_text())
            self.assertTrue(child['tty'])
            send_signal = os.killpg if process_group else os.kill
            send_signal(process.pid, requested_signal)
            wait_for(cancellation / 'requested')
            forwarded_signal = signal.SIGTERM if requested_signal == signal.SIGHUP else requested_signal
            self.assertEqual(int((cancellation / 'requested').read_text()), forwarded_signal)
            # Repeated cancellation must also keep waiting, not race cleanup.
            send_signal(process.pid, requested_signal)
            time.sleep(0.05)
            self.assertIsNone(process.poll())
            self.assertTrue(Path(child['binary']).exists())
            self.assertFalse((cancellation / 'completed').exists())
            (cancellation / 'finish').touch()
            self.assertEqual(process.wait(timeout=5), 128 + requested_signal)
            completion = json.loads((cancellation / 'completed').read_text())
            self.assertTrue(completion['binary_exists'])
            self.assertEqual(completion['exit'], 23)
            with self.assertRaises(ProcessLookupError):
                os.kill(child['pid'], 0)
            self.assert_cleaned()
        finally:
            # On regression, let only this synthetic wizard finish gracefully.
            (cancellation / 'finish').touch()
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=5)

    def test_signal_cleans_downloads(self):
        self.assert_graceful_cancellation(signal.SIGTERM, process_group=True)

    def test_launcher_pid_term_cancels_wizard_and_waits_before_cleanup(self):
        self.assert_graceful_cancellation(signal.SIGTERM, process_group=False)

    def test_launcher_pid_hup_and_int_are_forwarded_before_cleanup(self):
        for requested_signal in (signal.SIGHUP, signal.SIGINT):
            with self.subTest(signal=requested_signal):
                self.assert_graceful_cancellation(requested_signal, process_group=False)


class PackagingTests(unittest.TestCase):
    def test_package_preserves_tested_bytes_predictable_names_sizes_and_checksums(self):
        with tempfile.TemporaryDirectory(prefix='pullboard-package-') as temporary:
            directory = Path(temporary)
            binary = directory / 'tested executable'
            payload = b'#!/bin/sh\nexit 0\n'
            binary.write_bytes(payload)
            binary.chmod(0o700)
            output = directory / 'release assets'
            result = subprocess.run(
                ['sh', str(ROOT / 'scripts/package-setup.sh'), str(binary), str(output)],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual({path.name for path in output.iterdir()},
                             {ASSET, 'pullboard-setup.sh', 'SHA256SUMS', 'setup-sizes.txt'})
            self.assertEqual(gzip.decompress((output / ASSET).read_bytes()), payload)
            self.assertEqual(binary.read_bytes(), payload)
            self.assertEqual((output / 'pullboard-setup.sh').read_bytes(), LAUNCHER.read_bytes())
            compressed = (output / ASSET).stat().st_size
            self.assertEqual(result.stdout,
                             f'Setup executable: {len(payload)} bytes raw; {compressed} bytes gzip.\n')
            for line in (output / 'SHA256SUMS').read_text().splitlines():
                checksum, name = line.split()
                self.assertEqual(hashlib.sha256((output / name).read_bytes()).hexdigest(), checksum)
            first = (output / ASSET).read_bytes()
            os.utime(binary, (12345, 12345))
            subprocess.run(['sh', str(ROOT / 'scripts/package-setup.sh'), str(binary), str(output)],
                           check=True, capture_output=True)
            self.assertEqual((output / ASSET).read_bytes(), first)

    def test_missing_executable_is_rejected(self):
        result = subprocess.run(['sh', str(ROOT / 'scripts/package-setup.sh')],
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)


class DeliveryPolicyTests(unittest.TestCase):
    def test_default_runtime_graph_gate_rejects_server_database_keyring_and_sdks(self):
        allowed = 'pullboard-setup v0.1.0\npullboard-core v0.1.0\nreqwest v0.13.0\n'
        self.assertEqual(DEPENDENCIES.forbidden_packages(allowed), [])
        for package in ['axum', 'axum-core', 'sqlx', 'sqlx-sqlite', 'libsqlite3-sys',
                        'rusqlite', 'keyring', 'linux-native-keyring-store',
                        'secret-service', 'zbus', 'octocrab', 'gitlab']:
            with self.subTest(package=package):
                self.assertEqual(DEPENDENCIES.forbidden_packages(allowed + package + ' v1.0.0\n'),
                                 [package])
        result = subprocess.run(['python3', str(ROOT / 'scripts/check-setup-dependencies.py')],
                                input='', capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)

    def test_release_assets_depend_on_successfully_published_matching_image(self):
        workflow = (ROOT / '.github/workflows/publish.yml').read_text()
        self.assertIn('needs: [ghcr, setup]', workflow)
        self.assertIn("if: needs.ghcr.outputs.published == 'true'", workflow)
        image, assets = workflow.split('  setup-assets:', 1)
        self.assertNotIn('contents: write', image)
        self.assertIn('contents: write', assets)
        self.assertEqual(workflow.count('packages: write'), 1)
        self.assertEqual(workflow.count('contents: write'), 1)
        self.assertIn("echo 'published=true'", image)
        self.assertIn('master_sha', image)
        self.assertIn('master_sha', assets)
        self.assertIn('!github.event.release.prerelease', image)
        self.assertIn('--prerelease --latest=false', assets)
        self.assertNotIn('pull_request:', workflow)

    def test_pr_checks_reuse_read_only_setup_build_without_publication(self):
        verify = (ROOT / '.github/workflows/ci.yml').read_text()
        build = (ROOT / '.github/workflows/setup-build.yml').read_text()
        self.assertIn('pull_request:', verify)
        self.assertIn('uses: ./.github/workflows/setup-build.yml', verify)
        self.assertIn('workflow_call:', build)
        for workflow in [verify, build]:
            self.assertNotIn('contents: write', workflow)
            self.assertNotIn('packages: write', workflow)
        self.assertIn('toolchain install 1.99.1', build)
        self.assertIn('musl-tools cmake', build)
        self.assertIn('cargo +1.99.1 test --manifest-path setup/Cargo.toml --locked --release --target', build)
        self.assertIn('"$binary" --help', build)
        self.assertIn('sh scripts/package-setup.sh setup/target/', build)


class PublisherTests(unittest.TestCase):
    """Run the actual workflow shell blocks with local git/docker/gh fakes."""
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='pullboard-publication-')
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.commands = self.directory / 'commands'
        self.commands.mkdir()
        self.log = self.directory / 'calls.jsonl'
        self.output = self.directory / 'output'
        self.assets = self.directory / 'setup-release'
        self.assets.mkdir()
        for name in [ASSET, 'pullboard-setup.sh', 'SHA256SUMS', 'setup-sizes.txt']:
            (self.assets / name).write_text('synthetic test data, not a release artifact')
        self.environment = dict(
            os.environ, PATH=str(self.commands) + os.pathsep + os.environ['PATH'],
            GITHUB_SHA='a' * 40, MOCK_MASTER_SHA='a' * 40,
            MOCK_LOG=str(self.log), GITHUB_OUTPUT=str(self.output),
            RUNNER_TEMP=str(self.directory), EVENT_NAME='push',
            GH_REPO='pbrezina/pullboard', GH_TOKEN='synthetic-test-value',
            MOCK_RELEASE_EXISTS='', MOCK_FAIL='', RELEASE_TAG='',
            IMAGE_TAGS='ghcr.io/pbrezina/pullboard:nightly',
        )
        mock = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['MOCK_LOG'], 'a') as log:
    log.write(json.dumps([name, *args]) + '\\n')
if name == 'git':
    print(os.environ['MOCK_MASTER_SHA'] + '\\trefs/heads/master')
elif name == 'gh' and args[:2] == ['release', 'view']:
    sys.exit(0 if os.environ['MOCK_RELEASE_EXISTS'] else 1)
elif os.environ['MOCK_FAIL'] and os.environ['MOCK_FAIL'] in args:
    sys.exit(1)
'''
        for name in ['git', 'docker', 'gh']:
            command = self.commands / name
            command.write_text(mock)
            command.chmod(0o700)

    def block(self, name):
        workflow = (ROOT / '.github/workflows/publish.yml').read_text()
        step = workflow.split('      - name: ' + name + '\n', 1)[1]
        script = step.split('        run: |\n', 1)[1]
        lines = []
        for line in script.splitlines():
            if line and not line.startswith('          '):
                break
            lines.append(line)
        return textwrap.dedent('\n'.join(lines))

    def publish(self, assets=False):
        name = ('Attach versioned assets or update rolling nightly prerelease' if assets
                else 'Publish the exact tested image')
        return subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', self.block(name)],
                              cwd=self.directory, env=self.environment,
                              capture_output=True, text=True, timeout=10)

    def calls(self, command):
        return [call[1:] for line in self.log.read_text().splitlines()
                if (call := json.loads(line))[0] == command]

    def test_superseded_push_publishes_neither_image_nor_assets(self):
        self.environment['MOCK_MASTER_SHA'] = 'b' * 40
        for assets in [False, True]:
            result = self.publish(assets=assets)
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls('docker'), [])
        self.assertEqual(self.calls('gh'), [])
        self.assertFalse(self.output.exists())

    def test_image_success_is_explicit_and_failed_push_never_reports_success(self):
        self.environment['MOCK_FAIL'] = 'push'
        result = self.publish()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.output.exists())
        self.environment['MOCK_FAIL'] = ''
        result = self.publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_text(), 'published=true\n')

    def test_nightly_creation_is_prerelease_not_latest_and_uploads_predictable_assets(self):
        result = self.publish(assets=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls('gh')
        creation = next(call for call in calls if call[:2] == ['release', 'create'])
        self.assertEqual(creation[2], 'nightly')
        self.assertIn('--prerelease', creation)
        self.assertIn('--latest=false', creation)
        self.assertEqual(creation[creation.index('--target') + 1], 'a' * 40)
        upload = calls[-1]
        self.assertEqual(upload[:3], ['release', 'upload', 'nightly'])
        self.assertEqual({Path(path).name for path in upload[3:-1]},
                         {ASSET, 'pullboard-setup.sh', 'SHA256SUMS', 'setup-sizes.txt'})
        self.assertEqual(upload[-1], '--clobber')

    def test_existing_nightly_moves_source_tag_without_creating_second_release(self):
        self.environment['MOCK_RELEASE_EXISTS'] = 'yes'
        result = self.publish(assets=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls('gh')
        update = next(call for call in calls if call[0] == 'api')
        self.assertIn('repos/pbrezina/pullboard/git/refs/tags/nightly', update)
        self.assertIn('sha=' + 'a' * 40, update)
        self.assertIn('force=true', update)
        self.assertFalse(any(call[:2] == ['release', 'create'] for call in calls))
        self.assertIn('--latest=false', next(call for call in calls if call[:2] == ['release', 'edit']))

    def test_nightly_release_failure_does_not_upload_assets(self):
        self.environment['MOCK_FAIL'] = 'create'
        result = self.publish(assets=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(call[:2] == ['release', 'upload'] for call in self.calls('gh')))

    def test_versioned_release_only_uploads_to_existing_release_and_all_image_tags(self):
        self.environment.update(EVENT_NAME='release', RELEASE_TAG='v1.2.3',
                                IMAGE_TAGS='ghcr.io/pbrezina/pullboard:v1.2.3\nghcr.io/pbrezina/pullboard:latest')
        result = self.publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call[1] for call in self.calls('docker') if call[0] == 'push'],
                         ['ghcr.io/pbrezina/pullboard:v1.2.3', 'ghcr.io/pbrezina/pullboard:latest'])
        result = self.publish(assets=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls('gh')
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0][:3], ['release', 'upload', 'v1.2.3'])
        self.assertEqual(self.calls('git'), [])


if __name__ == '__main__':
    unittest.main()
