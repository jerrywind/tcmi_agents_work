# rrserver 运行镜像（直接打包预编译的 Linux 二进制，不在容器内编译）。
#
# 【构建上下文】必须是 workspace 根（server/），而非本文件所在目录：
#     docker build -f rrserver/Dockerfile ..      # 或由 deploy/docker-compose.yml 编排
# 因为 rrserver 与 harness 同属一个 workspace，release 产物统一落在
# **server/target/release/**，子 crate 没有独立 target/。
# 上下文裁剪由 server/.dockerignore 负责。
#
# 【为什么要预编译】Docker 构建容器内网络会损坏 crates.io 下载（manifest 解析失败，
# exit 101），无法在容器内 `cargo build`。改为在 **WSL2** 的 Linux 环境预先编译：
#     wsl -e bash -c '. "$HOME/.cargo/env"; cd /mnt/d/labs/windblue_tech/tcm_work/server && cargo build --release'
# 或使用仓库脚本：pwsh scripts/build-release.ps1
# 生成的 Linux 二进制 server/target/release/rrserver 即被本 Dockerfile 使用。
#
# TLS 全部走 rustls（reqwest / tokio-tungstenite 均用 *-tls），运行时【无需系统 OpenSSL】。
# 以非 root 用户（uid 10001）运行，降低被攻陷后的影响面。

# ubuntu:24.04 的 glibc 2.39 与 WSL2 编译环境一致（debian:bookworm 为 2.36，会报
# "GLIBC_2.39 not found"）。
FROM ubuntu:24.04

# rustls 不需要 OpenSSL；ca-certificates 用于对外 TLS 握手。
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 rrserver

WORKDIR /data

# 预编译的 Linux release 二进制（路径相对于 workspace 根上下文）
COPY target/release/rrserver /usr/local/bin/rrserver

# 默认配置：使镜像不挂载配置文件也能启动（便于 `docker run` 快速试用）。
# 安全处理：把模板里的示例 token 清空——未配置 token 的隧道无法被注册，
# 避免沿用 "change-me-strong-token" 这类弱口令。
# **生产必须挂载自己的配置**（见 deploy/docker-compose.yml）：
#   ../server/rrserver/config/rrserver.toml:/etc/rrserver.toml:ro
COPY rrserver/config/rrserver.toml.example /etc/rrserver.toml
RUN sed -i 's/^token = .*/token = ""/' /etc/rrserver.toml

USER rrserver

# 容器内必须监听 0.0.0.0，否则同网络下的 nginx 无法连通
EXPOSE 8080

# 健康检查：仅对默认 `server` 子命令有效（client / llm-server 不监听 8080，
# 用作这些子命令时可用 `docker run --no-healthcheck` 覆盖）。
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/8080' || exit 1

ENTRYPOINT ["rrserver"]
# 默认以云端 server 子命令启动；client / llm-server 可在 `docker run ... <subcmd>` 时覆盖。
CMD ["server", "--listen", "0.0.0.0:8080", "--config", "/etc/rrserver.toml"]
