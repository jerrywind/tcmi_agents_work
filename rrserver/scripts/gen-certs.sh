#!/usr/bin/env bash
# 生成本地自签名证书，便于 `docker compose up` 本地联调。
# 生产环境请改用 Let's Encrypt(certbot) 或真实证书，并放到 certs/ 下（fullchain.pem + privkey.pem）。
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p certs
openssl req -x509 -nodes -newkey rsa:2048 \
  -keyout certs/privkey.pem \
  -out certs/fullchain.pem \
  -days 365 \
  -subj "/CN=localhost"
echo "✅ 自签名证书已生成：certs/fullchain.pem + certs/privkey.pem"
echo "   现在可运行：docker compose up -d --build"
