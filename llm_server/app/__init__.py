"""llm_server —— LM Studio 网关 + Agent 中间层。

不再托管/内置模型推理：所有模型推理由 LM Studio（默认 http://localhost:11223/v1）
提供。本服务在此之上实现：
  - prompt 优化（上下文压缩/精简/预算裁剪）
  - tool calling（工具注册表 + 执行循环）
  - MCP（连接外部 MCP Server 并把其工具纳入 agent）
  - agent（多步工具调用循环）
并对 backend 保持 OpenAI 兼容（/v1/chat/completions、/v1/responses、
/v1/embeddings），backend 无需改动即可接入。
"""

__version__ = "2.0.0"
