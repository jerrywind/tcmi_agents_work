"""会话存储：默认内存版；可切换 Redis（多 worker / 多实例共享）。

通过环境变量 TCM_STORE 选择：memory（默认）| redis。
Redis 模式需安装 redis 依赖并设置 TCM_REDIS_URL。接口（save/get）保持不变，
未来接入 PostgreSQL 也只需在此新增一个实现并在 _build_store 中分支。
"""
from __future__ import annotations

import os

from .models.schemas import Consultation, Family, Member

_STORE_KIND = os.environ.get("TCM_STORE", "memory").lower()


class MemoryStore:
    def __init__(self) -> None:
        self._data: dict[str, Consultation] = {}
        self._families: dict[str, Family] = {}

    def save(self, c: Consultation) -> None:
        self._data[c.id] = c

    def get(self, cid: str) -> Consultation | None:
        return self._data.get(cid)

    # ---------- 家庭 / 成员 ----------
    def save_family(self, f: Family) -> None:
        self._families[f.id] = f

    def get_family(self, fid: str) -> Family | None:
        return self._families.get(fid)

    def list_families(self) -> list[Family]:
        return list(self._families.values())

    def list_consultations(self, family_id: str = "", member_id: str = "") -> list[Consultation]:
        out = []
        for c in self._data.values():
            if family_id and c.family_id != family_id:
                continue
            if member_id and c.member_id != member_id:
                continue
            out.append(c)
        out.sort(key=lambda x: x.ts, reverse=True)
        return out


class RedisStore:
    """用 Redis 持久化 Consultation（JSON 序列化），支持多进程共享。"""

    def __init__(self, url: str, ttl: int = 60 * 60 * 24 * 7) -> None:
        import redis  # 延迟导入：memory 模式无需该依赖
        self._r = redis.Redis.from_url(url, decode_responses=True)
        self._prefix = "tcm:consult:"
        self._ttl = ttl

    def save(self, c: Consultation) -> None:
        self._r.set(self._prefix + c.id, c.model_dump_json(), ex=self._ttl)

    def get(self, cid: str) -> Consultation | None:
        raw = self._r.get(self._prefix + cid)
        return Consultation.model_validate_json(raw) if raw else None

    # ---------- 家庭 / 成员 ----------
    def _family_key(self, fid: str) -> str:
        return "tcm:family:" + fid

    def save_family(self, f: Family) -> None:
        self._r.set(self._family_key(f.id), f.model_dump_json(), ex=self._ttl)

    def get_family(self, fid: str) -> Family | None:
        raw = self._r.get(self._family_key(fid))
        return Family.model_validate_json(raw) if raw else None

    def list_families(self) -> list[Family]:
        out: list[Family] = []
        for k in self._r.keys(self._family_key("*")):
            raw = self._r.get(k)
            if raw:
                out.append(Family.model_validate_json(raw))
        return out

    def list_consultations(self, family_id: str = "", member_id: str = "") -> list[Consultation]:
        out: list[Consultation] = []
        for k in self._r.keys(self._prefix + "*"):
            raw = self._r.get(k)
            if not raw:
                continue
            c = Consultation.model_validate_json(raw)
            if family_id and c.family_id != family_id:
                continue
            if member_id and c.member_id != member_id:
                continue
            out.append(c)
        out.sort(key=lambda x: x.ts, reverse=True)
        return out


def _build_store():
    if _STORE_KIND == "redis":
        try:
            import redis  # noqa: F401
        except ImportError:
            raise RuntimeError("TCM_STORE=redis 但未安装 redis 依赖，请 pip install redis")
        return RedisStore(os.environ.get("TCM_REDIS_URL", "redis://localhost:6379/0"))
    return MemoryStore()


store = _build_store()
