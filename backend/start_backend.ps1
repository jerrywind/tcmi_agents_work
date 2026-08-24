$env:TCM_LLM_API_KEY = "sk-lm-8V8Kbso6:v7Mo908k7l6m5olqosJA"
$env:TCM_LLM_BASE_URL = "http://localhost:11223/v1"
$env:TCM_LLM_TEXT_MODEL = "google/gemma-4-12b-qat"
$env:TCM_LLM_VISION_MODEL = "google/gemma-4-12b-qat"
$env:TCM_LLM_API = "responses"
$env:TCM_ROUTING_FILE = "app/routing.llm.yaml"

Set-Location "d:/labs/windblue_tech/tcm_work/backend"
Start-Process -FilePath "python" -ArgumentList "-m","uvicorn","app.main:app","--host","0.0.0.0","--port","8000" -RedirectStandardOutput "run.log" -RedirectStandardError "run.err" -NoNewWindow
Write-Host "backend started with LM Studio Responses API"
