# harness 运行镜像（直接打包预编译的 Linux 二进制，不在容器内编译）。
#
# 【构建上下文】必须是 workspace 根（server/），而非本文件所在目录：
#     docker build -f harness/Dockerfile ..      # 或由 deploy/docker-compose.yml 编排
# 因为 harness 与 rrserver 同属一个 workspace，release 产物统一落在
# **server/target/release/**，子 crate 没有独立 target/。
# 上下文裁剪由 server/.dockerignore 负责（否则会传输整个 target/）。
#
# 【为什么要预编译】Docker 构建容器内网络会损坏 crates.io 下载（manifest 解析失败，
# exit 101），无法在容器内 `cargo build`。改为在 **WSL2** 的 Linux 环境预先编译：
#     wsl -e bash -c '. "$HOME/.cargo/env"; cd /mnt/d/labs/windblue_tech/tcm_work/server && cargo build --release'
# 或使用仓库脚本：pwsh scripts/build-release.ps1
# 生成的 Linux 二进制 server/target/release/harness 即被本 Dockerfile 使用。
#
# 运行时【无需系统 OpenSSL】（全部走 rustls），镜像内只需 ca-certificates 即可对外 TLS。
# 以非 root 用户（uid 10001）运行，降低被攻陷后的影响面。

# ubuntu:24.04 的 glibc 2.39 与 WSL2 编译环境一致，
# 避免 "GLIBC_2.39 not found" 的 ABI 不兼容（debian:bookworm 为 glibc 2.36）。
FROM ubuntu:24.04

# rustls 不需要 OpenSSL；ca-certificates 用于对外 TLS 握手。
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 harness

WORKDIR /data

# 预编译的 Linux release 二进制（路径相对于 workspace 根上下文）
COPY target/release/harness /usr/local/bin/harness
# 可改的 YAML 资源：打进镜像作为默认值，运行时用 bind mount 覆盖即可免重建调整
COPY harness/resources /data/resources

# 资源目录归属运行用户，保证可读（bind mount 场景由宿主机权限决定）
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
