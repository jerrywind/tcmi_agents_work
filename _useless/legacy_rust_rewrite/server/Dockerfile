# 多阶段构建：在 glibc 2.39 环境（WSL2 Ubuntu 24.04）预编译，再 COPY 二进制到运行镜像。
# 注意：容器内 cargo build 会因网络损坏 crates.io 下载（exit 101），务必在宿主机 WSL2 构建。
FROM rust:1.92-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release

FROM ubuntu:24.04
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /src/target/release/server /usr/local/bin/server
# 运行时配置由 deploy 挂载到 /etc/server.toml
EXPOSE 8080
ENTRYPOINT ["server", "--listen", "0.0.0.0:8080", "--config", "/etc/server.toml"]
