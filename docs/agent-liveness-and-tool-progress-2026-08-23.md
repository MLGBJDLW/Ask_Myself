# Agent 活性、工具推进与 Browser / Computer 协同深度研究（2026-08-23）

> 核对时间：2026-08-23（Asia/Shanghai）
> 范围：长推理模型在 5–10 分钟内持续 reasoning 却迟迟不调用工具；主 agent、subagent、browser agent 与 computer-use agent 的并发、交接、取消、恢复和死锁规避。
> 证据口径：只使用厂商官方文档、项目所有者的官方源码/仓库和论文原文。表中的“事实”来自链接来源；“Nexa 建议”是本文基于这些事实作出的工程推导，不能反向冒充上游能力。
> 与现有文档的关系：本报告是日期化研究；稳定协议仍以 [`AGENT_STREAMING_PROTOCOL.md`](./AGENT_STREAMING_PROTOCOL.md)、[`ORCHESTRATION_RUNTIME.md`](./ORCHESTRATION_RUNTIME.md) 和 [`computer-use-integration.md`](./computer-use-integration.md) 为准。

## 结论先行

本轮核对了 **22 个项目或官方实现家族**。最重要的结论不是把一个统一 timeout 调小，而是同时补齐两套相互独立的机制：

1. **推理活性与任务推进必须分开计时。** SSE/WebSocket ping、reasoning delta、模型 token、工具输出和“界面真的改变了”不是同一类进展。只用一个 `last_activity_at`，既会把一直吐 reasoning 的模型误判为“有推进”，也会把安静但仍在合法长推理的模型误杀。
2. **工具型交互必须有 first-action contract。** 明确需要 browser/computer/文件/终端证据的交互回合，不应默认用最高推理档。应先获得最便宜的新鲜观察，再把深推理用于观察后的规划或验证；到达 first-action deadline 时，取消并以更小提示、较低 effort、缩小工具集和强制工具选择重发一次。
3. **Browser 和 Computer 不能同时驱动同一个浏览器表面。** DOM/CDP lane 与 Windows 输入/UIA lane 必须通过带 epoch 的表面所有权交接；任何一方都不能在等待模型、审批、用户或子 agent 时持有执行锁。
4. **取消、超时和重试不是一回事。** 取消必须贯穿模型请求、工具、子进程、子 agent 和审批；timeout 必须产生一次且仅一次的终态。发生过可能有副作用的动作后，不得盲目重放，必须先观察并返回 `uncertain_effect` 或验证后的结果。
5. **长任务用预算和 checkpoint 收敛，而不是无限 patience。** 步数、工具调用、token、成本和 wall-clock 都应独立设限；接近预算时先强制 checkpoint / best-effort summary，再终止。用户等待和审批不计入“无进展”。
6. **供应商配置必须能力化。** Kimi K3、Qwen3.8-Max、OpenAI 和 Anthropic 的 effort、thinking budget、forced tool choice、reasoning history 与取消语义不同；不能把统一枚举值直接透传，更不能静默丢字段。

## 一、先纠正两个型号名称

### Kimi K3 是正式官方型号

[Moonshot AI 官方 Kimi K3 仓库](https://github.com/MoonshotAI/Kimi-K3/blob/main/README.md)明确给出模型 ID `kimi-k3`。它始终开启 thinking，顶层 `reasoning_effort` 支持 `low`、`high`、`max`，**默认是 `max`**。多轮和工具调用还要求把 API 返回的完整 assistant message 原样回传，包括 `reasoning_content` 与 `tool_calls`。因此，若 Nexa 对 K3 不显式设置 effort，就很容易把普通交互回合变成最长思考回合。

### Qwen3.8-Max 也是正式官方型号，但 preview 与正式 ID 要区分

[Qwen3.8 官方仓库](https://github.com/QwenLM/Qwen3.8)和[阿里云正式模型页](https://help.aliyun.com/en/model-studio/qwen3-8-max)都确认 Qwen3.8-Max；正式 API ID 是 `qwen3.8-max`。更早的 Qwen Code 案例把 `qwen3.8-max` 口语写法解释为 `qwen3.8-max-preview`，只能视为当时的预览型号，不能硬编码成当前唯一 ID。

真正影响“六七分钟不动工具”的是默认推理参数：[阿里云 Chat Completions 文档](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)说明 Qwen3.8-Max 的 `reasoning_effort` 档位为 `low`、`medium`、`xhigh`，默认 `xhigh`；未显式设置时默认 thinking budget 为 131,072 tokens，而 `low`、`medium`、`xhigh` 分别映射到 4,096、16,384、262,144。`reasoning_effort` 与 `thinking_budget` **不能同时提交**。在 [Responses 兼容接口](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses)中，`max_output_tokens` 还同时覆盖 reasoning 与最终回复。

这两项直接导出一个实现结论：**交互式工具回合不能继承 Kimi K3 的默认 max，也不能继承 Qwen3.8-Max 的默认 xhigh / 131K thinking budget。**

## 二、22 个项目 / 实现的证据矩阵

成熟度：**A** = 可直接借鉴的运行时；**B** = 成熟参考实现；**C** = 研究/局部参考；**D** = 专有 API，只能借鉴契约。

| # | 项目与一手来源 | 已证实的活性 / 交互机制（事实） | Nexa 可迁移结论（建议） | 许可证与边界 |
|---:|---|---|---|---|
| 1 | [OpenAI Codex app-server](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md) | Turn 有 `started → completed` 终态；item 有 `started → completed` 生命周期；`turn/interrupt` 最终产生 `interrupted`；steer 与 interrupt 分离；审批请求在 turn 完成/中断时清理。中断 turn 不会自动杀掉 background terminal。 | Nexa 的 turn、tool、审批均保持 exactly-one-terminal；stop 必须另行按策略清理 owned background jobs。 | [Apache-2.0](https://github.com/openai/codex/blob/main/LICENSE)，**A**。公开 CLI/runtime 不等于 Codex Desktop 全部实现。 |
| 2 | [OpenAI Codex 输入队列](https://github.com/openai/codex/blob/main/codex-rs/tui/src/chatwidget/input_queue.rs)与[流超时配置](https://github.com/openai/codex/blob/main/codex-rs/core/config.schema.json) | 源码把 queued follow-up、pending steer、rejected steer 分为三条队列；provider 配置把 streaming idle timeout、stream retries 和 WebSocket connect timeout 分开。 | 用户“改方向”“排到下一轮”“立即停止”必须是三种显式意图；transport idle 不能兼任 semantic-progress watchdog。 | 同上，**A**。 |
| 3 | [OpenAI Agents SDK](https://openai.github.io/openai-agents-python/running_agents/)与[结果/取消契约](https://openai.github.io/openai-agents-python/results/) | 有 `max_turns`、本地 function-tool 并发上限、强制 `tool_choice`、immediate / after-turn cancel、可恢复 `RunState`；调用 cancel 后仍要继续消费事件直到清理完成。 | 取消应是可观察的异步屏障，而不是改一个 UI 标志；wait timeout 与 run cancel 分开。 | [MIT](https://github.com/openai/openai-agents-python/blob/main/LICENSE)，**A/B**。SDK 本身没有替应用定义 first-action deadline。 |
| 4 | [GPT-5.4 CUA Sample App](https://github.com/openai/openai-cua-sample-app) | 把 native computer 与 Playwright code lane 放在同一 scenario/replay/evaluator 契约下；runner 管理 run-scoped workspace、SSE、浏览器 session、最大 response turns 与确定性验证。 | Browser 与 Computer 共享的是 run/evidence/evaluator，不应共享未仲裁的执行权；两条 lane 应能用同一回放格式验收。 | [MIT](https://github.com/openai/openai-cua-sample-app/blob/main/LICENSE)，**B**。官方明确是 browser-focused sample，且安全确认未完整实现。 |
| 5 | [Anthropic thinking](https://platform.claude.com/docs/en/about-claude/models/extended-thinking-models)、[task budgets](https://platform.claude.com/docs/en/build-with-claude/task-budgets)与[server-tool loop](https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools) | `max_tokens` 是单请求硬上限；effort 是软控制；task budget 是整个 agent loop 的建议预算；server loop 可用 `pause_turn` 续跑。手动 extended thinking 与强制 `tool_choice:any/tool` 不兼容，adaptive thinking 才支持强制工具。 | capability profile 必须表达“thinking 模式 × forced-tool 是否兼容”；预算接近耗尽时让模型收尾，硬上限只作最后保险。 | Messages/API 专有，**D**；[官方 quickstarts 为 MIT](https://github.com/anthropics/claude-quickstarts/blob/main/LICENSE)。 |
| 6 | [Kimi K3](https://github.com/MoonshotAI/Kimi-K3/blob/main/README.md)、[Kimi Code loop 配置](https://moonshotai.github.io/kimi-code/en/configuration/env-vars.html)与[后台任务工具](https://moonshotai.github.io/kimi-code/en/reference/tools.html) | K3 默认 max effort 且 preserved-thinking；Kimi Code 分开限制每 turn steps、每 step attempts、MCP startup/tool timeout 和 subagent wall time。其后台 task 输出可通过状态工具与自动通知回流。 | K3 的 GUI/tool profile 默认 `low`；只有 checkpoint/review 才升 `high`，`max` 留给显式异步质量优先任务。中断后的部分 reasoning 不能伪造为完整 assistant history。 | [Kimi K3 自定义许可证](https://github.com/MoonshotAI/Kimi-K3/blob/main/LICENSE)（含大规模 MaaS/商业条件）；[Kimi Code MIT](https://github.com/MoonshotAI/kimi-code/blob/main/LICENSE)，**A/B**。 |
| 7 | [Qwen3.8-Max 参数](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses)、[Qwen Code headless budgets](https://qwenlm.github.io/qwen-code-docs/en/users/features/headless/)与[daemon deadlines](https://github.com/QwenLM/qwen-code/blob/main/docs/users/qwen-serve.md) | Qwen Code 有 `max-wall-time`、`max-tool-calls`、`max-session-turns`、共享 AbortController、prompt absolute deadline、独立 SSE writer idle deadline；MCP 在交互模式渐进发现，避免坏 server 阻塞首屏。 | Qwen3.8 交互 profile 使用 `low`，replan 使用 `medium`；禁止同时发 effort 与 budget。MCP discovery 不应阻塞整个 agent 可用性。 | [Qwen3.8 repo Apache-2.0](https://github.com/QwenLM/Qwen3.8/blob/main/LICENSE)，具体权重仍按 model card；[Qwen Code Apache-2.0](https://github.com/QwenLM/qwen-code/blob/main/LICENSE)，**A**。 |
| 8 | [OpenHands stuck detector](https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-sdk/openhands/sdk/conversation/stuck_detector.py)与[Conversation runtime](https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-sdk/openhands/sdk/conversation/impl/local_conversation.py) | 只扫描 active branch 最近的有界事件，识别重复 action/observation、action/error、monologue 与交替循环；另有 iteration、cost budget 和 cooperative cancellation token。 | loop detection 比“连续工具名相同”更丰富，且必须比较结果是否变化；检测窗口必须有界并忽略 abandoned branch。 | [MIT](https://github.com/OpenHands/software-agent-sdk/blob/main/LICENSE)，**A/B**。 |
| 9 | [OpenCode prompt runtime](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/session/prompt.ts)与[SDK abort](https://github.com/anomalyco/opencode/blob/dev/packages/web/src/content/docs/sdk.mdx) | session 级 cancel 与 `AbortSignal` 被传给工具和子任务，子任务中断写入 cancelled tool result。该源码证明了取消传播，但不能单独证明存在 wall-clock deadline。 | 不要把“有 AbortController”误当“有 watchdog”；所有外部 await 仍需明确 deadline 与一次性终态。 | [MIT](https://github.com/anomalyco/opencode/blob/dev/LICENSE)，**B**。 |
| 10 | [Cline checkpoints](https://docs.cline.bot/core-workflows/checkpoints)与[ClineCore API](https://github.com/cline/cline/blob/main/.agents/skills/cline-sdk/references/clinecore/api.md) | 每次工具后写 shadow-Git checkpoint；可只恢复文件、只恢复任务或同时恢复；SDK 有 abort、stop、restore，并区分 `completed/max_iterations/aborted/mistake_limit/error`。 | replan 前先形成 durable baton/checkpoint；budget exhaustion 与 infra error 必须是不同终态。 | [Apache-2.0](https://github.com/cline/cline/blob/main/LICENSE)，**A/B**。Checkpoint 对大仓库有成本，不能在输入锁内执行。 |
| 11 | [Roo Code MCP timeout](https://github.com/RooCodeInc/Roo-Code/blob/main/apps/docs/docs/advanced-usage/available-tools/use-mcp-tool.md)、[settings](https://github.com/RooCodeInc/Roo-Code/blob/main/apps/docs/docs/features/settings-management.md)与[tool result 处理](https://github.com/RooCodeInc/Roo-Code/blob/main/src/core/assistant-message/presentAssistantMessage.ts) | MCP tool timeout 可配置，API request、command execution 也分开；未知/无效工具得到对应 tool result，避免工具协议悬空。 | deadline 必须按 provider、MCP、command 分层；解析失败同样要闭合 call lifecycle 并让模型可恢复。 | [Apache-2.0](https://github.com/RooCodeInc/Roo-Code/blob/main/LICENSE)，**B**；官方仓库截至核对日已 archived，只作参考。 |
| 12 | [Aider architect/editor mode](https://github.com/Aider-AI/aider/blob/main/aider/website/docs/usage/modes.md)与[配置项](https://aider.chat/docs/config/options.html) | 把高推理 architect 与具体 edit model 分成两次请求；支持 API timeout、thinking tokens 与 chat-history token 上限。 | 对工具任务采用“短 action planner → executor”比让最高 effort 模型一次性想完更可控；深推理结果必须落为短结构化执行契约。 | [Apache-2.0](https://github.com/Aider-AI/aider/blob/main/LICENSE.txt)，**B**。不是 computer-use runtime。 |
| 13 | [Continue Agent handshake](https://github.com/continuedev/continue/blob/main/docs/ide-extensions/agent/how-it-works.mdx)与[request timeout](https://github.com/continuedev/continue/blob/main/docs/reference/json-reference.mdx) | 每轮明确 tools → permission → execute → tool result → model 的握手；模型 capability 可显式声明，LLM request timeout 默认很长。 | provider capability 探测与实际 wire 支持必须一致；“超长默认 timeout”只能是兜底，不能代替交互 first-action deadline。 | [Apache-2.0](https://github.com/continuedev/continue/blob/main/LICENSE)，**B**。 |
| 14 | [Open Interpreter config](https://github.com/openinterpreter/openinterpreter/blob/main/docs/config-reference.md)与[当前模型/harness 文档](https://www.openinterpreter.com/docs/terminal/models) | 当前 Open Interpreter 已是 Codex fork，而不是旧版 classic Python loop。它按 provider 自动选择 Kimi/Qwen/Claude 等 harness，并分开设置 stream idle/retries、MCP startup/tool timeout、subagent job runtime 与 model reasoning effort。 | provider、model、wire API、harness 四者必须分别建模；旧版 `--loop/max_budget` 资料不能继续当当前实现证据。对 K3 仍要显式把交互 effort 从 launch max 降到 low。 | [Apache-2.0](https://github.com/OpenInterpreter/openinterpreter/blob/main/LICENSE)，**A/B**。 |
| 15 | [Browser Use 参数](https://docs.browser-use.com/open-source/customize/agent/all-parameters)与[运行循环源码](https://github.com/browser-use/browser-use/blob/main/browser_use/agent/service.py) | 分开限制 LLM、step、每类 browser event；有 max steps、max failures、最后 best-effort response、pause/resume/stop、CDP reconnect、初始 action timeout 和 loop detection。 | Browser lane 采用多层 deadline，不用一个全局 timeout；失败到阈值后给一次收尾机会；initial observe/action 也必须受 deadline。 | [MIT](https://github.com/browser-use/browser-use/blob/main/LICENSE)，**A**。 |
| 16 | [Microsoft Playwright MCP](https://github.com/microsoft/playwright-mcp)与[官方配置说明](https://github.com/microsoft/playwright/blob/main/docs/src/getting-started-mcp.md) | action 默认 5s、navigation 默认 60s，HTTP heartbeat 可独立配置；并行 client 应使用 isolated 或不同 user-data-dir；snapshot refs 优先于截图坐标。 | 一个 agent/browser context 一份 profile 与执行序列；heartbeat 只证明 transport；action/navigation/settle 分离超时。 | [Apache-2.0](https://github.com/microsoft/playwright-mcp/blob/main/LICENSE)，**A**。 |
| 17 | [BrowserGym environment](https://github.com/ServiceNow/BrowserGym/blob/main/browsergym/core/src/browsergym/core/env.py)与[BrowserGym 论文](https://arxiv.org/abs/2412.05467) | 明确 `reset/step → observation/reward/terminated/truncated/info`；Playwright action timeout 会进入结构化 `action_exec_timeout`；统一多 benchmark observation/action。 | Nexa 的 browser/computer smoke 必须用同一 episode 终止语义，timeout 不能伪装成功；生产 judge 与 benchmark reward 分开。 | [Apache-2.0](https://github.com/ServiceNow/BrowserGym/blob/main/LICENSE)，**A/B**。 |
| 18 | [AgentLab](https://github.com/ServiceNow/AgentLab) | Ray backend 可终止超时 job；Study 可重新加载并只续跑 incomplete/error jobs；任务依赖约束并发，避免共享站点互相污染。 | 测试 runner 要有 worker hard-kill、resume 与独立环境快照；不能用共享登录态做并发 GUI rollout。 | [Apache-2.0](https://github.com/ServiceNow/AgentLab/blob/main/LICENSE)，**A/B**。 |
| 19 | [Microsoft UFO agent hierarchy](https://github.com/microsoft/UFO/blob/main/documents/docs/infrastructure/agents/agent_types.md)、[HostAgent 状态](https://github.com/microsoft/UFO/blob/main/documents/docs/ufo2/host_agent/state.md)、[Host strategy](https://github.com/microsoft/UFO/blob/main/documents/docs/ufo2/host_agent/strategy.md)与[UFO² 论文](https://arxiv.org/abs/2504.14603) | HostAgent 路由到 per-app AppAgent，共享 blackboard 但每个 AppAgent 专属应用；状态机有显式 handoff/finish；长 LLM 调用用 executor 隔离，避免阻塞 WebSocket ping/pong。 | Supervisor 只分配 surface；worker 独占一个 app/window。共享事实放 blackboard，执行对象和锁绝不共享。 | [MIT](https://github.com/microsoft/UFO/blob/main/LICENSE)，**A/B**。 |
| 20 | [Agent-S](https://github.com/simular-ai/Agent-S)与[Agent S2 论文](https://arxiv.org/abs/2504.00906) | generalist/specialist 分工、分层计划与 grounding 分离；公开 runtime 对本地脚本设置有限 timeout，并限制视觉轨迹长度。 | planner、grounder、executor 分开能降低单模型长思考；但仍需在 harness 外补 first-action 与 surface lease。 | [Apache-2.0](https://github.com/simular-ai/Agent-S/blob/main/LICENSE)，**B/C**。论文效果不能当单次生产 SLA。 |
| 21 | [UI-TARS Desktop SDK](https://github.com/bytedance/UI-TARS-desktop/blob/main/packages/ui-tars/sdk/README.md) | `AbortSignal` 贯穿 agent，默认 `maxLoopCount=25`，并显式报告 INIT/RUNNING/END/MAX_LOOP；model 与 operator 分离。 | 每种 agent loop 都必须有可取消信号、最大循环和 distinct terminal；operator 可以替换，但 action/result 契约保持稳定。 | [Apache-2.0](https://github.com/bytedance/UI-TARS-desktop/blob/main/LICENSE)，**B/C**。 |
| 22 | [Hermes delegation](https://hermes-agent.nousresearch.com/docs/user-guide/features/delegation)、[gateway/watchdog 配置](https://hermes-agent.nousresearch.com/docs/user-guide/configuration)与[browser session 隔离](https://hermes-agent.nousresearch.com/docs/user-guide/features/browser/) | child heartbeat 只有 API、tool、activity timestamp 等真实活动才刷新；无进展才让 inactivity timeout 生效。可选 child hard cap 返回结构化 phase/elapsed 元数据。gateway 有 session turn lease timeout；并行 browser harness 用命名 session 隔离。 | 这是最值得迁移的 liveness 形状：meaningful heartbeat、结构化 child timeout、turn lease、browser session name；定时 heartbeat 不得自我续命。 | [MIT](https://github.com/NousResearch/hermes-agent/blob/main/LICENSE)，**A/B**。 |
| 23 | [Cua Driver session contract](https://cua.ai/docs/reference/cua-driver/contracts)与[browser targeting](https://cua.ai/docs/concepts/browser-targeting-and-background-delivery) | 隐式 session 绑定认证 transport；默认 idle TTL 5 分钟；in-flight call 不会过期，完成后续租一次；transport close/end/revoke/expiry 共用 cleanup。browser refs 绑定 session + snapshot，导航或新快照使旧 ref 失效。 | Nexa 的表面 lease 使用相同原则：权威身份来自 transport/run，不来自可伪造 label；call in flight 不被 TTL 切断；每次 navigation/action 都推进 generation。 | [MIT](https://github.com/trycua/cua/blob/main/LICENSE.md)，**A**。 |

> 计数说明：表中按“实现家族”统计为 22 家；Codex 的 protocol 与 queue 两行属于同一项目，所以表格编号到 23。论文只用于解释架构，不额外充数。

## 三、为什么“思考六七分钟但不调用工具”会发生

### 1. 默认 effort 本身就可能是质量优先档

- Kimi K3 默认 `max`，且无法完全关闭 thinking。
- Qwen3.8-Max 默认 `xhigh`，Chat 兼容接口默认 thinking budget 为 131K；对 GUI 任务而言，这个默认远超一次“先截图/读 DOM”的需要。
- OpenAI 官方模型指南明确建议从较低 effort 做 latency baseline，高档只在 eval 显示质量收益时使用；`max_output_tokens` 同时包含 reasoning 与可见输出。[OpenAI model guidance](https://developers.openai.com/api/docs/guides/latest-model)
- Anthropic 将 effort 定义为软行为控制，`max_tokens` 才是硬上限；其 task budget 是整个 agent loop 的节奏提示，而非单次网络超时。

### 2. “有流量”不代表“有任务进展”

一个连接可能持续收到 keepalive；模型可能持续输出不可见或摘要化 reasoning；UI 也可能只是在刷新计时器。以上都不能证明 agent 已经获取新证据或改变环境。反过来，本地大模型做长 prefill 时可能几分钟没有 token，但进程仍健康。因此一个通用 `last_activity_at` 无法同时解决这两类情况。

### 3. 工具列表太大、提示太开放会把“先行动”变成“先穷举计划”

[Moonshot 的 prompt best practices](https://platform.moonshot.ai/docs/guide/prompt-best-practice)建议明确步骤、限定输出和拆分复杂任务；[OpenAI model guidance](https://developers.openai.com/api/docs/guides/latest-model)同样建议从能保留产品契约的最小 prompt 开始。对 GUI 任务，若同时暴露 browser、computer、shell、MCP、subagent 和几十个业务工具，却只说“深度完成”，高 effort 模型会先在巨大选择空间中规划。

### 4. 单一 hard timeout 会制造两种坏结果

- 太短：合法的长 reasoning / 本地 prefill 被当作断流，重试后重复计费。
- 太长：模型、MCP、browser、UIA 或 child 任一环节真挂死，整个 turn 看起来一直 Working。

正确方案是按阶段、按进展类型设 deadline，并让每个 deadline 触发不同恢复动作。

## 四、Nexa 应新增的 ProgressClock 与状态机

以下全部是 **Nexa 建议**。

### 4.1 五个独立的单调时钟

每个 `run_id / turn_id / call_id` 都维护：

| 时钟 | 允许刷新它的事件 | 明确不能刷新它的事件 | 用途 |
|---|---|---|---|
| `transport_activity_at` | socket bytes、SSE/WS frame、ping/pong | UI redraw | 判断连接是否物理活着 |
| `model_activity_at` | request ack、reasoning/text/tool-call delta、model terminal | transport keepalive | 判断 provider 是否仍在生成 |
| `semantic_progress_at` | 新证据、工具开始/完成、计划 checkpoint 改变、可见 answer 增量、状态验证通过 | 重复 reasoning、相同 heartbeat、相同 tool/result | 判断任务是否真的前进 |
| `external_effect_at` | DOM/URL/window/file/process 状态产生可验证变化 | 仅“tool returned success” | 判断外部世界是否改变 |
| `durable_checkpoint_at` | checkpoint / evidence ledger / terminal prefix 提交成功 | 内存状态 | 判断是否可安全重启 |

所有时间使用 monotonic clock；wall time 只用于显示和持久化时间戳。事件必须带完整的 run/turn/call/surface identity，旧 run 的 heartbeat 不能给新 run 续命。

### 4.2 分阶段 deadline

```text
CREATED
  -> REQUESTING        (等 provider request acknowledgement)
  -> THINKING          (模型活着，但还没有工具意图)
  -> TOOL_INTENT       (收到完整、可校验的 tool call)
  -> WAIT_APPROVAL     (暂停 stall / first-action 预算)
  -> TOOL_RUNNING      (按工具 deadline + output progress 计时)
  -> OBSERVING         (fresh state / effect verification)
  -> CHECKPOINTING     (短、可终止的持久化屏障)
  -> THINKING / DONE / FAILED / CANCELLED / TIMED_OUT
```

关键约束：

- `REQUESTING` 看的是 request-correlated ack，不能被旧连接的 ping 满足。
- reasoning delta 可刷新 `model_activity_at`，但在 tool-required turn 中不能刷新 first-action deadline。
- `WAIT_APPROVAL`、`WAIT_USER` 是可恢复暂停，不是 stall；进入前必须释放 surface execution lock。
- `TOOL_RUNNING` 的输出 heartbeat 只延长该工具的 inactivity deadline，不无限延长 hard deadline。
- tool terminal、turn terminal 都只接受第一次 compare-and-swap；晚到事件记审计但不改变状态。

### 4.3 First-action contract

在路由器判断“本回合必须获取外部证据”时设置 `action_required=true`，并记录：

- `first_tool_intent_deadline`：必须出现完整、可执行的 tool call。
- `first_effect_deadline`：必须完成观察或产生可验证外部效果。
- `allowed_first_tools`：通常只含 `browser_snapshot` / `computer_observe` / read-only search / file read。

到达 soft deadline：UI 显示“仍在模型推理，尚未调用工具”，允许用户 steer、继续等或降档重试。到达 hard deadline且还没有任何外部副作用：

1. 取消当前 provider generation，并等待取消屏障闭合。
2. 不保存、伪造或回放未完成的私有 reasoning。
3. 生成一个结构化 retry baton：目标、已知事实、尚未取得的第一份证据、允许的首个工具、剩余预算。
4. 仅重试一次：降 effort、缩小工具表、缩短 prompt、设置兼容的 forced tool choice。
5. 第二次仍无 action，终止为 `first_action_timeout`，而不是无限换 transport 重试。

只要已有 hosted tool、client tool 或可能的外部副作用开始，就禁止按上述方式盲目重放；先走 effect reconciliation。

## 五、供应商专用的交互 profile

以下是建议的默认策略，不是厂商 SLA。

| Provider/model | 交互式 tool turn | replan / verification | 明确异步质量优先 | 关键兼容规则 |
|---|---|---|---|---|
| Kimi K3 | `reasoning_effort=low`；首轮只暴露 observe/read 工具 | `high`，输入为已压缩证据 | `max`，必须 background/checkpointable | 始终 thinking；完整 assistant history 才能合法续接；中断的 partial reasoning 不回放 |
| Qwen3.8-Max | `reasoning.effort=low` 或 Chat `reasoning_effort=low` | `medium` | `xhigh`/`max` 仅显式启用 | effort 与 `thinking_budget` 二选一；`max_output_tokens` 包含 reasoning；记录最终生效映射 |
| OpenAI reasoning models | `low` 或 `medium`；tool-required 时用 `required`/具体 tool | `medium/high` | 高档或 pro/background | `max_output_tokens` 包含 reasoning；background 用于本来就允许长等待的任务，不用于掩盖交互 stall |
| Anthropic adaptive-thinking models | `low/medium` + forced tool（若该模型支持） | `high` | task budget + high/xhigh | manual extended thinking 不能与 `any`/指定 tool 强制选择组合；`pause_turn` 按官方内容续接 |
| 未知 OpenAI-compatible | 先 capability probe，默认最低可靠 effort | 显式升级 | 只有用户选择且 probe 通过 | 不支持的参数 fail visible；禁止静默丢弃或自行发明 effort 名称 |

建议增加版本化 `ProviderExecutionProfile`：

```text
provider_id, model_id, profile_version
reasoning_modes, accepted_efforts, default_effort
reasoning_budget_field, output_limit_includes_reasoning
forced_tool_modes, forced_tool_with_thinking
preserve_reasoning_items, supports_cancel, supports_background
stream_event_kinds, request_ack_kind, tool_call_kind
```

启动/切换模型时验证 profile；每次请求记录 requested 与 effective 值。若 adapter 做了映射（例如 `max → xhigh`），必须把映射显示在 trace/diagnostics 中。

## 六、简化 prompt 与工具面

以下是建议：

1. **只给当前阶段的工具。** Browser lane 不同时暴露 desktop pointer；computer native-dialog lane 不暴露任意 browser/CDP；普通网页优先 DOM/AX refs。
2. **第一轮先观察。** 系统契约只写一句可验证规则：需要操作时，本响应首先调用给定 observe 工具，不先输出长计划。
3. **深规划后移。** 获得 screenshot/DOM/UIA/文件状态后，才允许 `high` replan；大多数执行步恢复 `low/medium`。
4. **给模型剩余预算。** 注入 steps/tools/time 的 countdown；80% 时要求 checkpoint，90% 时要求 best-effort finalize。硬上限仍由 runtime 执行。
5. **短工具描述、结构化错误。** 描述必须包含返回字段、错误类别、是否有副作用和重试语义；不要塞入整个操作手册。
6. **不索取私有长 CoT。** UI 展示阶段、耗时、证据和简短 action preamble，而不是依赖模型泄露内部思维链来证明“有进展”。
7. **渐进工具发现。** 坏掉的 MCP server 不阻塞首个 prompt；已 ready 的工具先可用，后到的能力只在下一模型边界更新。

推荐的最小执行片段：

```text
<execution_contract>
This turn requires external evidence.
Call exactly one of the allowed observation tools in this response before analysis.
After every mutating action, obtain fresh state and verify the expected effect.
After two non-progress attempts, checkpoint and replan once; then stop with a structured blocker.
</execution_contract>
```

## 七、Browser Agent 与 Computer Agent 的所有权协议

### 7.1 两层所有权，不把长等待放进 mutex

1. **逻辑 owner lease**：说明哪个 worker 有权计划某个 surface，带 `lease_id + epoch + owner_agent_id + expires_at`；可跨模型回合存在。
2. **短 execution mutex**：只包住一次 capture/action/verification 的关键区；不得跨 LLM、审批、用户输入、subagent wait、网络重试或 checkpoint I/O。

`surface_id` 至少包含：

```text
browser: browser_instance + context_id + tab/target_id
window: process_creation_time + pid + window_id + desktop_session
desktop: desktop_session + display_id
```

用户鼠标/键盘接管、窗口身份变化、导航、新 snapshot、broker restart 都推进 epoch 或吊销 lease。所有 element/DOM/UIA refs 绑定 `surface_id + observation_generation + lease_epoch`。

### 7.2 Browser → Computer → Browser 单向 handoff

典型场景是网页点击后弹出原生文件选择器：

```text
BrowserWorker
  1. 完成当前 browser tool，等待 item terminal
  2. 保存 {context, tab, target, url, document_generation, expected_native_dialog}
  3. 吊销 DOM refs，释放 browser execution mutex
  4. CAS: owner(browser, epoch=N) -> TRANSFERRING(N+1)

Supervisor/Broker
  5. 枚举并验证 exact native window/process/session
  6. CAS: TRANSFERRING -> ComputerWorker(owner epoch=N+1)

ComputerWorker
  7. fresh observe；只做已批准的一次语义动作或输入
  8. fresh observe + effect result；释放 execution mutex
  9. CAS: ComputerWorker -> TRANSFERRING(N+2)

BrowserWorker
 10. 重新附着原 context/tab；核对 URL/target/document
 11. fresh DOM/AX snapshot；CAS 为 BrowserWorker(N+2)
```

任一步身份不符就 fail closed。Browser/CDP 与 Computer/UIA 不得对同一 browser window 并发 mutate；也不得为了“原子交接”同时持有两把锁，交接原子性来自 registry 的 CAS 状态。

### 7.3 锁顺序与禁止事项

- lifecycle barrier 只用于短状态转换；不能在其内等模型、工具或数据库长事务。
- surface registry lock 只用于 claim/CAS；claim 后立刻释放。
- action 执行先拿 surface execution mutex；只有确需 foreground input 时再拿全局 foreground-input mutex。
- 绝不反向持有 global-input 后再等待 browser/UIA/WGC。
- 绝不在持锁状态发 approval 或 `request_user_input`。
- parent 绝不持有 surface 再 `wait child`；child 通过 broker 获取自己的 lease。
- 一个 agent 不得同时拥有同一资源的 browser 与 computer lease；多 tab 可以并行，但同一 BrowserContext 的 profile/storage 写仍需串行化。

### 7.4 不确定副作用

若 click/type/navigation 在 transport timeout 前可能已执行，tool result 必须是：

```text
status = uncertain_effect
call_id, surface_id, lease_epoch
last_known_before_hash
reconciliation_required = true
```

下一步只能 fresh observe / reconcile，不能自动重试原动作。验证后再转为 `effect_confirmed`、`effect_absent` 或 `effect_ambiguous`。

## 八、Subagent、Browser 与 Computer 的交互协议

以下是建议：

1. **默认不给 child 原始全局输入能力。** child 获得 broker 签发的 surface-scoped capability；supervisor 能撤销。
2. **parent wait 是观察，不是 child timeout。** wait 返回 `still_running` 只表示本次等待窗口结束；child 真超时必须返回 `child_timed_out` 及 phase、elapsed、last progress。
3. **meaningful heartbeat。** 只由 request ack、model delta、tool start/output/end、checkpoint、surface effect 更新；周期 timer/health ping 不能刷新任务 activity。
4. **事件驱动完成。** 后台 child 完成通过 typed notification 唤醒 parent；状态查询永远是立即 snapshot，不提供可把 conversation 卡住的无限 block。
5. **父子取消域。** parent cancel 默认级联 child、owned tools 与 owned browser/computer leases；显式 `detached` task 才可继续，并转移到独立 run owner。
6. **审批上浮但不持锁。** child 请求审批时释放执行锁，审批由 outer run 呈现；恢复时重拿 lease 并重验 identity/generation。
7. **一次性 terminal。** child、tool 和 surface handoff 均有 monotonic state + CAS；late result 进入审计，不复活已取消 turn。
8. **有界 fan-out 与队列。** 每 run / provider / surface 分别设并发上限；背压要显式 `queue_saturated`，不能无界积累事件或 silently drop terminal。

## 九、建议的初始默认值

这些是 Nexa 的起始 A/B 参数，不是外部项目的通用真理；本地模型和高延迟 endpoint 必须有单独 profile。

| 参数 | Hosted 交互起始值 | Local/slow profile | 触发动作 |
|---|---:|---:|---|
| request ack deadline | 20s | 120s | 新 transport 重试一次；无 ack 不复用可疑连接 |
| first tool intent soft / hard | 45s / 120s | 120s / 600s | soft 提示；hard 取消并降档/强制 observe 重试一次 |
| model inactivity | 180s | 1800s 或按实测禁用 | 终止 provider call；与 semantic deadline 分开 |
| semantic no-progress | 120s | 600s | checkpoint + replan；重复一次后终止 |
| browser action / navigation | 15s / 60s | 30s / 120s | fresh snapshot；可能有副作用则 reconcile |
| computer capture/action | 15s / 30s | 30s / 60s | broker 取消；fresh identity verify |
| browser session idle TTL | 300s | 600s | cleanup；in-flight call 不过期 |
| surface owner lease | 120s，可续租 | 300s | revoke + epoch++；不切断 in-flight critical section |
| GUI max steps | 50 | 50 | 80% checkpoint、90% wrap-up、100% distinct max_steps terminal |
| identical non-progress cycle | 3 | 3 | replan 一次；再次出现则 stuck |
| idempotent retry | 最多 2 次 | 最多 2 次 | 指数退避；副作用动作不在此列 |
| UI liveness update | 10s | 10s | 只展示 phase/elapsed/last evidence，不刷新 semantic clock |

First-action hard deadline 只适用于 `action_required` 的交互回合。显式的 deep research、pro/background、用户选择 max effort 的任务，应展示预计高延迟并走 checkpointable async path，而不是被这个 deadline 强制降档。

## 十、P0 / P1 / P2 升级顺序

### P0：先消灭静默挂死和互锁

1. 引入五类 ProgressClock 和 phase-specific deadline。
2. 所有 tool/child/approval/turn 都有 exactly-one-terminal；取消后等待清理屏障。
3. Browser 与 Computer 接入统一 SurfaceLeaseRegistry，禁止同表面双 owner。
4. 所有审批、用户输入、subagent wait 前释放 execution mutex；恢复后重验 generation。
5. provider 请求、MCP、browser action/navigation、computer broker、child wait 分层 timeout。
6. UI event loop 与 executor 分离，stop/steer/redraw 永远不等待长 future。

### P1：解决长推理不行动

1. 加 `ProviderExecutionProfile` 和 requested/effective 参数诊断。
2. Kimi K3 interactive 默认 low；Qwen3.8-Max interactive 默认 low，禁止 effort + budget 同发。
3. tool-required turn 启用 first-action contract、缩小首轮工具集和兼容的 forced tool choice。
4. hard deadline 后只允许一次降档重试；第二次返回结构化 timeout。
5. budget countdown、80% checkpoint、90% finalization。
6. 提示模板删除冗余规划要求，不要求长 CoT；先观察再推理。

### P2：恢复、评测与进程隔离

1. WGC/UIA/CDP 移入可终止 broker；broker crash 自动吊销 lease。
2. 统一 browser/computer replay schema 与 deterministic evaluator。
3. 基于 BrowserGym/AgentLab 运行并发、timeout、resume、污染隔离回归。
4. 统计 time-to-request-ack、time-to-first-tool、time-to-first-effect、semantic-stall、取消收敛时间和 lease contention。
5. 用真实 Kimi K3 low/high/max、Qwen3.8 low/medium/xhigh 做同任务矩阵，而不是只比较最终成功率。

## 十一、必须加入的故障注入测试

1. provider 连接有 ping，但没有 request-correlated ack。
2. reasoning delta 持续 8 分钟，但 tool-required turn 无工具意图。
3. 本地模型 5 分钟 prefill 无 token，profile 明确允许等待。
4. first-action hard timeout 后取消；旧流晚到 tool call 不得执行。
5. Qwen3.8 同时配置 effort 与 budget 时在客户端预检失败。
6. Anthropic manual thinking + forced tool 的不兼容组合在发送前失败。
7. Kimi K3 中断后不得把 partial reasoning 拼成 assistant history。
8. browser navigation 已发生但响应超时：不得重复 navigate，先 reconcile URL/DOM。
9. browser 点击弹出 native file dialog，执行完整 browser→computer→browser handoff。
10. 两个 subagent 同时请求同一 tab；只能一个获得 owner lease，另一个得到有界等待/冲突结果。
11. parent 持有逻辑 owner 时 child 请求审批；任何执行锁都不得保留。
12. child 一直发 timer heartbeat，但无 API/tool/evidence；meaningful inactivity timeout 必须触发。
13. parent wait 30s 返回 `still_running`，child 不被误标 timed out。
14. cancel 时同时存在 LLM stream、MCP call、browser action、approval 和 background terminal；各自恰好一个终态。
15. computer action timeout 后用户手动改变窗口；fresh identity/generation 检查必须拒绝旧 action。
16. surface lease owner 进程崩溃；TTL/broker recovery 吊销 capability，不留永久锁。
17. event queue 饱和；普通 preview 可合并，但 terminal、approval resolution、lease revoke 不能丢。
18. restart/resume 恢复 checkpoint 后，旧 run/lease/element ref 全部失效。
19. BrowserContext 并行 session 使用不同 profile/storage，不发生 tab、cookie 或 event 串线。
20. UI 主事件循环被工具 future 卡住的模拟；Esc/stop/redraw 仍在限定时间内响应。

验收建议：Hosted tool-required turn 的 `time_to_first_tool_intent` P95 应显著低于现状 5–10 分钟；任何超时/取消测试都应在限定时间内产生可恢复终态，且无重复外部副作用。

## 十二、明确不要照搬的模式

- 不把 keepalive、token count、spinner 或定时“still working”当 semantic progress。
- 不用一个 timeout 覆盖 provider、tool、navigation、approval、child 和整 turn。
- 不在所有任务上强制工具；只有路由器确认 `action_required` 才启用 first-action contract。
- 不默认给 Kimi K3 `max` 或给 Qwen3.8-Max `xhigh` 做交互式 GUI step。
- 不在 manual extended thinking 不支持时强塞 forced tool 参数。
- 不让 browser agent 与 computer agent 并发操作同一 browser window。
- 不在等待 LLM、审批、用户或 child 时持有 surface/global-input lock。
- 不把 wait timeout 说成 child timeout。
- 不对可能已经执行的 click/type/navigation 自动重试。
- 不让 child 的周期 heartbeat 给自己无限续命。
- 不共享真实登录 browser profile 给并行 agent；附着现有 profile 必须独立授权。
- 不用模型输出的“done”代替状态验证和 evaluator。

## 十三、研究边界与链接核验

- 论文 benchmark 的 step budget、模型快照、环境镜像和 rollout 数不同，本文没有拼接排行榜。
- 官方源码展示的是当前公开实现，不保证云端闭源服务内部完全相同。
- GitHub issue 没有作为能力证据写入矩阵；故障建议来自官方源码/文档所展示的契约和本文工程推导。
- 许可证按项目根 `LICENSE` 或官方 model license 核对；模型权重、数据集和第三方依赖仍需逐 artifact 审计。

本文写入完成后提取了 **78 个唯一外链**，逐一执行带重定向的 HTTP GET 与最终落点核对：**78/78 为 HTTP 2xx**（15 个 `200`、63 个因范围请求返回 `206`），没有保留 404、登录墙、搜索结果页或跳往非一手资料的链接。核对过程中发现并修正了两处已迁移路径：Cua 根许可证已改为 `LICENSE.md`；Open Interpreter 已从旧 classic Python 实现迁移为当前 Codex fork，因此本文改用当前 `config-reference.md` 和官方模型/harness 文档，不再引用过期的 `all-settings.mdx`。
