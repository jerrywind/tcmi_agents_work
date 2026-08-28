"""内置工具：开箱即用的基础能力，供 agent 循环调用。

生产接入真实业务工具（TCM 知识库、预约、档案等）时，在
`app/runtime.py` 里额外 register 即可，或通过 MCP 接入外部工具。
"""
from __future__ import annotations

import ast
import datetime
import json
import operator
import random
from typing import Any

from .registry import Tool, ToolRegistry

_BINOPS = {
    ast.Add: operator.add, ast.Sub: operator.sub,
    ast.Mult: operator.mul, ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv, ast.Mod: operator.mod,
    ast.Pow: operator.pow,
}
_MAX_EXPR_CHARS = 200


def _safe_eval(expr: str) -> Any:
    """受限表达式求值（仅四则/乘方/括号/数字）。"""
    if len(expr) > _MAX_EXPR_CHARS:
        raise ValueError("表达式过长")
    tree = ast.parse(expr, mode="eval")

    def _eval(node: ast.AST) -> Any:
        if isinstance(node, ast.Constant):
            if isinstance(node.value, (int, float)):
                return node.value
            raise ValueError("仅支持数值")
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, (ast.UAdd, ast.USub)):
            v = _eval(node.operand)
            return v if isinstance(node.op, ast.UAdd) else -v
        if isinstance(node, ast.BinOp) and type(node.op) in _BINOPS:
            return _BINOPS[type(node.op)](_eval(node.left), _eval(node.right))
        raise ValueError("仅支持四则运算/乘方")

    return _eval(tree.body)


async def _get_current_time(tz: str = "Asia/Shanghai") -> str:
    try:
        import zoneinfo
        tzinfo = zoneinfo.ZoneInfo(tz)
    except Exception:  # noqa: BLE001  缺少时区库则用本地时间
        tzinfo = None
    now = datetime.datetime.now(tzinfo)
    return (f"当前时间：{now:%Y-%m-%d %H:%M:%S}（{tz}）。"
            f"星期{'一二三四五六日'[now.weekday()]}。"
            f"农历信息不可用，请勿臆测。")


async def _calculate(expression: str) -> str:
    try:
        value = _safe_eval(expression)
        return f"计算 {expression} = {value}"
    except Exception as e:  # noqa: BLE001
        return f"ERROR: 无法计算「{expression}」: {e}"


async def _roll_dice(sides: int = 6) -> str:
    sides = int(sides)
    if sides < 2 or sides > 1000:
        return "ERROR: 骰子面数需在 2~1000 之间"
    return f"掷骰结果：{random.randint(1, sides)} / {sides}"


async def _echo(text: str) -> str:
    return text


def register_builtin_tools(registry: ToolRegistry) -> None:
    """注册内置工具到 registry。"""
    registry.register(Tool(
        name="get_current_time",
        description="获取当前日期与时间（按指定时区）。agent 需要知道时间/日期时使用。",
        parameters={
            "type": "object",
            "properties": {
                "tz": {"type": "string", "description": "IANA 时区，默认 Asia/Shanghai", "default": "Asia/Shanghai"},
            },
            "additionalProperties": False,
        },
        handler=_get_current_time,
        source="builtin",
    ))
    registry.register(Tool(
        name="calculate",
        description="执行数学计算（仅支持四则运算、乘方与括号，数值输入）。",
        parameters={
            "type": "object",
            "properties": {
                "expression": {"type": "string", "description": "要计算的数学表达式，如 (12+34)*5"},
            },
            "required": ["expression"],
            "additionalProperties": False,
        },
        handler=_calculate,
        source="builtin",
    ))
    registry.register(Tool(
        name="roll_dice",
        description="掷一个骰子，返回 1~sides 之间的随机整数。",
        parameters={
            "type": "object",
            "properties": {"sides": {"type": "integer", "description": "骰子面数，默认 6", "default": 6}},
            "additionalProperties": False,
        },
        handler=_roll_dice,
        source="builtin",
    ))
    registry.register(Tool(
        name="echo",
        description="原样返回输入文本，用于测试工具调用链路是否打通。",
        parameters={
            "type": "object",
            "properties": {"text": {"type": "string", "description": "要回显的文本"}},
            "required": ["text"],
            "additionalProperties": False,
        },
        handler=_echo,
        source="builtin",
    ))
    # 汇总工具清单（可选）：供 agent 在不确定可用工具时查询
    registry.register(Tool(
        name="list_available_tools",
        description="列出当前可用的全部工具及其说明，agent 不确定有哪些工具时使用。",
        parameters={
            "type": "object",
            "properties": {},
            "additionalProperties": False,
        },
        handler=lambda: json.dumps([
            {"name": t.name, "description": t.description, "source": t.source}
            for t in registry.list()
        ], ensure_ascii=False, indent=2),
        source="builtin",
    ))
