FROM ubuntu:22.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends mosh openssh-server locales iproute2 && rm -rf /var/lib/apt/lists/* && mkdir -p /run/sshd /root/.ssh && chmod 700 /root/.ssh
RUN printf 'PermitRootLogin prohibit-password\nPasswordAuthentication no\n' > /etc/ssh/sshd_config.d/mosh-test.conf
ENV LANG=C.UTF-8
EXPOSE 22 60000/udp
CMD ["/usr/sbin/sshd", "-D", "-e"]
