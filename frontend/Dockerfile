# 前端镜像：Taro4 H5 多阶段构建 + Nginx 托管
FROM node:18-alpine AS builder
WORKDIR /app
COPY package.json ./
RUN npm install
COPY . .
# 构建 H5 静态产物到 dist/（apiBase 走相对路径 /api，由 Nginx 反代到后端）
RUN npm run build:h5

FROM nginx:alpine
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
