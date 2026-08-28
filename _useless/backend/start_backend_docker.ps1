$ErrorActionPreference = "Stop"
Set-Location "d:/labs/windblue_tech/tcm_work/backend"

# backend 容器通过 host.docker.internal 访问宿主机上的 LM Studio (localhost:11223)
$env:TCM_LLM_BASE_URL = "http://host.docker.internal:11223/v1"
$env:TCM_LLM_API_KEY  = "sk-lm-8V8Kbso6:v7Mo908k7l6m5olqosJA"
$env:TCM_LLM_API      = "responses"
$env:TCM_LLM_TEXT_MODEL  = "google/gemma-4-12b-qat"
$env:TCM_LLM_VISION_MODEL = "google/gemma-4-12b-qat"
$env:TCM_ROUTING_FILE = "/app/routing.llm.yaml"
$env:TCM_CORS_ORIGINS = "*"

docker compose up -d --build
Write-Host "backend docker started"
