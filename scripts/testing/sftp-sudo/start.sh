#!/bin/sh
set -eu
ssh-keygen -A >/dev/null 2>&1
printf 'Port 22\nListenAddress 0.0.0.0\nPasswordAuthentication yes\nUsePAM no\nPermitRootLogin no\nSubsystem sftp /usr/lib/openssh/sftp-server\n' > /etc/ssh/sshd_config
case "$NYATERM_SUDO_CASE" in
  password) printf 'fixture ALL=(root) ALL\n' > /etc/sudoers.d/fixture ;;
  nopass|missing|limited) printf 'fixture ALL=(root) NOPASSWD: ALL\n' > /etc/sudoers.d/fixture ;;
  requiretty) printf 'Defaults:fixture requiretty\nfixture ALL=(root) ALL\n' > /etc/sudoers.d/fixture ;;
  denied) : ;;
  internal) printf 'ForceCommand internal-sftp\n' >> /etc/ssh/sshd_config ;;
  *) exit 2 ;;
esac
if [ "$NYATERM_SUDO_CASE" = missing ]; then mv /usr/lib/openssh/sftp-server /usr/lib/openssh/sftp-server-disabled; fi
if [ "$NYATERM_SUDO_CASE" = limited ]; then printf 'MaxSessions 1\n' >> /etc/ssh/sshd_config; fi
exec /usr/sbin/sshd -D -e
