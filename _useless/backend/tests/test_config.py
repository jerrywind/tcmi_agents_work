"""配置层测试：默认值与环境变量覆盖。"""
import pytest

from app.config import Settings, settings

ENV_KEYS = [
    "TCM_LLM_BASE_URL", "TCM_LLM_PROVIDER", "TCM_LLM_TEXT_MODEL",
    "TCM_LLM_VISION_MODEL", "TCM_HOST", "TCM_PORT", "TCM_CORS_ORIGINS",
    "TCM_LLM_API_KEY",
]


@pytest.fixture
def clean_env(monkeypatch):
    for k in ENV_KEYS:
        monkeypatch.delenv(k, raising=False)
    return monkeypatch


def test_default_settings(clean_env):
    s = Settings()
    assert s.loop["max_rounds"] == 8
    assert s.route_of("treatment.plan")["impl"] == "rule"
    assert isinstance(s.resolve_model("text-default"), str) and s.resolve_model("text-default")
    assert isinstance(s.llm_api_key, str)
    assert isinstance(s.host, str)
    assert isinstance(s.port, int)
    assert isinstance(s.cors_origins, list)


def test_env_overrides(monkeypatch):
    monkeypatch.setenv("TCM_LLM_BASE_URL", "http://llm:1234/v1")
    monkeypatch.setenv("TCM_LLM_PROVIDER", "openai")
    monkeypatch.setenv("TCM_LLM_TEXT_MODEL", "gpt-4o")
    monkeypatch.setenv("TCM_LLM_VISION_MODEL", "gpt-4o-vision")
    monkeypatch.setenv("TCM_HOST", "127.0.0.1")
    monkeypatch.setenv("TCM_PORT", "9000")
    monkeypatch.setenv("TCM_CORS_ORIGINS", "https://a.com,https://b.com")

    s = Settings()
    assert s.llm["base_url"] == "http://llm:1234/v1"
    assert s.llm["provider"] == "openai"
    assert s.llm["models"]["text-default"] == "gpt-4o"
    assert s.llm["models"]["vision-default"] == "gpt-4o-vision"
    assert s.host == "127.0.0.1"
    assert s.port == 9000
    assert s.cors_origins == ["https://a.com", "https://b.com"]


def test_resolve_model_fallback():
    s = Settings()
    # 未在配置中登记的模型名原样透传（交由具体 provider 决定）
    assert s.resolve_model("custom-model") == "custom-model"


def test_route_of_defaults(clean_env):
    s = Settings()
    r = s.route_of("diagnosis.inspection")
    assert r["impl"] == "rule"
    assert r["model"] == "vision-default"
    # 未知能力回退到 rule
    assert s.route_of("nope.cap")["impl"] == "rule"


def test_singleton_is_settings():
    assert isinstance(settings, Settings)
