FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends openssh-server sudo && rm -rf /var/lib/apt/lists/*
RUN useradd -m -u 1100 -s /bin/sh fixture && mkdir -p /run/sshd /fixture/root /fixture/user /fixture/public /fixture/readonly && chmod 555 /fixture/readonly && chmod 755 /fixture/public && chmod 700 /fixture/root && printf original > /fixture/root/file && chmod 600 /fixture/root/file && printf original > /fixture/user/file && chown -R 1100:1100 /fixture/user && chmod 640 /fixture/user/file
COPY start.sh /start.sh
ENTRYPOINT ["/bin/sh", "/start.sh"]
