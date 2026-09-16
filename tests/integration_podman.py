#!/usr/bin/env python3.12
"""Run interactive cleanup against a temporary Podman store, never the host store."""

import codecs
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
        return tuple(sorted(subprocess.check_output(
            [podman_exe, '--remote=false', kind, '--all', '--quiet', '--no-trunc'],
            text=True).splitlines()) for kind in ('ps', 'images'))

    before = host_snapshot()
    with tempfile.TemporaryDirectory(prefix='cleanup-podman-integration-') as temp:
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
            podman('build', '--pull=never', '-q', '-t', f'localhost/cleanup-{kind}:one', str(context))
            podman('tag', f'localhost/cleanup-{kind}:one', f'localhost/cleanup-{kind}:two')
            podman('create', '--network=none', '--name', f'cleanup-{kind}',
                   f'localhost/cleanup-{kind}:one')
        podman('volume', 'create', 'cleanup-preserved-volume')
        keep_tags = podman('image', 'inspect', '--format', '{{json .RepoTags}}',
                           'localhost/cleanup-keep:one')
        isolated_bin = root / 'bin'
        isolated_bin.mkdir()
        wrapper = isolated_bin / 'podman'
        wrapper.write_text('#!/bin/sh\n[ "${1-}" = --remote=false ] || exit 99\nshift\nexec '
                           + shlex.join(base) + ' "$@"\n')
        wrapper.chmod(0o755)
        env['PATH'] = str(isolated_bin) + os.pathsep + os.environ['PATH']
        command = [str(binary), '--containers-only', '--engine', 'podman']

        def interact(mode):
            master, slave = pty.openpty()
            process = subprocess.Popen(command, env=env, stdin=slave,
                                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            os.close(slave)
            output = ''
            pending = ''
            decoder = codecs.getincrementaldecoder('utf-8')()
            pattern = re.compile(
                r'  请选择 \[1/2/3，默认 3\]: |'
                r'  删除 (容器|镜像) ([^\n]+?)？\[y/N/q，q 跳过本组剩余\]: ')
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
                        if match.group(1) is None:
                            answer = {'all': '1', 'each': '2', 'skip': ''}[mode]
                        elif match.group(1) == '容器':
                            answer = 'y' if 'cleanup-drop' in match.group(2) else 'n'
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
        code, output = interact('skip')
        assert code == 0, output
        assert len(podman('ps', '--all', '--quiet').splitlines()) == 2
        print('PASS: dry run, noninteractive mode and default skip preserve resources', flush=True)

        code, output = interact('each')
        assert code == 1, (code, output)
        assert '保留镜像及标签' in output, output
        assert podman('ps', '--all', '--format', '{{.Names}}') == 'cleanup-keep'
        assert podman('image', 'inspect', '--format', '{{json .RepoTags}}',
                      'localhost/cleanup-keep:one') == keep_tags
        assert 'cleanup-drop' not in podman('images', '--format', '{{.Repository}}:{{.Tag}}')
        assert podman('volume', 'ls', '--format', '{{.Name}}') == 'cleanup-preserved-volume'
        print('PASS: individual choices, multiple tags and referenced-image protection', flush=True)

        code, output = interact('all')
        assert code == 0, (code, output)
        assert podman('ps', '--all', '--quiet') == ''
        assert podman('images', '--all', '--quiet') == ''
        assert podman('volume', 'ls', '--format', '{{.Name}}') == 'cleanup-preserved-volume'
        print('PASS: all selected resources deleted; volumes preserved', flush=True)

    assert host_snapshot() == before, 'Host resources changed during test'
    print('PASS: host containers/images unchanged')


if __name__ == '__main__':
    main()
