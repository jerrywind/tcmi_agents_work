"""存储层测试：MemoryStore 与可选 RedisStore（使用内存桩避免真实 Redis 依赖）。"""
import sys
import types

import pytest

from app.store import MemoryStore, RedisStore, _build_store, store
from app.models.schemas import Consultation, Evidence, Patient, Report


class FakeRedis:
    """极简 Redis 桩，仅实现 RedisStore 用到的命令。"""

    def __init__(self):
        self.data: dict[str, str] = {}

    def set(self, name, value, ex=None, **kw):
        self.data[name] = value
        return True

    def get(self, name):
        return self.data.get(name)


@pytest.fixture
def fake_redis_module(monkeypatch):
    """把 sys.modules['redis'] 替换成桩，使 RedisStore 无需真实 redis 包。"""
    fake = types.ModuleType("redis")

    class _Redis:
        @staticmethod
        def from_url(*a, **k):
            return FakeRedis()

    fake.Redis = _Redis
    monkeypatch.setitem(sys.modules, "redis", fake)
    return fake


def _sample(cid="c1", gender="女"):
    c = Consultation(patient=Patient(gender=gender), complaint="测试主诉")
    c.id = cid
    return c


def test_memory_store_crud():
    s = MemoryStore()
    c = _sample("c1")
    s.save(c)
    assert s.get("c1").id == "c1"
    assert s.get("missing") is None
    c.status = "running"
    s.save(c)
    assert s.get("c1").status == "running"


def test_memory_store_same_object():
    s = MemoryStore()
    c = _sample("c2")
    s.save(c)
    c.evidences.append(Evidence(key="k", value="v", source="问", confidence=0.9))
    assert s.get("c2").evidences[0].value == "v"


def test_redis_store_roundtrip(fake_redis_module):
    import app.store as sm
    s = sm.RedisStore("redis://fake")

    c = _sample("r1", gender="男")
    c.status = "running"
    c.evidences.append(Evidence(key="k", value="v", source="问", confidence=0.9))
    s.save(c)

    got = s.get("r1")
    assert got is not None
    assert got.id == "r1"
    assert got.status == "running"
    assert got.evidences[0].value == "v"
    assert s.get("missing") is None


def test_default_store_is_memory():
    assert isinstance(store, MemoryStore)


def test_family_member_store():
    s = MemoryStore()
    f = __import__("app.models.schemas", fromlist=["Family"]).Family(
        name="家庭A", members=[__import__("app.models.schemas", fromlist=["Member"]).Member(name="本人")])
    s.save_family(f)
    assert s.get_family(f.id).name == "家庭A"
    assert s.list_families()[0].id == f.id

    from app.models.schemas import Member
    m = Member(name="父亲", relation="父亲", family_id=f.id)
    f.members.append(m)
    s.save_family(f)
    assert s.get_family(f.id).members[-1].name == "父亲"


def test_list_consultations_filter():
    s = MemoryStore()
    c = Consultation(patient=Patient(gender="女"), complaint="x", family_id="f1", member_id="m1")
    c.id = "cx"; c.ts = 1.0
    s.save(c)
    c2 = Consultation(patient=Patient(gender="男"), complaint="y", family_id="f1", member_id="m2")
    c2.id = "cy"; c2.ts = 2.0
    s.save(c2)
    assert len(s.list_consultations(family_id="f1")) == 2
    assert len(s.list_consultations(family_id="f1", member_id="m1")) == 1
    assert s.list_consultations(family_id="f1")[0].id == "cy"  # 按 ts 倒序


def test_build_store_redis_kind(fake_redis_module, monkeypatch):
    import app.store as sm
    monkeypatch.setattr(sm, "_STORE_KIND", "redis")
    monkeypatch.setenv("TCM_REDIS_URL", "redis://fake")
    # 用同模块引用比较，规避 pytest import mode 下 app.store 可能被加载两次导致的类身份不一致
    assert type(sm._build_store()).__name__ == "RedisStore"
    # 还原默认类型，避免影响后续测试
    monkeypatch.setattr(sm, "_STORE_KIND", "memory")
    assert type(sm._build_store()).__name__ == "MemoryStore"
