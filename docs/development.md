# 开发文档（Development）

面向开发者：环境、目录结构、本地启动、调试与常见问题。
**部署/端口/配置见 [`deployment.md`](./deployment.md)；模型与降级事实见
[`llm_server.md`](./llm_server.md)；测试体系见 [`testing.md`](./testing.md)。
本文件只描述开发流程，不重复这些事实。**

---

## 1. 环境要求

| 组件 | 要求 | 必需性 |
|---|---|---|
| Docker | 任意近期版本 | **必需**（后端构建、运行、验证全在容器内） |
| Node.js 18+ | 前端 | 必需（改前端时） |
| Python 3.11+ | `llm_server` 网关与 RAG | 可选（直连 LM Studio 则不需要） |
| LM Studio | 宿主机 `:11223` | 真实推理时需要 |
| Rust 工具链 | — | **不需要**（镜像内编译） |

---

## 2. 仓库结构

```
tcm_work/
├── frontend/          Taro 多端（H5/微信小程序），只产出静态 dist
│   └── src/
│       ├── pages/     index（建档）/ consult（问诊）/ report（报告）
│       │              reports（存证记录）/ skills（技能）/ family（家庭档案）
│       ├── services/  harness.ts（契约客户端）+ session.ts（前端多轮状态）
│       └── utils/
├── server/            Rust workspace
│   ├── harness/       诊断编排
│   │   ├── src/       agents / orchestrator / knowledge / rag_health / skills
│   │   │              mcp / http / resources / store / trace
│   │   ├── resources/ 可改 YAML 数据
│   │   ├── tests/     cases.rs（案例回归）/ behavior.rs（行为）/ llm_eval.rs（LLM 评分）
│   │   └── cases.jsonl  病例基准（合成：5 种主诉 / 3 种证候组合，资源完整性护栏）
│   └── rrserver/      反向隧道：server + client + llmsrv
├── llm_server/        纯 LM Studio 网关（Python，可选）；rag/ 为其检索子组件
├── deploy/            nginx 配置 + certs + docker-compose
├── docs/              文档（索引见 docs/README.md）；samples/ 为真实 LLM 验收样例
├── scripts/           build-release.ps1（出镜像）/ cleanup.ps1（清理）
├── e2e_tests/         全链路 E2E + 人工验收脚本
└── rag_data/          中医典籍语料（700 部，**不入库**，见 docs/rag.md）
```

产物目录（均已被 `.gitignore` 覆盖）：`server/target/`、`frontend/dist/`、
`__pycache__/`、`e2e_tests/images/`、`e2e_tests/_reports/`、
`e2e_tests/_*.log`、`e2e_tests/_resp.json`、`e2e_tests/_mr_*`（验收与多轮验证的运行期产物）。

> `server/target/` 会涨到数 GB（容器内编译经挂载卷落回宿主机），
> 后端一律 Docker 验证，可安全删除。

---

## 3. 后端开发（harness）

> **铁律：后端完全依赖 Docker**，不使用宿主机 `cargo build` 产物。

```powershell
cd server
docker build -f harness/Dockerfile -t tcm-harness:local .   # 多阶段，镜像内编译
docker run -d --name tcm-harness-8011 -p 8011:8011 tcm-harness:local
```

- 想改 YAML 而不重建镜像：`-v "$PWD/harness/resources:/data/resources:ro"`，
  再 `POST /reload`（需 `hot_reload: true`）。
- 改 Rust 代码 → 重新 `docker build`。测试与 lint 也在容器内跑（见 `testing.md`）。
- 无 LLM 时：`/chat` 会失败（harness 无 MockProvider），只读端点可用。

---

## 4. 接入真实 LLM

1. LM Studio 加载 `google/gemma-4-12b-qat`，开启 Local Server（`:11223`）。
2. （可选）起 `llm_server` 网关：`cd llm_server && python -m app.main`（`:8000`）。
3. harness 指向 LLM（前缀是 **`HARNESS_`**，不是 `TCM_LLM_*`）：

```powershell
$env:HARNESS_LLM_BASE_URL = "http://host.docker.internal:11223/v1"  # 容器内访问宿主机
$env:HARNESS_LLM_API_KEY  = "<LM Studio 开启校验时必填>"
docker run -d --name tcm-harness-8011 -p 8011:8011 `
  -e HARNESS_LLM_BASE_URL -e HARNESS_LLM_API_KEY -e HARNESS_MODEL `
  tcm-harness:local
```

完整配置项见 `deployment.md` 3.2 与 `resources/config.yaml`。

---

## 5. rrserver（可选）

```powershell
cd server
docker build -f rrserver/Dockerfile -t tcm-rrserver:local .
```

启动与隧道配置见 `deployment.md` 第 5 节；本地一键也可用
`server/rrserver/start_rrserver.ps1`（起 server `:8088` + client `:9000`）。
调试探测：`curl https://rr.windblue.tech/healthz`（应返回 `ok`）。

---

## 6. 前端开发

```bash
cd frontend
npm install
npm run dev:h5        # H5：http://localhost:10086
npm run dev:weapp     # 微信小程序（需微信开发者工具）
```

- 契约客户端 `src/services/harness.ts`；多轮 `messages` 由 `src/services/session.ts`
  在前端维护（harness 无服务端会话）。
- 跨端差异由 Taro 适配：H5 走 devServer 代理（`/api` → harness:8011），小程序直连后端地址。
- 类型检查：`npx tsc --noEmit`。
- ⚠️ **不要用 Taro 原生 `<Picker mode='date'>` 做出生日期**：Taro 4 在 H5 端把它实现成
  Stencil 自定义元素 `<taro-picker-core>` 的 **transform 轮盘**，在手机浏览器上有两个修不掉的问题——
  ① 轮盘的 `touchmove` 冒泡到 `document`，导致背景整页跟着上移/下拉；
  ② transform 轮盘在滚动/重绘时留下空白项，年/月/日（尤其月、日）显示不全。
  应用层抓不到它的内部事件、改不了它的渲染，只能换掉。出生日期已改用自建三列滚动选择器
  `src/components/BirthDatePicker.tsx`（年/月/日各一列原生 `ScrollView`，`overscroll-behavior: contain`
  锁住背景滚动、自己渲染每一项无空白），日期换算纯逻辑在 `src/utils/birthdate.ts`（有单测）。
  同样原理：`<Input>`/`<Textarea>` 在 H5 渲染成 `<taro-input-core>`/`<taro-textarea-core>`，
  Playwright 必须用 shadow-piercing（`taro-input-core >>> input`）选中，详见 `testing.md`。

  **自建选择器踩过的两个 Taro H5 坑（已修，别再踩）**：
  - **组件 `.scss` 样式隔离**：Taro 4 默认给组件级 `className` 加 hash，组件内裸类名匹配不上、sheet 会坍缩。
    选择器的样式放在全局 `app.scss`（已在 `BirthDatePicker.tsx` 注释说明）。
  - **`pxtransform` 缩放**：`config/index.ts` 的 `designWidth: 750` 会把 scss 里的 `px` 转成响应式单位，
    在 390 屏上约缩到 0.4 倍，轮盘被压扁、初始 `scrollTop`（未缩放像素）越界错乱。
    **轮盘的精确尺寸（body 高、项高、指示线位置、列宽）一律用组件内联 `style`**（不经 pxtransform，是精确像素）。
    另外 `<ScrollView>` 渲染成 `<taro-scroll-view-core>`，host 不响应 `flex:1`，要**包一层 `<View>`** 来撑 flex 尺寸。

---

## 7. 测试

全部命令见 [`testing.md`](./testing.md)，要点：

- 后端：`docker run ... cargo test --workspace`（**不要在本地跑 cargo**）。
- 前端：`npm run test`。
- RAG：`python -m unittest test_corpus`。
- 全链路 E2E：`e2e_tests/run_full_chain_e2e.ps1`；人工验收 `run_manual_e2e.ps1`。

---

## 8. 常见问题（FAQ）

| 现象 | 排查 |
|---|---|
| `/chat` 报 LLM 不可用 | 未设 `HARNESS_LLM_BASE_URL` 或 LM Studio 未启动。容器内要用 `host.docker.internal` 而非 `localhost`。 |
| 设了 `TCM_LLM_BASE_URL` 不生效 | 前缀错误：harness 只认 **`HARNESS_`**。 |
| `GET /reports` 返回 `enabled: false` | 报告持久化默认关闭，需配 `HARNESS_STORE_DIR`。 |
| harness 隧道连不上（WS 404） | 直连 rrserver 时 `external_ws_base` 不应带 `/rr` 前缀。 |
| llm_server `/healthz` = `degraded` | 上游 LM Studio 未开或 `LMSTUDIO_BASE_URL` 不通，属预期降级。 |
| 镜像构建失败 | 两个 Dockerfile 都在镜像内编译，需能访问 crates.io；构建上下文必须是 workspace 根 `server/`。 |
| 前端连不上后端 | 检查 `VITE_API_BASE` / `config/dev.ts` 的 apiBase 是否指向 `:8011`（经 nginx 为 `/api`），以及容器是否在跑。 |
| 改了 ps1 脚本后中文变乱码 | Windows PowerShell 5.1 按 ANSI 读取无 BOM 的脚本：含中文的 `.ps1` **必须存为 UTF-8 with BOM**。 |
| 视觉识别无独立服务 | 视觉与文本共用 `google/gemma-4-12b-qat` 多模态端点。 |
| 出生日期选择器在手机浏览器里背景跟着滚、月/日显示留空白 | Taro 4 的 `<Picker mode='date'>` 在 H5 是 Stencil transform 轮盘，触摸冒泡到 document 致背景滚动、transform 重绘留空白，应用层无法修复。已替换为自建三列滚动选择器 `src/components/BirthDatePicker.tsx`（逻辑在 `src/utils/birthdate.ts`，含单测），跨端可用，**不要再换回 Taro 原生日期 Picker**。 |
