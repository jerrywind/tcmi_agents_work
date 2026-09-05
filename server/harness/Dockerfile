# harness 运行镜像 —— 多阶段构建，**在 Docker 内完成编译**。
#
# 【构建上下文】必须是 workspace 根（server/），而非本文件所在目录：
#     cd server && docker build -f harness/Dockerfile -t tcm-harness:local .
# 因为 harness 与 rrserver 同属一个 workspace，且 harness 依赖 rrserver 的 lib，
# 两者源码都必须进入上下文。
#
# 【为什么在 Docker 内编译】
# 后端**完全依赖 Docker** 构建、运行与验证，不使用宿主机/WSL2 的本地产物。
# 此前本文件是 COPY 预编译二进制（需先在 WSL2 里 cargo build），
# 那既违背「一切走 Docker」，也让 CI 无法独立复现产物。
#
# 【glibc 兼容性】构建镜像用 rust:1.98-bookworm（glibc 2.36），
# 运行镜像用 ubuntu:24.04（glibc 2.39）。低版本 glibc 编译出的二进制
# 可在高版本上运行（向前兼容）；反之才会报 "GLIBC_2.39 not found"。
#
# 【层缓存策略】先只复制清单 + 占位源码预编译依赖，再复制真实源码。
# 依赖不变时，耗时的第三方 crate 编译结果可被 Docker 层缓存复用。

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

# 3) 预编译全部依赖（本层是耗时大头，但可被缓存）
RUN cargo build --release -p harness

# 4) 复制真实源码与资源
COPY harness/src ./harness/src
COPY harness/resources ./harness/resources
COPY harness/cases.jsonl ./harness/cases.jsonl
# 黄金病例集（H7）：tests/golden.rs 的输入，CI 在镜像内跑测试时需要
COPY harness/golden_cases.jsonl ./harness/golden_cases.jsonl
COPY harness/tests ./harness/tests
COPY rrserver/src ./rrserver/src

# 5) 重新构建：依赖已缓存，这里只重编 harness / rrserver 自身
#    touch 是为了让 mtime 一定新于上一步产物，确保触发重编
RUN touch harness/src/main.rs harness/src/lib.rs rrserver/src/lib.rs \
    && cargo build --release -p harness

# ---------------- 运行阶段 ----------------
FROM ubuntu:24.04

# rustls 不需要 OpenSSL；ca-certificates 用于对外 TLS 握手。
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 harness

WORKDIR /data

COPY --from=builder /build/target/release/harness /usr/local/bin/harness
# 可改的 YAML 资源：打进镜像作为默认值，运行时用 bind mount 覆盖即可免重建调整
COPY harness/resources /data/resources

RUN chown -R harness:harness /data

USER harness

# 容器内必须监听 0.0.0.0，否则同网络下的 nginx 无法连通（127.0.0.1 仅容器回环）
EXPOSE 8011

# 健康检查：镜像保持精简（不装 curl），用 bash 的 /dev/tcp 做端口探测。
# harness 的 /health 在监听后即返回 ok，端口可达即视为就绪。
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/8011' || exit 1

ENTRYPOINT ["harness"]
# 默认监听 0.0.0.0:8011 并加载镜像内的 /data/resources
CMD ["--listen", "0.0.0.0:8011", "--resources", "/data/resources"]
