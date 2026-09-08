# syntax=docker/dockerfile:1.6
# 统一 Rust 后端镜像 `tcmi_server` —— harness（诊断后端）+ rrserver（云端中继）
# **在同一个镜像/容器里**。
#
# 构建（上下文 = server/，即 Cargo workspace 根）：
#     cd server && docker build -t tcmi_server:local .
#
# 【端口约定】
#   harness  43301   ← 对外通信（nginx /api、契约测试、curl 直连）
#   rrserver 43302   ← 中继通信（nginx /rr 反代目标；rrserver 由 nginx 对外，不必直连宿主机）
#
# 【为什么合并】
# 此前 harness 与 rrserver 是两个镜像、两个容器，但两者总是一起部署：
# nginx 把 /api 反代到 harness、把 /rr 反代到 rrserver。
# 合并后：一次编译出两个二进制 → 一个镜像 → 一个容器同时监听 43301 与 43302；
# 少一层镜像基础层（约省 130MB）与一个容器进程。
# 只跑其中一个（或 rrserver 其他子命令）时传参即可：
#     docker run tcmi_server:local rrserver client --server ... --name home --token ...
#     docker run tcmi_server:local harness --help
#
# 【镜像瘦身】三层手段：
#   1) 多阶段构建：运行镜像只带两个二进制 + 必要资源/配置，天然不含 target/。
#   2) BuildKit cache mount：cargo registry 与编译中间产物（/build/target）挂外部缓存，
#      **不落进任何镜像层**，改源码不会把 GB 级增量产物打进新层，且跨构建复用。
#      （需 BuildKit：Docker >= 23 默认已启用，或用 DOCKER_BUILDKIT=1。）
#   3) release 档 strip + thin-LTO（server/Cargo.toml 的 [profile.release]）。
#
# 【glibc 兼容性】构建镜像 rust:1.98-bookworm（glibc 2.36）→ 运行镜像
# ubuntu:24.04（glibc 2.39），向前兼容，不会出现 "GLIBC_2.39 not found"。
#
# 【缓存策略】先只复制清单 + 占位源码预编译依赖，再复制真实源码。
# 依赖不变时，耗时的第三方 crate 编译结果缓存在 BuildKit cache mount 中复用。

# ---------------- 构建阶段 ----------------
FROM rust:1.98-bookworm AS builder

WORKDIR /build

# 1) 只复制清单文件（依赖未变动时，下面整层都能命中缓存）
COPY Cargo.toml Cargo.lock ./
COPY harness/Cargo.toml ./harness/
COPY rrserver/Cargo.toml ./rrserver/

# 2) 占位源码：让 cargo 能解析 workspace 并编译依赖。
#    harness 依赖 rrserver 的 lib，故 rrserver 的 src 也必须有占位。
RUN mkdir -p harness/src rrserver/src \
    && printf '' > harness/src/lib.rs \
    && printf 'fn main() {}\n' > harness/src/main.rs \
    && printf '' > rrserver/src/lib.rs \
    && printf 'fn main() {}\n' > rrserver/src/main.rs

# 3) 预编译依赖：一次性编译出 harness 与 rrserver 两个二进制所需的全部依赖
#    （cache mount：产物只存外部缓存、不进镜像层）
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=rust-workspace-target,sharing=locked,target=/build/target \
    cargo build --release -p harness -p rrserver

# 4) 复制真实源码与资源
COPY harness/src ./harness/src
COPY harness/resources ./harness/resources
COPY harness/cases.jsonl ./harness/cases.jsonl
# 黄金病例集（H7）：tests/golden.rs 的输入，CI 在镜像内跑测试时需要
COPY harness/golden_cases.jsonl ./harness/golden_cases.jsonl
COPY harness/tests ./harness/tests
COPY rrserver/src ./rrserver/src
# rrserver 的集成测试也要进上下文：rrserver/Cargo.toml 里 [[test]] 指向
# tests/integration.rs，缺了它 `cargo test -p rrserver` 在镜像内会直接报
# "can't find integration-test"。

# 5) 用真实源码重新构建（依赖已在缓存里，只重编自身），再把两个二进制拷到 /out
#    （cache mount 里的内容不能被后续 COPY --from 读取）。
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=rust-workspace-target,sharing=locked,target=/build/target \
    touch harness/src/main.rs harness/src/lib.rs \
          rrserver/src/lib.rs rrserver/src/main.rs \
    && cargo build --release -p harness -p rrserver \
    && mkdir -p /out \
    && cp /build/target/release/harness /build/target/release/rrserver /out/

# ---------------- 运行阶段 ----------------
FROM ubuntu:24.04

# rustls 不需要 OpenSSL；ca-certificates 用于对外 TLS 握手。
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 harness

WORKDIR /data

# 两个二进制同镜像：默认由 entrypoint 一起拉起
COPY --from=builder /out/harness /usr/local/bin/harness
COPY --from=builder /out/rrserver /usr/local/bin/rrserver

# harness 可改的 YAML 资源：打进镜像作为默认值，运行时用 bind mount 覆盖即可免重建调整
COPY harness/resources /data/resources

# rrserver 默认配置：使镜像不挂载配置文件也能启动（便于 `docker run` 快速试用）。
# 安全处理：把模板里的示例 token 清空——未配置 token 的隧道无法被注册，
# 避免沿用 "change-me-strong-token" 这类弱口令。
# **生产必须挂载自己的配置**：../server/rrserver/config/rrserver.toml:/etc/rrserver.toml:ro
COPY rrserver/config/rrserver.toml.example /etc/rrserver.toml
RUN sed -i 's/^token = .*/token = ""/' /etc/rrserver.toml

# 统一入口：无参数 → 同时拉起 harness(43301) + rrserver server(43302)；
# 有参数 → 直接按参数执行（rrserver client / llm-server / harness --help 等）。
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh \
    # Windows 检出可能带 CRLF，去掉行尾 \r，否则 bash 认不出解释器
    && sed -i 's/\r$//' /usr/local/bin/entrypoint.sh \
    && chown -R harness:harness /data

USER harness

# 容器内必须监听 0.0.0.0，否则同网络下的 nginx 无法连通（127.0.0.1 仅容器回环）
EXPOSE 43301 43302

# 健康检查：默认模式两个端口都要通。
# 只跑 rrserver client / llm-server 子命令时用 `docker run --no-healthcheck` 覆盖。
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/43301 && exec 4<>/dev/tcp/127.0.0.1/43302' || exit 1

ENTRYPOINT ["entrypoint.sh"]
# 空 CMD：默认走 entrypoint 的「同时拉起两个进程」分支
CMD []
