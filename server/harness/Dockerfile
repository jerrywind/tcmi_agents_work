# harness 运行镜像（直接打包预编译的 Linux 二进制）。
#
# 说明：Docker 构建容器内网络会损坏 crates.io 下载（manifest 解析失败，exit 101），
# 无法在容器内 `cargo build`。改为在 **WSL2** 的 Linux 环境预先编译：
#     wsl -e bash -c '. "$HOME/.cargo/env"; cd /mnt/d/labs/windblue_tech/tcm_work/server/harness; cargo build --release'
# 生成的 Linux 二进制 server/harness/target/release/harness 即被本 Dockerfile 使用。
#
# 运行时【无需系统 OpenSSL】（rustls）；镜像内仅安装 ca-certificates 即可对外建立 TLS 连接。
# 运行时以非 root 用户（uid 10001）运行。

FROM ubuntu:24.04

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 harness

WORKDIR /data
# 直接使用 WSL2 预编译的 Linux release 二进制（避免容器内编译的网络问题）。
COPY target/release/harness /usr/local/bin/harness
# 一并打包可改的 YAML 资源（流程与数据分离，便于在容器内挂载/修改）。
COPY resources /data/resources

USER harness

# harness 在容器内应监听 0.0.0.0，才能被同网络下的 nginx 访问（默认 127.0.0.1 仅本容器回环）。
EXPOSE 8011
ENTRYPOINT ["harness"]
# 默认监听 0.0.0.0:8011 并加载同目录 resources/。
CMD ["--listen", "0.0.0.0:8011", "--resources", "/data/resources"]
