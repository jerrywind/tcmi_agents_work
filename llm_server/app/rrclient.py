"""llm_server → rrserver 的注册与心跳客户端。

契约（与 `server/rrserver` 的 `/api/register`、`/api/heartbeat`、`/api/unregister` 对齐）：

1. **注册**：启动时用 `name + token` 主动 `POST /api/register`，
   云端为本次注册签发一个**独立 hash code** 并下发心跳周期（默认 30 分钟）；
   若配置了 `RR_SERVICE_ENDPOINT`（本服务可被 rrserver 直达的基址），则以
   `transport=http` 注册，云端探活时会来访问 `GET /rr/heartbeat`；
   否则按 `transport=ws` 注册（转发仍走 rrserver client 的反向隧道）。
2. **心跳**：后台任务每 `heartbeat_interval` 秒 `POST /api/heartbeat` 上报存活，
   证明「llm_server 的服务仍然活跃」。
3. **探活**：云端 40 分钟没收到心跳时会主动探活；1 分钟内无回应或回应异常，
   云端会记录日志并注销本条注册维护 —— 此时本客户端下一次心跳会拿到 404，
   随即自动重新注册（拿到新的 hash code）。
4. **下线**：关机时 `POST /api/unregister`，优雅关闭注册维护。

未配置 `RR_SERVER_BASE` 时整条链路不启用，不影响本地开发。
"""
from __future__ import annotations

import asyncio
import contextlib
import logging
import time
from dataclasses import dataclass

import httpx

logger = logging.getLogger("llm_server.rrclient")

#: 云端未下发心跳周期时的兜底值（30 分钟）
DEFAULT_HEARTBEAT_SECS = 1800


@dataclass
class RegistrationInfo:
    """当前注册状态（供 /healthz、/rr/heartbeat 展示与诊断）。"""

    name: str = ""
    hash_code: str = ""
    transport: str = ""
    heartbeat_interval: float = float(DEFAULT_HEARTBEAT_SECS)
    registered_at: float = 0.0
    last_heartbeat: float = 0.0
    failures: int = 0
    enabled: bool = False
    detail: str = "not registered"
    #: 最近一次心跳是否失败
    last_error: str = ""


class Registrar:
    """注册 + 心跳维护。生命周期由 Runtime（FastAPI lifespan）托管。"""

    def __init__(self, cfg, transport: "httpx.AsyncBaseTransport | None" = None) -> None:
        """`transport` 用于测试注入（如 `httpx.ASGITransport` 指向假的 rrserver）。"""
        self.cfg = cfg
        self._transport = transport
        self.info = RegistrationInfo(enabled=bool(cfg.rr_server_base))
        self._task: asyncio.Task | None = None
        self._stop = asyncio.Event()
        self._client: httpx.AsyncClient | None = None

    # ---------- 生命周期 ----------
    @property
    def enabled(self) -> bool:
        return bool(self.cfg.rr_server_base)

    async def start(self) -> None:
        if not self.enabled:
            logger.info("rrserver 注册未启用（未配置 RR_SERVER_BASE）")
            return
        self._stop = asyncio.Event()
        self._client = self._new_client()
        if not await self.register():
            # 注册失败不阻塞服务启动：后台任务会按退避间隔持续重试
            logger.warning("rrserver 注册失败，转入后台重试")
        self._task = asyncio.create_task(self._loop(), name="rr-heartbeat")

    def _new_client(self) -> httpx.AsyncClient:
        """统一的客户端构造（含测试注入的 transport）。"""
        return httpx.AsyncClient(timeout=self.cfg.rr_timeout, transport=self._transport)

    async def stop(self) -> None:
        self._stop.set()
        task, self._task = self._task, None
        if task is not None:
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await task
        if self.enabled and self.info.hash_code:
            await self.unregister()
        if self._client is not None:
            await self._client.aclose()
            self._client = None

    # ---------- 三个端点 ----------
    async def register(self) -> bool:
        """主动注册并换取独立 hash code。"""
        payload: dict = {
            "name": self.cfg.rr_service_name,
            "token": self.cfg.rr_service_token,
        }
        if self.cfg.rr_service_endpoint:
            payload["endpoint"] = self.cfg.rr_service_endpoint

        data, err = await self._post("/api/register", payload)
        if err or data is None:
            self.info.failures += 1
            self.info.last_error = err or "register failed"
            logger.warning("rrserver 注册失败: %s", self.info.last_error)
            return False

        # 注册响应必须给全 hash code 与心跳周期（毫秒），缺一即视为注册失败并重试
        hash_code = data.get("hash_code") or ""
        millis = data.get("heartbeat_interval_millis")
        if not hash_code or not isinstance(millis, (int, float)) or millis <= 0:
            self.info.failures += 1
            self.info.last_error = f"incomplete register response: {data}"
            logger.warning("rrserver 注册响应不完整: %s", data)
            return False
        interval = float(millis) / 1000.0
        now = time.time()
        self.info = RegistrationInfo(
            name=data.get("name") or self.cfg.rr_service_name,
            hash_code=hash_code,
            transport=data.get("transport") or "",
            heartbeat_interval=interval,
            registered_at=now,
            last_heartbeat=now,
            enabled=True,
            detail="registered",
        )
        logger.info(
            "已注册到 rrserver: name=%s hash=%s transport=%s 心跳周期=%.0fs",
            self.info.name,
            self.info.hash_code,
            self.info.transport,
            interval,
        )
        return bool(self.info.hash_code)

    async def heartbeat(self) -> bool:
        """按注册时拿到的 hash code 上报一次心跳。"""
        payload = {"name": self.cfg.rr_service_name, "hash": self.info.hash_code}
        data, err = await self._post("/api/heartbeat", payload, expect_404=True)
        if err == "not found":
            logger.warning("rrserver 上已无本服务注册（可能已被回收），将重新注册")
            self.info.hash_code = ""
            self.info.detail = "registration expired"
            self.info.last_error = "404 unknown registration"
            return False
        if err or data is None:
            self.info.failures += 1
            self.info.last_error = err or "heartbeat failed"
            logger.warning("rrserver 心跳失败: %s", self.info.last_error)
            return False

        self.info.last_heartbeat = time.time()
        self.info.detail = "ok"
        self.info.last_error = ""
        logger.info("rrserver 心跳成功（name=%s hash=%s）", self.info.name, self.info.hash_code)
        return True

    async def unregister(self) -> None:
        """优雅下线：关闭云端对本服务的注册维护。"""
        payload = {"name": self.cfg.rr_service_name, "hash": self.info.hash_code}
        _, err = await self._post("/api/unregister", payload, expect_404=True)
        if err:
            logger.warning("rrserver 注销失败: %s", err)
        else:
            logger.info("已从 rrserver 注销: name=%s hash=%s", self.info.name, self.info.hash_code)
        self.info.hash_code = ""
        self.info.detail = "unregistered"

    # ---------- 内部 ----------
    async def _post(self, path: str, payload: dict, expect_404: bool = False):
        """向 rrserver 发一个 JSON 请求；返回 (json, error)。

        error 为 None 表示成功；`expect_404=True` 时 404 以 "not found" 返回而非笼统错误。
        """
        url = f"{self.cfg.rr_server_base.rstrip('/')}{path}"
        client = self._client or self._new_client()
        try:
            resp = await client.post(url, json=payload)
        except Exception as e:  # 网络/超时：心跳链路本就允许失败重试
            return None, f"{type(e).__name__}: {e}"
        finally:
            if self._client is None:
                await client.aclose()

        if resp.status_code == 200:
            try:
                return resp.json(), None
            except ValueError:
                return {}, None
        if resp.status_code == 404:
            return None, "not found" if expect_404 else f"HTTP 404 {resp.text[:120]}"
        return None, f"HTTP {resp.status_code} {resp.text[:120]}"

    async def _loop(self) -> None:
        """后台循环：没注册成功就退避重试，成功后按周期心跳。

        - 注册失效（心跳 404）会**立即**重新注册，不等下一个周期；
        - 网络类错误按 `rr_retry_interval` 退避，避免刷爆日志。
        """
        while not self._stop.is_set():
            if not self.info.hash_code:
                ok = await self.register()
                await self._sleep(0.0 if ok else self.cfg.rr_retry_interval)
                continue

            ok = await self.heartbeat()
            if ok:
                await self._sleep(self.info.heartbeat_interval)
            else:
                # hash 被清空说明注册已被云端回收 → 立刻重新注册换取新 hash
                await self._sleep(0.0 if not self.info.hash_code else self.cfg.rr_retry_interval)

    async def _sleep(self, seconds: float) -> None:
        """可被 `stop()` 打断的等待；`seconds <= 0` 时立即返回。"""
        if seconds <= 0:
            return
        # 下限 50ms：既允许亚秒周期（测试），又避免异常配置导致的空转
        try:
            await asyncio.wait_for(self._stop.wait(), timeout=max(float(seconds), 0.05))
        except asyncio.TimeoutError:
            pass

    # ---------- 状态 ----------
    def status(self) -> dict:
        """注册状态快照（用于 /healthz 与 /rr/heartbeat）。"""
        now = time.time()
        return {
            "enabled": self.enabled,
            "registered": bool(self.info.hash_code),
            "name": self.info.name,
            "hash": self.info.hash_code,
            "transport": self.info.transport,
            # 与 rrserver 一致：周期用毫秒；已过去时长用秒便于阅读
            "heartbeat_interval_millis": round(self.info.heartbeat_interval * 1000),
            "registered_secs_ago": round(now - self.info.registered_at) if self.info.registered_at else None,
            "heartbeat_age_secs": round(now - self.info.last_heartbeat) if self.info.last_heartbeat else None,
            "failures": self.info.failures,
            "detail": self.info.detail,
            "last_error": self.info.last_error,
        }
