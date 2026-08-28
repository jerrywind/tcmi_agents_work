# 后端镜像：Python 3.11 + FastAPI
FROM python:3.11-slim

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    TCM_HOST=0.0.0.0 \
    TCM_PORT=8000 \
    TCM_STORE=memory

WORKDIR /app

# 先装依赖（利用层缓存）
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# 复制源码
COPY . .

# 上传目录（舌象/面相/患处照片）；生产建议挂载 volume 或接入对象存储
RUN mkdir -p uploads

EXPOSE 8000

# TCM_HOST / TCM_PORT 可由环境变量覆盖（见 docker-compose）
CMD ["sh", "-c", "uvicorn app.main:app --host ${TCM_HOST} --port ${TCM_PORT}"]
