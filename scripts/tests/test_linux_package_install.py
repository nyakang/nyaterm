"""Opt-in package-manager smoke tests in disposable containers.

NYATERM_PACKAGE_CONTAINER_TESTS=1 python3 -m unittest scripts.tests.test_linux_package_install -v
Images must already exist locally. Only the containers' package databases change.
These tests exercise the external commands, not the GPUI dialog or Polkit agent.
"""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


FIXTURE = r'''
import pathlib, shutil, subprocess, sys, tempfile
kind = sys.argv[1]
root = pathlib.Path(tempfile.mkdtemp(prefix='nyaterm-updater-smoke-'))
root.chmod(0o755)
for version in ('1.0', '2.0', '3.0'):
    if kind == 'deb':
        package = root / version
        (package / 'DEBIAN').mkdir(parents=True)
        depends = 'Depends: nyaterm-nonexistent-fixture-dependency\n' if version == '3.0' else ''
        (package / 'DEBIAN/control').write_text(f'Package: nyaterm-update-fixture\nVersion: {version}\nArchitecture: all\n{depends}Maintainer: Test <test@example.com>\nDescription: isolated updater fixture\n')
        (package / 'opt/nyaterm-update-fixture').mkdir(parents=True)
        (package / 'opt/nyaterm-update-fixture/version').write_text(version)
        subprocess.run(['dpkg-deb', '--build', str(package), str(root / (version + '.deb'))], check=True)
    else:
        spec = root / (version + '.spec')
        requires = 'Requires: nyaterm-nonexistent-fixture-dependency\n' if version == '3.0' else ''
        spec.write_text(f'Name: nyaterm-update-fixture\nVersion: {version}\nRelease: 1\nSummary: Isolated updater fixture\nLicense: MIT\nBuildArch: noarch\n{requires}\n%description\nIsolated updater fixture\n%install\nmkdir -p %{{buildroot}}/opt/nyaterm-update-fixture\necho {version} > %{{buildroot}}/opt/nyaterm-update-fixture/version\n%files\n/opt/nyaterm-update-fixture\n')
        subprocess.run(['rpmbuild', '--define', f'_topdir {root}/rpmbuild', '-bb', str(spec)], check=True)
        shutil.copy(next((root / 'rpmbuild/RPMS/noarch').glob(f'*-{version}-1.noarch.rpm')), root / (version + '.rpm'))
subprocess.run(['dpkg' if kind == 'deb' else 'rpm', '-i', str(root / ('1.0.' + kind))], check=True)
subprocess.run(['useradd', '-m', 'fixture'], check=True)
subprocess.run(['chpasswd'], input='fixture:fixture-password\n', text=True, check=True)
sudoers = pathlib.Path('/etc/sudoers.d/nyaterm-fixture')
sudoers.write_text('fixture ALL=(root) ALL\n')
sudoers.chmod(0o440)
script = r"""
import pathlib, shutil, subprocess, sys
root, kind = pathlib.Path(sys.argv[1]), sys.argv[2]
def run(args, **kw):
    return subprocess.run(args, text=True, capture_output=True, **kw)
def preflight(version):
    args = ['apt-get', '--simulate', '--no-download', '--no-remove', '--no-install-recommends', '-o', 'Debug::NoLocking=1', 'install'] if kind == 'deb' else ['rpm', '--test', '-U']
    return run(args + [str(root / (version + '.' + kind))])
check = preflight('2.0')
assert check.returncode == 0, check.stderr
if kind == 'deb':
    for line in check.stdout.splitlines():
        if line.startswith(('Inst ', 'Conf ')):
            assert line.split()[1] == 'nyaterm-update-fixture', line
assert preflight('3.0').returncode != 0
command = [shutil.which('dpkg'), '--force-confdef', '--force-confold', '--install'] if kind == 'deb' else [shutil.which('rpm'), '-U']
command = ['sudo', '-S', '-k', '-p', '', '--'] + command + [str(root / ('2.0.' + kind))]
wrong = run(command, input='incorrect\n')
assert wrong.returncode != 0
assert (pathlib.Path('/opt/nyaterm-update-fixture/version').read_text().strip()) == '1.0'
installed = run(command, input='fixture-password\n')
assert installed.returncode == 0, installed.stderr
assert pathlib.Path('/opt/nyaterm-update-fixture/version').read_text().strip() == '2.0'
if kind == 'deb':
    status = run(['dpkg-query', '--show', '--showformat=${Version}\n${Status}\n', 'nyaterm-update-fixture'])
    assert status.stdout == '2.0\ninstall ok installed\n', status.stdout
else:
    status = run(['rpm', '-q', '--queryformat', '%{VERSION}-%{RELEASE}', 'nyaterm-update-fixture'])
    assert status.stdout == '2.0-1', status.stdout
print('PASS: unprivileged preflight, missing dependencies, wrong password, sudo install and upgrade verification')
"""
result = subprocess.run(['runuser', '-u', 'fixture', '--', 'python3', '-c', script, str(root), kind])
if result.returncode:
    raise SystemExit(result.returncode)
'''


@unittest.skipUnless(os.environ.get('NYATERM_PACKAGE_CONTAINER_TESTS') == '1',
                     'set NYATERM_PACKAGE_CONTAINER_TESTS=1 to run isolated package installation')
class LinuxPackageInstallTests(unittest.TestCase):
    def smoke(self, image: str, kind: str, setup: str) -> None:
        script = f"set -eu\n{setup}\npython3 - {kind} <<'PY'\n{FIXTURE}\nPY\n"
        with tempfile.TemporaryDirectory(prefix='nyaterm-updater-container-') as directory:
            cid = Path(directory) / 'container-id'
            try:
                result = subprocess.run(
                    ['docker', 'run', '--rm', '--pull=never', '--cidfile', str(cid), '-i', image, 'bash', '-s'],
                    input=script, text=True, capture_output=True, timeout=300,
                )
                self.assertEqual(result.returncode, 0, (result.stdout + result.stderr)[-6000:])
                self.assertIn('PASS:', result.stdout)
            finally:
                if cid.exists():
                    subprocess.run(['docker', 'rm', '--force', cid.read_text().strip()],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)

    def test_ubuntu_deb_upgrade_with_sudo(self) -> None:
        self.smoke('ubuntu:22.04', 'deb', 'export DEBIAN_FRONTEND=noninteractive\napt-get update -qq\napt-get install -y -qq sudo python3')

    def test_fedora_rpm_upgrade_with_sudo(self) -> None:
        self.smoke('fedora:latest', 'rpm', 'dnf -y install rpm-build python3 shadow-utils')
