# Nexa

> 本地优先的桌面助手与个人知识工作台。

[English README](README.md)

Nexa 面向个人文件、知识库和日常桌面工作。你可以把笔记、PDF、日志、表格、演示文稿和图片等本地资料交给它索引，然后用自然语言检索、追问、汇总、对比和生成文档。核心数据路径默认在本机运行；外部 LLM 只接收当前任务需要的受控上下文，而不是整份资料库。

## 产品定位

Nexa 不是单纯的聊天窗口，也不是只面向开发者的 agent 控制台。它的目标是：

- 本地优先的个人知识召回
- 基于证据的文件调查和问答
- 面向普通桌面用户的文档、办公和研究辅助
- 可恢复的对话、轨迹、记忆和集合工作流

## 主要能力

- 多来源本地文件索引，支持 include/exclude glob 规则
- SQLite FTS5 关键词搜索与向量语义搜索的混合检索
- Markdown、纯文本、日志、PDF、DOCX、XLSX、PPTX、图片等常见格式
- 图片和扫描 PDF 的 OCR，视频/音频处理可通过 feature flag 启用
- 基于引用证据的回答，支持 `[cite:CHUNK_ID]` 风格引用
- 聊天中的 thinking、工具调用、路由和状态轨迹
- 集合工作台：保存证据、整理笔记，并从集合继续追问
- 可配置的 OpenAI-compatible、Anthropic、Google Gemini、Ollama 等模型提供商；Google 直连目录包含 Gemini 3.6 Flash 与 Gemini 3.5 Flash-Lite，Qwen 目录提供 Qwen3.8 Max Preview 的 Token Plan 专属入口
- 输入框下方采用模型/推理强度双段控件：左侧直接切换模型，右侧按模型能力选择推理档位或 thinking budget
- 对话归档中心：可搜索、完整只读查看、恢复、永久删除，并正确处理归档对话的直接链接
- 与当前对话绑定的终端：框选内容可送入输入框，常规复制/粘贴快捷键可用；Agent 可读取近期输出，写入和中断必须经过用户确认
- 角色、Skills、Slash commands、子代理和项目工具
- 本地用户记忆、Agent 工作流记忆和项目记忆
- Markdown 数学公式与 Mermaid 图表渲染
- 隐私配置：路径排除规则和正则脱敏规则

## 本地运行

先安装 Node.js、Rust stable 和 Tauri 所需平台依赖，然后分别安装根目录与桌面前端的锁定依赖：

```bash
npm ci
npm ci --prefix apps/desktop
cd apps/desktop
npm run tauri -- dev
```

桌面前端位于 `apps/desktop`，核心 Rust crate 位于 `crates/core`。只开发浏览器前端时，在 `apps/desktop` 执行 `npm run dev`。

常用命令：

```bash
# 在 apps/desktop 执行
npm test
npm run build
npm run e2e

# 在仓库根目录执行
cargo fmt --all -- --check
cargo clippy -p nexa-core -- -D warnings
cargo test -p nexa-core
cargo check -p nexa-desktop
```

完整桌面打包命令是在 `apps/desktop` 执行 `npm run tauri -- build`。Windows 本机没有 Playwright Chromium 时，可设置 `NEXA_PLAYWRIGHT_CHANNEL=msedge` 后运行 E2E。

## 打包产物

桌面应用使用 Tauri v2 打包。默认保留每个系统最常见的一种安装包，减少 release 页面噪音：

- Windows: NSIS 安装包
- macOS: DMG
- Linux: AppImage

## 仓库结构

```text
apps/desktop        Tauri 桌面应用与 React 前端
crates/core         本地索引、检索、Agent、工具和隐私逻辑
docs                产品方向、路线图、架构和 i18n 指南
shared              前后端共享类型和资源
testdata            测试样例数据
```

## 隐私原则

- 索引、解析、检索、OCR、对话持久化默认在本机完成。
- 可以配置排除路径，避免 `.env`、密钥、依赖目录和日志进入索引。
- 可以配置正则脱敏规则，在内容发送给模型前替换敏感片段。
- 项目本身不包含遥测流水线。

## 参考文档

- [产品方向](docs/PRODUCT_DIRECTION.md)
- [路线图](docs/ROADMAP.md)
- [UX 质量标准](docs/UX_QUALITY_BAR.md)
- [国际化指南](docs/I18N_GUIDELINES.md)
- [生态架构](docs/ECOSYSTEM_ARCHITECTURE.md)
- [文档索引](docs/README.md)
