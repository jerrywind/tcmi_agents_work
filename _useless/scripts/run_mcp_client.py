"""演示本系统作为 MCP Client 连接外部 MCP Server。

把外部 MCP Server 暴露的工具注册进本系统的 skill 调用体系，使 LLM Agent 在
望闻问切/辨证/treatment 推理时即可通过 function calling 调用外部能力。

用法：
    # 连接一个 stdio 型外部 MCP（本地脚本/可执行）
    python scripts/run_mcp_client.py --stdio weather --cmd python --args weather_mcp.py

    # 连接一个 Streamable HTTP 型外部 MCP
    python scripts/run_mcp_client.py --http calendar --url http://localhost:9000/mcp

    # 连接后调用一次远端工具，验证连通性
    python scripts/run_mcp_client.py --http tcm --url http://localhost:8001/mcp \
        --call list_agent_capabilities

连接成功后会打印注册进本系统的工具名（形如 mcp__<server>__<tool>），可在
问诊编排中通过 run_tool_loop 被 LLM 调用。

提示：日常使用更推荐把连接写进 `backend/app/routing.yaml` 的 `mcp.clients`
（随后端启动自动连接），或调用后端的 `POST /api/mcp/clients` 运行时接入。
本脚本主要用于**离线联调**外部 MCP Server。
"""
from __future__ import annotations

import argparse
import asyncio
import json
import sys
from pathlib import Path

# 让 backend 包可被导入（脚本位于项目根的 scripts/ 目录）
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "backend"))

from app.mcp.client import MCPToolHub


async def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--stdio", metavar="NAME", help="stdio 型外部 MCP 名称")
    ap.add_argument("--cmd", help="stdio 启动命令，如 python")
    ap.add_argument("--args", nargs="*", default=[], help="stdio 启动参数")
    ap.add_argument("--http", metavar="NAME", help="http 型外部 MCP 名称")
    ap.add_argument("--url", help="http 型外部 MCP 的 URL")
    ap.add_argument("--call", metavar="TOOL", help="连接后调用一次该远端工具")
    ap.add_argument("--args-json", default="{}", help="--call 的参数（JSON 对象）")
    args = ap.parse_args()

    hub = MCPToolHub()
    if args.stdio:
        name = args.stdio
        tools = await hub.connect(name, "stdio", command=args.cmd or "python",
                                  args=list(args.args))
        print(f"[client] connected stdio '{name}', tools: {tools}")
    elif args.http:
        name = args.http
        tools = await hub.connect(name, "http",
                                  url=args.url or "http://127.0.0.1:9000/mcp")
        print(f"[client] connected http '{name}', tools: {tools}")
    else:
        print("请指定 --stdio/--cmd 或 --http/--url")
        return

    print(f"[client] 已连接服务器: {hub.connected_servers}")
    try:
        if args.call:
            result = await hub.call(name, args.call, json.loads(args.args_json))
            print(f"[client] {args.call} ->")
            print(json.dumps(result, ensure_ascii=False, indent=2, default=str))
            return
        print("[client] 外部 MCP 工具已注册进本系统 skill 体系，LLM 可在问诊中调用。")
        print("[client] 按 Ctrl+C 退出。")
        while True:
            await asyncio.sleep(3600)
    except (KeyboardInterrupt, asyncio.CancelledError):
        pass
    finally:
        await hub.close()


if __name__ == "__main__":
    asyncio.run(main())
