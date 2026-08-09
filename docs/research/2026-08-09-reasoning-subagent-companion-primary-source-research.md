# Reasoning、Subagent 与 Companion 升级的一手资料研究

- 日期：2026-08-09
- 状态：实现研究，不是稳定架构规范
- 输入：`D:\Nexa.txt`
- 适用范围：reasoning 可见性与 provider-native replay、subagent 生命周期与运行时隔离、Companion 渲染与行为

## 结论摘要

三条升级线都应当修复“两个生命周期被错误绑在一起”的问题：

| 升级线 | 必须拆开的生命周期 | 推荐给 Nexa 的最小契约 | 不能用来替代的捷径 |
| --- | --- | --- | --- |
| Reasoning | 当前响应的生成/展示；下一请求的原生 replay | `requestControl`、`displayCapture`、`replayPolicy` 分别判定；当前可见输出不依赖未来 replay 是否可信 | `Unknown + tools` 时关闭整轮 reasoning；把可见文本伪装成 provider-native item 回放 |
| Subagent | 创建；观察；有界等待；输入；取消；回收 | `spawn` 在 child 完成前返回稳定 handle；状态快照与增量事件分流；SQLite 由专用同步 owner 执行 | 把 `spawn` 实现成 join；把无 UI 事件等同于断线；在 Tokio worker 上持有同步连接锁 |
| Companion | 候选资源加载/解码；已提交资源显示；行为状态；动画时钟 | 旧资源持续显示，候选资源 decode 完成且 generation 仍最新时一次提交；单写者时钟与有优先级的状态机 | 切换前 `asset = null`；一个 interval tick 固定前进一帧；配置字段存在就算行为完成 |

这份研究支持 `D:\Nexa.txt` 的问题判定，但资料本身不替 Nexa 决定兼容策略。例如 OpenAI Responses 的 reasoning item 契约不能自动推广到任意 OpenAI-compatible 服务；Codex 的工具名称也不必成为 Nexa 的持久 API。

## 仓库约定与研究方法

`docs/README.md` 与 `docs/ARCHITECTURE.md` 都将带日期的一次性实现研究放在被忽略的 `docs/research/`，稳定契约才进入正式文档。因此本文遵循现有研究笔记的做法：

- 只引用官方文档、规范或仓库实际源码；
- GitHub 源码均固定到完整 commit SHA；
- 明确区分“源码事实”“对 Nexa 的推导”“不应照搬”；
- 许可证只说明参考源码的许可，不表示 Nexa 已复制或引入对应实现。

研究快照：

| 来源 | 固定版本 | 许可证 | 本文使用面 |
| --- | --- | --- | --- |
| OpenAI Responses 官方文档 | 2026-08-09 检索的官方页面 | 官方文档条款 | reasoning summary、opaque reasoning item、stateless replay |
| OpenAI Codex | [`94937de51ba28d4b308dbe1b8472d6fe1dddad28`](https://github.com/openai/codex/tree/94937de51ba28d4b308dbe1b8472d6fe1dddad28) | [Apache-2.0](https://github.com/openai/codex/blob/94937de51ba28d4b308dbe1b8472d6fe1dddad28/LICENSE#L1-L29) | spawn、wait、interrupt、registry、capacity reservation |
| Pi（原 `badlogic/pi-mono` 地址现重定向到该仓库） | [`936aff00918de1187f085f123c2812d8f2d67745`](https://github.com/earendil-works/pi/tree/936aff00918de1187f085f123c2812d8f2d67745) | [MIT](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/LICENSE#L1-L20) | 可见 thinking/opaque signature 分离、子进程 streaming 示例 |
| Tokio | [`ecd621dd2c1a5205a84f579225e1454b62af211c`](https://github.com/tokio-rs/tokio/tree/ecd621dd2c1a5205a84f579225e1454b62af211c) | [MIT](https://github.com/tokio-rs/tokio/blob/ecd621dd2c1a5205a84f579225e1454b62af211c/LICENSE#L1-L20) | blocking 隔离、bounded channel、watch、timeout 语义 |
| rusqlite | [`31ddc1c645cca1e5eecaa7cb859067acd08f12b5`](https://github.com/rusqlite/rusqlite/tree/31ddc1c645cca1e5eecaa7cb859067acd08f12b5) | [MIT-style](https://github.com/rusqlite/rusqlite/blob/31ddc1c645cca1e5eecaa7cb859067acd08f12b5/LICENSE#L1-L19) | `Connection` 的线程/互斥边界 |
| OpenPets | [`7b197e71757ad12a8d64028b9bd9734fe25a5e5b`](https://github.com/alvinunreal/openpets/tree/7b197e71757ad12a8d64028b9bd9734fe25a5e5b) | [MIT](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/LICENSE#L1-L20) | 桌面宠物单写者运动、边界、状态迟滞、reduced motion |
| PixiJS | [`1d90a20c62433ba68dff78466e06ee372a5a5232`](https://github.com/pixijs/pixijs/tree/1d90a20c62433ba68dff78466e06ee372a5a5232) | [MIT](https://github.com/pixijs/pixijs/blob/1d90a20c62433ba68dff78466e06ee372a5a5232/LICENSE#L1-L20) | 图片 decode、asset promise、elapsed-time sprite 动画 |
| W3C Media Queries Level 5 | [当前工作草案](https://www.w3.org/TR/mediaqueries-5/#prefers-reduced-motion) | W3C 文档/软件许可 | 系统 reduced-motion 偏好的语义 |

## 1. Visible reasoning capture 与 provider-native replay

### 1.1 一手资料事实

OpenAI Responses 官方文档给出了两个互相独立的面：

1. API 不提供原始 reasoning tokens；应用可请求 `reasoning.summary`，summary 位于一个独立的 reasoning output item，而最终答案位于 message item。[Reasoning summaries](https://developers.openai.com/api/docs/guides/reasoning#reasoning-summaries)
2. reasoning model 与工具一起使用时，后续请求必须保留 reasoning items 与工具调用/输出；在无服务端存储的模式下，可请求 `encrypted_content`，再把完整 output items 原样传回。[Keeping reasoning items in context](https://developers.openai.com/api/docs/guides/reasoning#keeping-reasoning-items-in-context)；[Preserve reasoning without stored responses](https://developers.openai.com/api/docs/guides/reasoning#preserve-reasoning-without-stored-responses)；[Manual conversation state](https://developers.openai.com/api/docs/guides/conversation-state#manually-manage-conversation-state)

因此，官方 Responses 里的“给用户看的 summary”和“给 provider 连续推理用的 opaque item”不是同一数据。前者可以展示，后者应按协议保真保存/回放，不应被前端编辑、压缩或从可见文本重建。

Pi 把同一边界落实成类型和转换代码：

- [`ThinkingContent`](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/types.ts#L338-L352) 分开保存可见的 `thinking` 与可选的 `thinkingSignature`；redacted thinking 的 opaque encrypted payload 也放在 signature 一侧。
- Responses 历史转换只有在 [`thinkingSignature` 存在时才恢复 reasoning item](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L211-L224)，不会把普通可见 thinking 文本冒充原生 replay payload。
- streaming 期间，Pi 为 thinking 建立独立 block，并把 reasoning summary/text delta 变成可见 thinking delta；结束时再单独序列化原生 reasoning item 作为 signature。[stream block](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L430-L472)；[delta mapping](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L595-L629)；[finalize visible text and signature](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L678-L688)

### 1.2 可借鉴到 Nexa 的契约

把 reasoning capability 明确拆成三个轴；不要再用一个枚举同时控制三者：

| 轴 | 问题 | 安全默认 |
| --- | --- | --- |
| `requestControl` | 当前 route 是否接受显式 reasoning 参数、参数名和取值 | 未知时不注入未经验证的控制参数；这不等于强制关闭 provider 默认 reasoning |
| `displayCapture` | 当前响应是否能解析 summary、structured thinking、兼容字段或显式 think block | 捕获实际返回且通过解析器验证的可见字段；缺失就是无可见内容 |
| `replayPolicy` | 下一请求能否原样回放当前 provider-native continuation payload | `Unknown` 默认不回放；要求原生 item 的工具续接缺 item 时 fail closed |

候选行为矩阵：

| Route/replay 状态 | 当前请求 | 当前响应展示 | 后续 replay / tool continuation |
| --- | --- | --- | --- |
| 已验证、原生 replay 可用 | 发送该 route 已知的控制参数 | 展示已解析的 visible reasoning | 只回放经过验证的原生 envelope |
| 已验证、不需要 replay | 正常请求 | 展示 | 不回放 reasoning |
| 自定义或未知兼容 route | 不注入未知控制参数，也不强制 `reasoning=false` | 若 provider 实际返回受支持字段则展示 | 省略未知 replay payload |
| 协议要求原生 item，但 item 缺失/route 改变 | 本轮已经产生的可见内容仍保留 | 展示 | 工具续接失败关闭，或从可信边界进行 answer-only 新采样；不得伪造 item |

对 Nexa 文件边界的直接建议：

- `crates/core/src/agent/model_step.rs`：生成请求与 tool continuation authorization 分成前后两个 gate；后者失败不能反向抹去当前 turn 已捕获的 thinking。
- `crates/core/src/llm/reasoning_profile.rs`：profile 应提供三个轴，不让 `Unknown` 自动等于 `reasoning_enabled = false`。
- `crates/core/src/llm/provider_turn.rs`：opaque payload 连同 route identity、schema version、sample identity 原子持久化，且只供后端 replay。
- `apps/desktop/src-tauri/src/agent_stream.rs`：前端投影继续只接收可见 thinking；provider-native envelope 必须从 frontend compaction 中剔除。

### 1.3 不应照搬或过度推断

- OpenAI 的 reasoning item 完整性是 **Responses API 的协议要求**，不能据此假定 DeepSeek、Qwen、MiniMax 或任意本地 OpenAI-compatible endpoint 使用同一种 item。
- Pi 的 `thinkingSignature: string` 是跨 provider 的便捷承载类型，不足以替代 Nexa 已有的 typed envelope、route snapshot、版本和原子持久化约束。
- `response.reasoning_text.delta` 的兼容处理不表示 OpenAI 官方会公开 raw chain-of-thought；官方文档承诺的是 reasoning summary。Nexa UI 应标注实际收到的语义，不能把 summary 宣称为“原始思维”。
- `<think>...</think>` 是某些兼容服务的文本约定，不是 OpenAI Responses 标准。只应由受限 parser 在当前响应展示侧处理，绝不能升级成可 replay 的可信原生 item。

### 1.4 必须覆盖的测试

1. `Unknown + tools`：provider 返回结构化 thinking 或兼容 `reasoning_content`，当前 turn 可见，下一请求不含未验证 payload。
2. 未知 route 不接收显式 reasoning 参数：请求不注入参数，但 provider 默认返回 thinking 时仍被捕获。
3. required native replay 缺失、损坏、schema 未知或 route identity 改变：当前 visible thinking 保留，工具不执行，错误可审计。
4. Responses stateless：完整 output item 顺序保留，encrypted reasoning 未经 JSON 重建；前端消息不包含 encrypted/native envelope。
5. 多种 visible 形状：summary item、structured block、`reasoning_content`、完整/分片 `<think>`；普通答案文本不能被误吞。
6. answer-only recovery：不得把 reasoning 文本复制进最终答案，也不得执行可能截断的工具参数。

## 2. Subagent 生命周期、事件桥和同步 SQLite 隔离

### 2.1 Codex 的生命周期边界

OpenAI Codex 的固定源码快照把创建、等待、打断和 registry 分离：

- [`spawn_agent` handler](https://github.com/openai/codex/blob/94937de51ba28d4b308dbe1b8472d6fe1dddad28/codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs#L39-L181) 创建 child、发出 `Started` activity，并把 task name/nickname 返回给调用方；工具本身不等待 child 的完整任务结果。
- 底层 spawn 先预留容量、创建 thread、登记父子关系、发初始输入，再返回 live handle；创建时还立即通知 client 新 thread 可订阅或 drain。[control spawn](https://github.com/openai/codex/blob/94937de51ba28d4b308dbe1b8472d6fe1dddad28/codex-rs/core/src/agent/control/spawn.rs#L400-L584)
- registry 的 capacity reservation 使用原子计数和 RAII rollback，创建中途失败不会泄露并发名额。[reservation](https://github.com/openai/codex/blob/94937de51ba28d4b308dbe1b8472d6fe1dddad28/codex-rs/core/src/agent/registry.rs#L278-L340)
- [`wait_agent`](https://github.com/openai/codex/blob/94937de51ba28d4b308dbe1b8472d6fe1dddad28/codex-rs/core/src/tools/handlers/multi_agents_v2/wait.rs#L39-L202) 有被 clamp 的独立 timeout，并等待 mailbox/activity/steer/timeout 之一；超时是一次观察结果，不等于 child 失败。
- [`interrupt_agent`](https://github.com/openai/codex/blob/94937de51ba28d4b308dbe1b8472d6fe1dddad28/codex-rs/core/src/tools/handlers/multi_agents_v2/interrupt_agent.rs#L26-L96) 是单独工具；control 还能读取状态、解析引用、获取 subtree 和订阅状态。[control operations](https://github.com/openai/codex/blob/94937de51ba28d4b308dbe1b8472d6fe1dddad28/codex-rs/core/src/agent/control.rs#L270-L382)
- status 是由 thread event 映射的显式状态，而不是由“多久没收到 UI delta”推断。[status mapping](https://github.com/openai/codex/blob/94937de51ba28d4b308dbe1b8472d6fe1dddad28/codex-rs/core/src/agent/status.rs#L4-L27)

Pi 的 subagent extension 提供另一组可借鉴机制：

- 独立进程通过 JSON lines 逐条输出，在 message/tool result 边界调用更新 callback；abort 时先终止，再在 5 秒后强制结束。[process and stream bridge](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/coding-agent/examples/extensions/subagent/index.ts#L333-L409)
- 并发 map 有上限，示例最多 8 个任务、4 个并发；parallel UI 保留每个 worker 的 running/done 占位并持续更新。[limits](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/coding-agent/examples/extensions/subagent/index.ts#L28-L38)；[concurrency](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/coding-agent/examples/extensions/subagent/index.ts#L219-L236)；[bounded progress view](https://github.com/earendil-works/pi/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/coding-agent/examples/extensions/subagent/index.ts#L584-L645)

### 2.2 推荐的 Nexa tool contract

`spawn_subagent` 的完成条件应是“child 已登记且可观察”，不是“child 已完成任务”。建议把工具面定义为：

| 工具 | 返回条件 | 最小返回值 |
| --- | --- | --- |
| `spawn_subagent` | durable run record、capacity lease、event stream 已建立，初始输入已受理 | `agentId`、`runId`、`state`、`eventCursor` |
| `observe_subagent` | 立即读取快照和 cursor 之后的可用事件，不等待 | snapshot、events、`nextCursor`、`truncatedBefore` |
| `wait_subagent` | 指定 agent 集合发生终态/新事件，或 bounded timeout | 每个 agent snapshot、events、`timedOut`；timeout 不是 error |
| `send_subagent_input` | 输入入队或明确拒绝 | message id、accepted state |
| `cancel_subagent` | cancellation request 被登记并广播 | previous/current state、scope、request id |
| `close_subagent` | child 已终态、资源释放并保留必要审计记录 | closed state；运行中默认拒绝 |

状态机至少区分：

```text
Created -> Queued -> Starting -> Running -> Completed
                                  |       -> Failed
                                  |       -> CancelRequested -> Cancelled
                                  |       -> Interrupted (可恢复时不是终态)
```

关键不变量：

- `agentId`/`runId` 一旦发布即稳定；创建失败必须回滚 capacity lease、registry edge 和未发布记录。
- `cancel` 是协作式请求，不等同于已经终止；UI 只有收到 durable `Cancelled`/`Failed`/`Completed` 才显示终态。
- parent 结束时是否级联取消必须是显式 policy；单 agent cancel 与 subtree cancel 不可混为一谈。
- `close` 只负责回收与隐藏，不得成为偷偷取消运行中 child 的别名。
- 任何工具调用的 timeout 都只约束这次等待，不改变 child 的事实状态。

### 2.3 实时事件桥与 bounded progress

状态快照和事件流的语义不同，应使用不同通道：

- 最新状态：`watch`/snapshot 语义，只需要最新值。Tokio `watch` 明确只保留最后一个值，适合 `Queued/Running/Completed`，不适合保存每个 token delta。[Tokio watch receiver](https://docs.rs/tokio/latest/tokio/sync/watch/struct.Receiver.html)
- 增量事件：bounded `mpsc` 或有容量的 durable ring/event log；Tokio bounded channel 在容量满时提供 backpressure。[Tokio bounded channel](https://docs.rs/tokio/latest/tokio/sync/mpsc/fn.channel.html)

候选父级事件：

```text
SubagentSpawned / Queued / Connected
SubagentThinkingDelta / OutputDelta
SubagentToolStarted / ToolProgress / ToolCompleted
SubagentStatusChanged
SubagentCompleted / Failed / Cancelled
```

建议的负载策略：

- thinking/output token delta 在 producer 侧按 agent 合并为 100–250 ms chunk；容量紧张时可以合并相邻同类 delta，但不能无声丢失文本。
- lifecycle、tool started/completed、error、usage final 不可丢；必要时先 flush 已合并 delta。
- 每个 event 有单调 `seq`；`observe` 返回 `nextCursor`。如果环形缓存覆盖旧事件，显式返回 `truncatedBefore`，前端改从 durable snapshot rehydrate。
- UI 活性与 worker 活性分开：渲染节流不能阻塞 durable event persistence，数据库慢也不能阻塞 heartbeat/status watch 的 poll。
- bounded progress 默认只展示每个 worker 的最近阶段和受限摘要；完整结果通过 artifact/durable output 查询，避免一个 child 把父上下文淹没。

### 2.4 Tokio 与 rusqlite 给出的隔离边界

Tokio 官方源码明确说明：

- [`spawn_blocking`](https://github.com/tokio-rs/tokio/blob/ecd621dd2c1a5205a84f579225e1454b62af211c/tokio/src/task/blocking.rs#L83-L135) 把 blocking 工作放到专用线程池，但任务一旦开始不能被普通 abort 停止；CPU-heavy 工作还需另行限制并发。
- 对长寿命、持续接收工作的同步组件，文档建议使用独立线程；同步线程可通过 `blocking_send`/`blocking_recv` 与 async 侧通信。[sync/async bridge](https://github.com/tokio-rs/tokio/blob/ecd621dd2c1a5205a84f579225e1454b62af211c/tokio/src/task/blocking.rs#L141-L168)
- 普通 `std::sync::Mutex` 只有在临界区不阻塞、也不跨 `.await` 时才适合 async 代码；共享 IO resource 通常更适合由单独 task/owner 加消息传递管理。[Tokio mutex guidance](https://github.com/tokio-rs/tokio/blob/ecd621dd2c1a5205a84f579225e1454b62af211c/tokio/src/sync/mutex.rs#L24-L50)
- `timeout` 会先 poll inner future；如果 inner future 不 yield，超时检查可能晚于 deadline。因此仅增加 watchdog 秒数无法修复 runtime starvation。[Tokio timeout](https://docs.rs/tokio/latest/tokio/time/fn.timeout.html)

rusqlite 的 `Connection` 内含 `RefCell`，实现 `Send` 但不是 `Sync`；默认 `NO_MUTEX`，即使 SQLite 使用 full mutex，也不使同一个 `Connection` 适合同时调用。[Connection internals](https://github.com/rusqlite/rusqlite/blob/31ddc1c645cca1e5eecaa7cb859067acd08f12b5/src/lib.rs#L362-L370)；[threading discussion](https://github.com/rusqlite/rusqlite/blob/31ddc1c645cca1e5eecaa7cb859067acd08f12b5/src/lib.rs#L1251-L1280)

对 Nexa 的直接推导：

- `crates/core/src/db_executor.rs` 的 reader/writer 专用线程方向符合 Tokio/rusqlite 边界；subagent 的 history、task、timeline、usage、launch metrics 和 terminal status 都应通过它进入同步 owner。
- channel 必须 bounded，并记录 `queue_wait` 与 `execution`；队列满应返回可分类的 backpressure 结果，不能在 Tokio worker 上改为阻塞发送。
- transaction/operation closure 应在专用线程内完整执行；async 侧不能拿出 `Connection` 或 guard。
- 短小、无法立即迁移的同步调用可暂放 `spawn_blocking`，但它不是可靠的 cancellation primitive，也不适合无限等待的全局锁。
- 数据库调用的可取消性应定义为“入队前可取消；开始后完成/回滚并丢弃过期结果”，而不是假设 abort 能中断 SQLite 中间步骤。

### 2.5 Watchdog/恢复契约

“前端一段时间没有收到认可的事件”只能产生 `suspected_stall`，不能直接产生 `Connection lost`：

```text
active
  -> suspected_stall
  -> query backend by turn/run handle
  -> replay events after durable cursor
  -> active | reconnecting | confirmed_failed | not_found
```

只有后端确认 stream 关闭且 run 不存在/已失败，或传输层给出确认错误，才显示断线。若后端仍为 `Running`，UI 应恢复 cursor、显示最近 durable progress，并允许用户 cancel。

### 2.6 不应照搬

- Pi 的 extension 虽然流式更新、进程隔离良好，但其 tool call 最终仍等待并发 worker 完成后返回；它不是 Nexa 所需的可长期 observe 的完整生命周期。只借鉴隔离、并发上限、输出上限和取消升级。
- Codex v2 的 `wait_agent` 固定快照主要等待 mailbox/activity，而不等同于“对任意 agent 集合 wait-all”。Nexa 必须自行规定 targets、cursor、any/all 和 timeout 语义。
- Codex status 将 `Interrupted` 视作可能继续的状态。Nexa 如果不支持 resume，就不应复制名称而不给恢复契约。
- 进程隔离不是唯一安全实现。Nexa 可以用 in-process task，但 registry、事件、DB owner、budget/capacity 与 cancellation 的隔离仍必须存在。

### 2.7 必须覆盖的测试

1. `spawn_subagent` 在 fake worker 被闸门阻塞时已经返回 handle；主 agent 随后仍能运行普通工具。
2. 三个 worker 独立产生 delta/tool/status；事件 seq 单调，UI 可持续观察，coalescing 不丢字符或生命周期边界。
3. channel 饱和：token delta 合并，terminal/tool/error 仍到达；报告 `truncatedBefore` 或 backpressure，而不是无声丢弃。
4. wait timeout：返回 `timedOut=true` 且 child 仍 `Running`；随后 observe/wait 能读到完成结果。
5. cancel：排队 child 不启动并释放 budget；运行中 child 进入 `CancelRequested` 后终态；需要级联时覆盖整个 subtree。
6. DB stress：多 child 并行 history/read/write 时 Tokio heartbeat、输入、滚动和 stop 可被调度；断言 SQLite closure 运行在线程名/owner lane 而非 Tokio worker。
7. 超过旧 120 秒阈值：后台 snapshot 仍 Running 时 UI 不写入 `Connection lost`；重载后从 durable cursor 恢复。
8. 故障注入：registry 创建、初始输入、DB persistence、provider connect 任一步失败都回滚 capacity/reservation，且留下可分类终态。

## 3. Companion 原子资源、动画与行为状态机

### 3.1 资源加载必须是一次候选事务

PixiJS 的 asset loader 体现了浏览器侧正确的 ready 边界：`Assets.load()` 返回包含已加载资源的 Promise，并使用缓存；图片 parser 在构造 texture 前等待 `createImageBitmap` 或 image load 完成。[Assets.load](https://github.com/pixijs/pixijs/blob/1d90a20c62433ba68dff78466e06ee372a5a5232/src/assets/Assets.ts#L513-L620)；[image decode path](https://github.com/pixijs/pixijs/blob/1d90a20c62433ba68dff78466e06ee372a5a5232/src/assets/loader/parsers/textures/loadTextures.ts#L56-L76)；[await decoded source](https://github.com/pixijs/pixijs/blob/1d90a20c62433ba68dff78466e06ee372a5a5232/src/assets/loader/parsers/textures/loadTextures.ts#L126-L168)

Nexa 不必引入 PixiJS，但应复制 ready 语义：

```text
Committed(old pack + old decoded asset)
  -> Loading(candidate generation N)
  -> Validating frame geometry
  -> Decoding image
  -> if N is still latest: one commit(candidate pack + decoded asset + first frame)
  -> otherwise discard candidate
```

实现不变量：

- pack 与 asset 是一个不可分割的 committed descriptor，不能先 `setPack(next)` 再让旧/空 asset 暂时配对。
- 切换期间旧 committed descriptor 继续渲染；失败时保留旧资源并报告错误，不渲染一帧 fallback。
- 使用 generation/abort guard；较慢的旧请求不得覆盖较新的选择。
- `rendererReady` 必须在首个 committed asset decode 且第一帧可画后发出。
- settings preview 和独立 Companion window 复用同一 loader/descriptor 语义，避免一处原子、一处仍闪烁。

对应 Nexa 风险点是 `CompanionWindowPage.tsx` 和 `CompanionSettingsCard.tsx` 的 asset/pack 独立 state；验收应直接观察 DOM mutation/截图序列，证明切换过程从不出现 fallback 或 pack/asset mismatch。

### 3.2 动画必须由 elapsed time 驱动

PixiJS 的 `AnimatedSprite` 在 play/stop 时注册/移除共享 ticker，并根据 ticker delta 或每帧 duration 推进，而不是假定每次回调都恰好过去一帧。[ticker lifecycle](https://github.com/pixijs/pixijs/blob/1d90a20c62433ba68dff78466e06ee372a5a5232/src/scene/sprite-animated/AnimatedSprite.ts#L496-L555)；[elapsed-time update](https://github.com/pixijs/pixijs/blob/1d90a20c62433ba68dff78466e06ee372a5a5232/src/scene/sprite-animated/AnimatedSprite.ts#L634-L721)；[requestAnimationFrame ticker](https://github.com/pixijs/pixijs/blob/1d90a20c62433ba68dff78466e06ee372a5a5232/src/ticker/Ticker.ts#L257-L287)

OpenPets 则把所有 pet 放在一个 shared ticker，只有存在 motion state 时运行，并在隐藏/拖拽期间避免竞争写位置。[shared ticker](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/apps/desktop/src/pet-motion-engine.ts#L98-L123)；[single-writer test](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/apps/desktop/tests/pet-motion-engine-single-writer.test.ts#L1-L113)；[ticker lifecycle test](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/apps/desktop/tests/pet-motion-engine-shared-ticker.test.ts#L49-L119)

推荐给 Nexa：

- 使用 `performance.now()` + `requestAnimationFrame` 或 CSS `steps()`；按 elapsed time 计算 logical frame，后台/卡顿恢复后跳到应处帧，不快速补播多个 React state 更新。
- animation key 未变时不重置 epoch；task projection 的同义状态映射到同一 key，避免每个 event 都回第 0 帧。
- 一次性动画有明确 `duration/onComplete/nextState`；looping 动画只在真正的 state transition 时换 epoch。
- 渲染帧和窗口位置分别只有一个 writer；drag 时 motion writer 暂停，drop 后从实际窗口位置重建 motion state。
- React state 不必随每一帧更新；CSS custom property、DOM style 或独立 ticker store 可降低整棵组件重渲染。

### 3.3 行为状态机与优先级

OpenPets 在 movement 方向变化时映射 run-left/run-right，停止后用约 180 ms 的 idle hysteresis，并只发送实际变化的 sprite state。[movement-state hysteresis](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/apps/desktop/src/pet-window.ts#L1488-L1527)

Nexa 的行为不能只是一组布尔配置。建议单一 reducer 持有：

```text
mode: Idle | Hovering | Petting | Dragging | Walking | TaskReaction | TerminalReaction | Sleeping
direction: Left | Right
enteredAt: monotonic timestamp
source: user | task | idleScheduler | boundary
generation: increasing integer
```

优先级从高到低：

1. `Dragging` / 用户显式输入；
2. click、pet、drop 等短暂交互动作；
3. success/failure/cancel terminal reaction；
4. 活跃 task reaction；
5. auto-walk；
6. idle gesture / sleeping。

每次抢占都定义恢复目标和最短停留时间。短暂 task event 使用 150–300 ms hysteresis；terminal reaction 播完再进入 idle，不能被同一 run 的迟到 progress 反向抢占。`idleActions=false` 关闭随机 gesture，`autoWalk=false` 关闭 scheduler/位移，但不应关闭 click/drag/accessibility 操作。

### 3.4 边界自动行走

OpenPets 的 motion controller 对窗口使用当前 display work area 进行 clamp，显示/注册 motion 时重新约束位置，销毁前先注销 motion。[controller lifecycle](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/apps/desktop/src/agent-pet-controller.ts#L22-L55)；[unregister before destroy](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/apps/desktop/src/agent-pet-controller.ts#L208-L215)

Nexa 的 auto-walk 最小契约：

- 路径在当前 monitor 的 `workArea` 内生成，扣除 sprite hitbox、shadow/bubble 安全边距；不使用包含任务栏的全屏 bounds。
- 每个 tick 先以 elapsed time 积分，再 clamp；触边时翻转方向并开启最短 cooldown，避免浮点抖动造成左右快速切换。
- display topology、DPI/scale 或窗口尺寸变化时取消旧目标，从实际位置重新规划。
- dragging/locked/click-through/hidden/reduced-motion 时停止 scheduler；停止后不得残留旧 timer 写位置。
- 随机性通过可注入 PRNG/clock 测试，避免 Playwright 依赖真实等待和不可复现路径。

### 3.5 Reduced motion

W3C 将 `prefers-reduced-motion: reduce` 定义为用户希望移除或替换可能引起不适/分心的运动，而不是简单把动画调慢。[Media Queries Level 5](https://www.w3.org/TR/mediaqueries-5/#prefers-reduced-motion)

OpenPets 在 renderer CSS 中显式关闭 sprite、bubble、status 等动画，并按 motion/state selector 生成 sprite state；它还用序号和串行 load chain 避免过期内容提交。[reduced-motion CSS](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/apps/desktop/src/pet-window.ts#L1288-L1305)；[serialized load generation](https://github.com/alvinunreal/openpets/blob/7b197e71757ad12a8d64028b9bd9734fe25a5e5b/apps/desktop/src/pet-window.ts#L1534-L1581)

Nexa 应把有效设置解析为：

```text
effectiveReducedMotion = appSetting || systemPrefersReducedMotion
```

除非产品明确提供“跟随系统/允许覆盖”的三态设置。reduce 模式应：

- 选择各状态的代表静帧；
- 禁止 auto-walk、bounce、bob、shake、flash 和自动视差；
- 保留 drag、click、键盘/菜单等直接操作，并用静态姿态、bubble 或颜色边界反馈；
- 资源切换仍保持原子；不能用 fallback 闪帧替代动画。

### 3.6 不应照搬

- OpenPets 在该快照仍使用 16 ms interval 做桌面窗口运动；它的价值是 shared ticker/single writer/lifecycle，而不是证明 interval 比 rAF 更适合 Nexa 的 WebView sprite。
- OpenPets 的 HTML/CSS content load chain 有 superseded guard，但并未完整证明“新图片 decode 后才原子替换旧 sprite”。Nexa 的 decode transaction 必须由 Pixi/浏览器 image decode 语义和本地测试共同保证。
- PixiJS 是完整 2D 引擎；Nexa 当前 CSS spritesheet 不需要为一个时钟/loader 契约引入整套引擎。
- reduced motion 不是“把 FPS 降低”。持续位移即使低 FPS 仍是运动；应替换为静态反馈。

### 3.7 必须覆盖的测试

1. 延迟旧 pack、快速选择新 pack：只提交新 generation；整个截图/DOM 序列从未出现 fallback 或旧 pack + 新 asset 组合。
2. decode reject：旧 committed pet 保持可见，错误可观测，`rendererReady` 不对失败 candidate 提前发出。
3. fake rAF/clock：1000 ms 卡顿后直接计算正确 logical frame，不产生大量补帧 render；同 animation key 的 projection 更新不归零。
4. hysteresis：150–300 ms 内的 transient state 不闪切；terminal reaction 不被迟到进度覆盖。
5. interaction reducer：click/pet/drag/drop 与 task reaction 的优先级、恢复目标和 cleanup 可重复验证。
6. auto-walk：左右边界翻转、任务栏 work area、多 monitor/DPI 重规划、drag 暂停、关闭设置后无残留写入。
7. reduced motion：系统 media query 或应用设置任一启用时固定代表帧、无自动位移；drag/click 仍可用。
8. Playwright 视觉边界：默认 sprite/window/hitbox 尺寸由一个 resolved size 计算，不再把放大的 bounding box 当作默认正确体验。

## 4. 建议的跨线验收门槛

三条线最终都需要验证“事实状态、投影状态、UI 状态”没有互相代替：

| 场景 | 事实状态 | UI 可显示 | UI 不得推断 |
| --- | --- | --- | --- |
| reasoning replay unknown | 当前响应可能有可见 thinking；未来 payload 不可信 | 当前已解析 thinking + replay omitted warning | 模型没有 reasoning 能力 |
| child 长时间运行 | backend snapshot 是 Running，cursor 可恢复 | running、最近进度、cancel | Connection lost |
| candidate pet 仍在 decode | old descriptor 仍 committed | 旧 pet 或明确 loading adornment | asset 不存在/fallback 是新状态 |

推荐 CI 分层：

1. Rust/TypeScript 单元测试验证上述不变量、state reducer、event cursor、DB thread ownership。
2. provider fixtures 验证请求 payload 与 replay payload，不能只断言 UI text。
3. Playwright 使用 fake clock、可控 image decode 和 fake backend run；另加至少一个真实 Tauri smoke 覆盖 IPC/event bridge。
4. 长时/并发测试观察 Tokio heartbeat 与 DB queue metrics；JS 定时发送假 heartbeat 不能作为 subagent runtime 的充分 E2E。
5. P1/P0 回归审查只接受可复现测试或真实日志证据；延长 timeout、增加 provider 白名单、缩小 CSS scale 都不能单独关闭问题。

## 5. 对当前实现工作的最短检查清单

- [ ] `model_step.rs` 不再因 `Unknown + tools` 在采样前关闭 reasoning；tool authorization 仍 fail closed。
- [ ] visible thinking 与 provider-native envelope 分别测试，frontend compaction 永远看不到 envelope。
- [ ] 单 agent `spawn` 返回 handle 而不是最终 artifact；observe/wait/cancel/close 有独立契约。
- [ ] event bridge bounded、带 cursor，status snapshot 与 delta log 分流。
- [ ] subagent 路径所有 SQLite closure 都在 `DatabaseExecutor` owner thread；队列满和执行时长可观测。
- [ ] watchdog 先向 backend 对账并 rehydrate，再决定 disconnected。
- [ ] pack/asset/decode 一次提交；settings preview 与 pet window 使用相同 loader。
- [ ] 动画按 elapsed time，状态变化有 hysteresis；行为由一个 reducer/单写者管理。
- [ ] auto-walk 有 work-area 边界、drag/hidden/reduced-motion cleanup。
- [ ] reduced motion 选择静帧并去掉自动位移，但直接交互仍可用。
