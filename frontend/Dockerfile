# 前端镜像：Taro4 H5 多阶段构建，仅产出静态产物，【不含 nginx】。
# 静态产物由 deploy/ 的统一 nginx 容器 bind mount（../frontend/dist）托管并反代
# /api、/uploads（反代配置在 deploy/nginx/frontend.conf）。本 Dockerfile 仅作为
# 可复现的静态产物镜像，最终阶段用 scratch，镜像内不运行任何 web 服务器。
FROM node:18-alpine AS builder
WORKDIR /app
COPY package.json ./
RUN npm install
COPY . .
# 构建 H5 静态产物到 dist/（apiBase 走相对路径 /api，由 deploy 的 nginx 反代到后端）
RUN npm run build:h5

# 产物镜像：纯静态文件，不含 nginx（反代/托管统一在 deploy/）。
FROM scratch
COPY --from=builder /app/dist /dist

