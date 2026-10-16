#!/usr/bin/env python3.12
"""Run interactive cleanup against a temporary Podman store, never the host store."""

import codecs
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import subprocess
import sys
import tempfile
import time


def main():
    binary = Path(sys.argv[1]).resolve()
    podman_exe = shutil.which('podman')
    if not podman_exe:
        raise SystemExit('Podman is required for this integration test')

    def host_snapshot():
        commands = [('ps', '--all', '--quiet', '--no-trunc'),
                    ('images', '--all', '--quiet', '--no-trunc'),
                    ('volume', 'ls', '--quiet')]
        return tuple(sorted(subprocess.check_output(
            [podman_exe, '--remote=false', *args], text=True).splitlines())
            for args in commands)

    before = host_snapshot()
    with tempfile.TemporaryDirectory(prefix='cloacina-podman-integration-') as temp:
        root = Path(temp)
        test_home = root / 'home'
        test_home.mkdir()
        env = dict(os.environ, HOME=str(test_home))
        base = [podman_exe, '--remote=false', '--root', str(root / 'store'),
                '--runroot', str(root / 'run'), '--storage-driver', 'vfs',
                '--events-backend', 'file', '--tmpdir', str(root / 'tmp')]

        def podman(*args):
            result = subprocess.run([*base, *args], env=env, capture_output=True,
                                    text=True, timeout=60)
            if result.returncode:
                raise RuntimeError(f'{args}: {result.stderr}\n{result.stdout}')
            return result.stdout.strip()

        for kind in ('keep', 'drop'):
            context = root / kind
            context.mkdir()
            (context / 'marker').write_text(kind)
            (context / 'Containerfile').write_text(
                'FROM scratch\nCOPY marker /marker\nCMD ["/not-run"]\n')
            podman('build', '--pull=never', '-q', '-t', f'localhost/cloacina-{kind}:one', str(context))
            podman('tag', f'localhost/cloacina-{kind}:one', f'localhost/cloacina-{kind}:two')
            podman('volume', 'create', f'cloacina-{kind}-data')
            podman('create', '--network=none', '--name', f'cloacina-{kind}',
                   '--volume', f'cloacina-{kind}-data:/named', '--volume', '/anonymous',
                   f'localhost/cloacina-{kind}:one')
        podman('volume', 'create', 'cloacina-preserved-volume')
        podman('volume', 'create', 'cloacina-unused-volume')
        keep_volumes = {mount['Name'] for mount in json.loads(podman(
            'inspect', '--format', '{{json .Mounts}}', 'cloacina-keep'))
            if mount['Type'] == 'volume'}
        initial_volumes = set(podman('volume', 'ls', '--quiet').splitlines())
        assert len(initial_volumes) == 6, initial_volumes
        keep_tags = podman('image', 'inspect', '--format', '{{json .RepoTags}}',
                           'localhost/cloacina-keep:one')
        isolated_bin = root / 'bin'
        isolated_bin.mkdir()
        wrapper = isolated_bin / 'podman'
        wrapper.write_text('#!/bin/sh\n[ "${1-}" = --remote=false ] || exit 99\nshift\nexec '
                           + shlex.join(base) + ' "$@"\n')
        wrapper.chmod(0o755)
        env['PATH'] = str(isolated_bin) + os.pathsep + os.environ['PATH']
        command = [str(binary), '--containers-only', '--engine', 'podman']

        def interact(mode, volume_mode=None):
            master, slave = pty.openpty()
            process = subprocess.Popen(command, env=env, stdin=slave,
                                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            os.close(slave)
            output = ''
            pending = ''
            decoder = codecs.getincrementaldecoder('utf-8')()
            pattern = re.compile(
                r'  (容器|镜像|卷)：1\) 全部删除[^\n]*\n'
                r'  请选择 \[1/2/3，默认 3\]: |'
                r'  删除 (容器|镜像|卷) ([^\n]+?)？\[y/N/q，q 跳过本组剩余\]: ')
            deadline = time.monotonic() + 60
            try:
                while True:
                    if time.monotonic() > deadline:
                        raise TimeoutError(output)
                    if not select.select([process.stdout], [], [], 1)[0]:
                        continue
                    chunk = os.read(process.stdout.fileno(), 65536)
                    if not chunk:
                        break
                    text = decoder.decode(chunk)
                    output += text
                    pending += text
                    while (match := pattern.search(pending)):
                        if match.group(1) is not None:
                            choice = volume_mode if match.group(1) == '卷' and volume_mode else mode
                            answer = {'all': '1', 'each': '2', 'skip': ''}[choice]
                        elif match.group(2) == '容器':
                            answer = 'y' if 'cloacina-drop' in match.group(3) else 'n'
                        elif match.group(2) == '卷':
                            answer = 'n' if 'cloacina-preserved-volume' in match.group(3) else 'y'
                        else:
                            answer = 'y'
                        os.write(master, (answer + '\n').encode())
                        pending = pending[match.end():]
                return process.wait(timeout=5), output
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                os.close(master)

        for extra in (['-n'], []):
            result = subprocess.run([*command, *extra], env=env, input='1\n1\n',
                                    capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, result.stdout + result.stderr
            assert len(podman('ps', '--all', '--quiet').splitlines()) == 2
            assert set(podman('volume', 'ls', '--quiet').splitlines()) == initial_volumes
            if extra:
                assert 'cloacina-unused-volume' in result.stdout
                assert 'cloacina-keep-data' not in result.stdout
        code, output = interact('skip')
        assert code == 0, output
        assert len(podman('ps', '--all', '--quiet').splitlines()) == 2
        assert set(podman('volume', 'ls', '--quiet').splitlines()) == initial_volumes
        print('PASS: dry run, noninteractive mode and default skip preserve resources', flush=True)

        code, output = interact('each')
        assert code == 1, (code, output)
        assert '保留镜像及标签' in output, output
        assert podman('ps', '--all', '--format', '{{.Names}}') == 'cloacina-keep'
        assert podman('image', 'inspect', '--format', '{{json .RepoTags}}',
                      'localhost/cloacina-keep:one') == keep_tags
        assert 'cloacina-drop' not in podman('images', '--format', '{{.Repository}}:{{.Tag}}')
        expected = keep_volumes | {'cloacina-preserved-volume'}
        assert set(podman('volume', 'ls', '--quiet').splitlines()) == expected
        print('PASS: individual choices; unused named/anonymous volumes deleted; '
              'stopped-container volumes, skipped volume and referenced images preserved', flush=True)

        code, output = interact('all', volume_mode='skip')
        assert code == 0, (code, output)
        assert podman('ps', '--all', '--quiet') == ''
        assert podman('images', '--all', '--quiet') == ''
        assert set(podman('volume', 'ls', '--quiet').splitlines()) == expected
        assert 'cloacina-keep-data' in output, output
        print('PASS: volume choices are independent; newly orphaned volumes listed', flush=True)

        code, output = interact('all')
        assert code == 0, (code, output)
        assert podman('volume', 'ls', '--quiet') == ''
        print('PASS: all selected unused volumes deleted', flush=True)

    assert host_snapshot() == before, 'Host resources changed during test'
    print('PASS: host containers/images/volumes unchanged')


if __name__ == '__main__':
    main()
