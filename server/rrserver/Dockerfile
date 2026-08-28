# rrserver 运行镜像（直接打包预编译的 Linux 二进制）。
#
# 说明：Docker 构建容器内网络会损坏 crates.io 下载（manifest 解析失败，exit 101），
# 无法在容器内 `cargo build`。改为在 **WSL2** 的 Linux 环境预先编译：
#     wsl -e bash -c '. "$HOME/.cargo/env"; cd /mnt/d/labs/windblue_tech/tcm_work/rrserver; cargo build --release'
# 生成的 Linux 二进制 rrserver/target/release/rrserver 即被本 Dockerfile 使用。
#
# 依赖说明：本项目 TLS 全部走 rustls（reqwest / tokio-tungstenite 均用 *-tls），
# 运行时【无需系统 OpenSSL】；镜像内仅安装 ca-certificates 即可对外建立 TLS 连接。
# 运行时以非 root 用户（uid 10001）运行，降低被攻陷后的影响面。

# 使用 ubuntu:24.04 作为运行镜像：其 glibc 2.39 与 WSL2 编译环境一致，
# 避免 "GLIBC_2.39 not found" 的 ABI 不兼容（debian:bookworm 的 glibc 为 2.36）。
FROM ubuntu:24.04

# rustls 不需要 OpenSSL；ca-certificates 用于对外 TLS 握手。
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 rrserver

WORKDIR /data
# 直接使用 WSL2 预编译的 Linux release 二进制（避免容器内编译的网络问题）。
COPY target/release/rrserver /usr/local/bin/rrserver

# 以非 root 用户运行
USER rrserver

# rrserver 在容器内应监听 0.0.0.0，才能被同网络下的 nginx 访问（默认 127.0.0.1 仅本容器回环）。
EXPOSE 8080
ENTRYPOINT ["rrserver"]
# 默认以云端 server 子命令启动；client / llm-server 可在 `docker run ... <subcmd>` 时覆盖。
CMD ["server", "--listen", "0.0.0.0:8080", "--config", "/etc/rrserver.toml"]
