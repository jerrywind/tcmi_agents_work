# rrserver 运行镜像 —— 多阶段构建，**在 Docker 内完成编译**。
#
# 【构建上下文】必须是 workspace 根（server/），而非本文件所在目录：
#     cd server && docker build -f rrserver/Dockerfile -t tcm-rrserver:local .
#
# 【为什么在 Docker 内编译】后端**完全依赖 Docker** 构建、运行与验证，
# 不使用宿主机/WSL2 的本地产物。
#
# 【glibc 兼容性】构建镜像 rust:1.98-bookworm（glibc 2.36）→ 运行镜像
# ubuntu:24.04（glibc 2.39），向前兼容，不会出现 "GLIBC_2.39 not found"。
#
# TLS 全部走 rustls（reqwest / tokio-tungstenite 均用 *-tls），
# 运行时【无需系统 OpenSSL】。

# ---------------- 构建阶段 ----------------
FROM rust:1.98-bookworm AS builder

WORKDIR /build

# 1) 只复制清单文件（依赖未变动时整层命中缓存）
COPY Cargo.toml Cargo.lock ./
COPY harness/Cargo.toml ./harness/
COPY rrserver/Cargo.toml ./rrserver/

# 2) 占位源码：让 cargo 能解析 workspace 并预编译依赖
RUN mkdir -p harness/src rrserver/src \
    && printf '' > harness/src/lib.rs \
    && printf 'fn main() {}\n' > harness/src/main.rs \
    && printf '' > rrserver/src/lib.rs \
    && printf 'fn main() {}\n' > rrserver/src/main.rs

# 3) 预编译依赖
RUN cargo build --release -p rrserver

# 4) 复制真实源码
COPY harness/src ./harness/src
COPY harness/resources ./harness/resources
COPY rrserver/src ./rrserver/src

# 5) 重新构建（依赖已缓存，只重编自身）
RUN touch rrserver/src/main.rs rrserver/src/lib.rs \
    && cargo build --release -p rrserver

# ---------------- 运行阶段 ----------------
FROM ubuntu:24.04

# rustls 不需要 OpenSSL；ca-certificates 用于对外 TLS 握手。
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 rrserver

WORKDIR /data

COPY --from=builder /build/target/release/rrserver /usr/local/bin/rrserver

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
