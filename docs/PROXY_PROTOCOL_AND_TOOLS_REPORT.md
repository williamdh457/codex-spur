# Codex Spur 代理协议与工具转换报告

**文档版本**：与仓库 `main` @ `b0b7043`（2026-08-01）对齐  
**范围**：Responses 透传、Responses↔Chat Completions 桥、工具类型转换、历史 sanitization、freeform 往返  
**产品前提**：Codex App（Desktop GUI）始终以 **OpenAI Responses** 调用 Spur localhost 代理；Spur 再按供应商车道转发上游。

---

## 1. 产品目标（读这份报告前必读）

Spur **不是** 把 Grok/Kimi 变成 GPT，也 **不是** 修改 `ChatGPT.app`。

目标是：

1. 用户在 **Codex 官方 App** 的 model picker 里选 Spur 路由；
2. App 侧协议与工具生命周期尽量符合 **官方 Desktop 契约**（尤其 freeform `apply_patch`）；
3. 上游侧按供应商能力做 **最小必要适配**，避免 422 / 静默丢历史 / Desktop abort。

因此「转换」分两层：

| 层 | 对象 | 目的 |
|----|------|------|
| **Desktop 契约层** | Codex App | 入站必须恢复官方 item 形状；否则 freeform 执行器 abort |
| **上游方言层** | xAI / Kimi / DeepSeek… | 出站改写成对方能吃的 tools / input / messages |

---

## 2. 总架构：上游车道（Lanes）

实现：`providers::UpstreamProtocolLane` + `proxy` 分发分支。

```
Codex App  ──POST /v1/responses──►  Spur 本地代理
                                      │
          ┌───────────────┬───────────┼────────────┬────────────────┐
          ▼               ▼           ▼            ▼                │
   OpenAiOfficial  ResponsesNative  ChatBridge  AnthropicMessages   │
   (官方 OpenAI)   (Responses 透传)  (CCS 式桥)   (OpenCode Go)       │
          │               │           │            │                │
          ▼               ▼           ▼            ▼                │
    OpenAI 后端    xAI/DS Flash/    Kimi/…/      MiniMax/Qwen        │
    /responses     Go Luna         /chat/…      Go /messages        │
          │               │           │            │                │
          └──────── 回程统一成 Responses 形状 ───────┘                │
                                      │
                                      ▼
                                 Codex App
```

### 2.1 车道判定规则

| kind / 条件 | 车道 |
|-------------|------|
| `openai` | **OpenAiOfficial** |
| `xai`（含 OAuth CLI proxy / API Key） | **ResponsesNative**（强制，忽略错误 protocol） |
| `deepseek` + V4 Flash/Pro 等原生模型 | **ResponsesNative** |
| `deepseek` + 旧 chat 模型 id | **ChatCompletionsBridge** |
| `kimi` / `minimax`（独立供应商） | **ChatCompletionsBridge** |
| `opencode-go` + `gpt-5.6-luna` | **ResponsesNative** → `/responses` |
| `opencode-go` + MiniMax / Qwen | **AnthropicMessagesBridge** → `/messages` |
| `opencode-go` + 其它模型（含 Go 上的 DeepSeek/Grok/Kimi…） | **ChatCompletionsBridge** → `/chat/completions` |
| `custom` 且 protocol 含 `response` | **ResponsesNative** |
| `custom` 其它 / protocol 含 `chat` / 默认 | **ChatCompletionsBridge** |

OpenCode Go 模型分流实现：`opencode_go::wire_protocol_for_model`。

### 2.2 请求入口公共步骤

1. 按 catalog slug 解析 `RouteTarget`（provider、upstream model、base_url、kind）。
2. 重写 body `model` → 上游真实 model id。
3. **Remote Compact V2**：
   - 默认：所有路由走本地 `spur1:` portable compact；
   - 例外：OpenAI + 用户开启 cloud compact → 透传官方密文 compact。
4. 按车道：
   - Chat 桥 → `forward_chat_compatible`；
   - Anthropic 桥 → `forward_anthropic_messages_compatible`；
   - 否则 → `map_reasoning` + `sanitize_responses_request_for_upstream` + `forward_responses_compatible`。
5. 媒体：部分 kind 剥离图片。

---

## 3. 车道 A：OpenAI Official（Responses 近透传）

### 3.1 做什么

- **协议**：继续 `/responses`（ChatGPT Codex / API）。
- **tools[]**：**不 port**（`keeps_codex_native_tools`）——保留 Desktop freeform / namespace 等 Codex-native 形状。
- **input[]**：只做 **跨供应商毒化清理**（`sanitize_openai_responses_input`），不是工具方言改写。

### 3.2 input 清理（OpenAI 专用）

| 操作 | 原因 |
|------|------|
| message id 重写为 `msg…` 前缀 | 混过 Kimi/Spur 的 `resp_…_msg` 会被 OpenAI 拒 |
| 丢弃 replayed `reasoning` | 外厂 encrypted 不可解密；summary-only 在 store=false 也有问题 |
| 历史 compact：portable→明文 user；外厂密文 drop | OpenAI 解不了 spur1/外厂 blob 的语义不同 |
| 保留 live `compaction` 控制载体 | Remote Compact V2 |
| `store≠true` 时丢 `item_reference` | 离线不可解析 |
| 有 drop 则 strip `previous_response_id` | 避免粘错上游 state |

### 3.3 工具

- **出站**：不转换 `custom` / freeform tools。
- **入站**：一般已是 Desktop 形；若中间夹了 function 形 freeform，JSON/SSE restorer 仍可 normalize。

### 3.4 与「官方体验」关系

这是 **最接近 OpenAI 官方后端** 的车道。  
仍不是 100% 原样：跨模型线程 sanitize、默认 portable compact 策略可能改 history。

---

## 4. 车道 B：Responses Native（透传 + 形状 port）

**适用**：xAI/Grok、DeepSeek V4 Flash/Pro、custom+Responses 等。

### 4.1 协议

- **始终** `POST {base}/v1/responses`（或等价路径）。
- **不做** `input[]` → `messages[]` 协议桥。
- Grok OAuth 订阅：`cli-chat-proxy.grok.com` + CLI 身份头；API Key：`api.x.ai`。  
  host 名含 “chat” **不等于** Chat Completions。

### 4.2 出站 sanitization 流水线

`sanitize_responses_request_for_upstream(kind, request)`：

1. **`sanitize_responses_tools_for_upstream`** — tools[] port + ensure apply_patch  
2. **`sanitize_responses_tool_choice_for_upstream`** — 非法/悬空 tool_choice drop  
3. **`sanitize_responses_input_for_upstream`** — history allow-list + freeform 改写  
4. **`strip_unsupported_responses_fields`** — 如 `prompt_cache_retention`、`safety_identifier`、`external_web_access`  
5. **`clamp_responses_fields_for_kind`** — 如 xAI grok-4.5 去掉 penalty/stop；composer 去 reasoning 等  

### 4.3 tools[] 允许类型

**xAI：**

```text
function, web_search, x_search, image_generation, collections_search,
file_search, code_execution, code_interpreter, mcp, shell
```

**通用非 OpenAI Responses：**

```text
function, web_search, file_search, code_interpreter, code_execution, mcp, shell
```

### 4.4 tools[] 转换规则（`port_codex_tools`）

| Desktop / Codex 形状 | 出站结果 |
|----------------------|----------|
| freeform `apply_patch` / `exec`（`type=custom` 或无 type） | `type=function` + `parameters.properties.input` |
| 已有 nested `function` 对象 | 尽量保留 |
| 已是 flat `type=function` + name/parameters | 保留 |
| `local_shell` | → `shell`（若 allow 含 shell） |
| `namespace` + 嵌套 tools | 展平后递归 port |
| 其它 `custom` / 未知 type | **丢弃**（fail closed，防 422） |
| 空 tools 或缺少 apply_patch | **注入** portable `apply_patch` function |

说明：Desktop 的 description 非空时优先保留，不覆盖官方文案。

### 4.5 input[] 历史转换（关键，Grok 曾踩坑）

**可移植 allow-list：**

```text
message | function_call | function_call_output
(+ 空 type 但有 role 的 message 形)
```

| 项 | 行为 |
|----|------|
| `custom_tool_call` | → `function_call`，`input` → `arguments: {"input":"…"}` |
| `custom_tool_call_output` | → `function_call_output` |
| `reasoning` / `additional_tools` | **丢弃** |
| 任意带 `encrypted_content` 的未知载体 | **丢弃** |
| `item_reference` | **丢弃** |
| 历史 `compaction` | portable 解码→user text；外厂密文→说明 note |
| live `compaction`（末条控制载体） | **保留** |
| 其它未知 type | **丢弃** |
| 发生 drop/rewrite | strip `previous_response_id` |

> **曾出的 P0**：allow-list 未含 `custom_tool_call`，整段 apply_patch 历史被静默丢掉 → Grok 跨轮断档 / 重试环。  
> **现状（b0b7043）**：CCS 式 Responses 内改写，**不**掉到 Chat 桥。

### 4.6 入站（上游 → Desktop）

| 路径 | 机制 |
|------|------|
| Responses JSON body | `restore_freeform_in_responses_body` |
| Responses SSE 流式 | `FreeformSseRestorer`：`function_call` 生命周期 → `custom_tool_call` + `custom_tool_call_input.*` |
| apply_patch 正文 | `normalize_apply_patch_input`：去掉 `*** Begin Patch ***` 尾星、修路径粘连、丢发明标记等 |

**硬契约**：freeform 回 Desktop 必须是 `custom_tool_call` + freeform `input`，否则官方执行器 **abort**。

### 4.7 与「官方体验」关系

- **协议面**：与 Codex App 一致（Responses）。  
- **工具形状**：上游侧故意 **不像** OpenAI freeform；回程再恢复 Desktop 官方形。  
- 用户感知：仍在官方 App 里 apply_patch / 多轮 agent。

---

## 5. 车道 C：Chat Completions Bridge（Responses → Chat → Responses）

**适用**：Kimi、独立 MiniMax、OpenCode Go 的 Chat 族模型、多数 custom、旧 DeepSeek chat。

参考行为：CC Switch `openai_chat` / Nice Switch transform，**独立实现**（不拷 LGPL/源码）。

### 5.1 出站：Responses → Chat Completions

入口：`responses_to_chat_completions` → `POST …/chat/completions`。

#### 5.1.1 messages 构建（`response_input_to_messages`）

| Responses input item | Chat messages |
|----------------------|---------------|
| `message` (user/assistant/system/developer) | 对应 role；developer→system；system 再 **collapse 到 messages[0]**（MiniMax 等只允许首条 system） |
| `function_call` | 并入 assistant `tool_calls[]` |
| `function_call_output` | `role=tool` + `tool_call_id` |
| `custom_tool_call` | 同上 tool_calls；freeform `input` → JSON `{"input":"…"}` |
| `custom_tool_call_output` | 同 function_call_output → role=tool |
| `reasoning` / `web_search_call` / `item_reference` | **跳过** |
| 历史 compact | 解码或 opaque note → user text |

助手轮合并规则（避免坏序）：

- 支持 `message → function_call → output` 与 `function_call → message → output`；
- 连续 tool 批处理，text 与 tools 合成一条 assistant。

#### 5.1.2 tools 构建（`responses_tools_to_chat_tools`）

1. `port_codex_tools(…, ["function"])` —— Chat **只认 function**。  
2. flat function → nested：

```json
{ "type": "function", "function": { "name", "description", "parameters" } }
```

3. `ensure_chat_apply_patch_tool` —— Desktop 漏发 freeform 时注入 apply_patch。

#### 5.1.3 其它映射

- stream / stream_options usage  
- temperature、top_p、max_tokens… 透传常见 knobs  
- reasoning effort → 供应商 profile patch（Chat 字段）  
- tool_choice：指向已删工具/namespace 则 drop  

### 5.2 入站：Chat Completions → Responses

| 上游形态 | Desktop 形态 |
|----------|--------------|
| JSON `choices[].message.tool_calls` | `output[]` 中 `function_call` 或 freeform→`custom_tool_call` |
| SSE `delta.tool_calls` 流式 | 组装后发 Responses SSE 生命周期 |
| 文本 content | `message` + `output_text` |
| freeform 名 `apply_patch`/`exec` | **必须** `custom_tool_call` + 抽出的 freeform `input`（并 normalize patch） |

实现要点：

- `chat_parts_to_responses_output` / `chat_parsed_to_responses_sse`  
- 使用 `tool_roundtrip::desktop_tool_call_item` 统一 Desktop 形状  
- 禁止「reasoning-only 且无 message/无 tools」被当成成功空回合（避免 Desktop 静默收工）

### 5.3 往返示意（Kimi 改文件）

```
Desktop ──Responses──► Spur
  tools: custom apply_patch
  input: … custom_tool_call …

Spur ──Chat Completions──► Kimi
  tools: [{type:function, function:{name:apply_patch, parameters:{input}}}]
  messages: assistant.tool_calls + role=tool

Kimi ──tool_calls──► Spur
  function.apply_patch arguments={"input":"*** Begin Patch ***…"}

Spur ──Responses SSE──► Desktop
  custom_tool_call + input 已 normalize 成 *** Begin Patch
  Desktop 执行 patch → 下一轮 custom_tool_call_output 再桥回去
```

---

## 6. 工具类型总表（Desktop ↔ 上游）

### 6.1 Desktop 调用形状注册表（`tool_roundtrip`）

| 名称 | Desktop 形状 | freeform |
|------|--------------|----------|
| `apply_patch` | `custom_tool_call` + `input` | 是 |
| `exec`（legacy freeform） | `custom_tool_call` + `input` | 是 |
| `exec_command` / `write_stdin` / `wait` / plan / MCP / computer-use… | `function_call` + JSON `arguments` | 否 |
| 未注册名 | 默认 `function_call` | 否 |

Freeform 集合在有新的 gold sample 前 **严格只有** `{apply_patch, exec}`。

### 6.2 tools[] 广告层（catalog 行 → 上游 tools 行）

| 来源 type | OpenAI 车道 | Responses native | Chat 桥 |
|-----------|-------------|------------------|---------|
| freeform/custom apply_patch|exec | 原样 | → function(+input schema) | → nested function |
| function（flat/nested） | 原样 | 保留若在 allow-list | nested function |
| local_shell | 原样 | → shell | 丢弃（非 function） |
| namespace | 原样 | 展平 | 展平后仅 function |
| web_search 等 hosted | 原样 | 保留若 allow | 丢弃 |
| 未知 custom | 原样 | 丢弃 | 丢弃 |

### 6.3 input[] / output[] 历史与流式

| Desktop / 中间形状 | → Responses native 上游 | → Chat 上游 | ← 回 Desktop |
|--------------------|-------------------------|-------------|--------------|
| `custom_tool_call` | `function_call` + JSON args | assistant.tool_calls | 保持/恢复 custom |
| `custom_tool_call_output` | `function_call_output` | role=tool | 保持 |
| `function_call`（非 freeform） | 原样 | tool_calls | 原样 function |
| `function_call`（freeform 名） | 原样（若历史已是） | tool_calls | **restore → custom** |
| SSE function args 流 | 上游发 function 事件 | Chat delta | **改写成 custom_tool_call_input.*** |

### 6.4 apply_patch 正文方言（全入站 freeform 路径）

Desktop 校验：**第一行必须精确** `*** Begin Patch`（无尾部 stars）。

normalize 处理（非穷尽）：

- `*** Begin Patch ***` / `***Begin Patch***` → 标准 begin  
- 路径粘连 `file.ts***`  
- 发明标记如 `*** End of File ***` 丢弃  
- 双层 JSON 字符串壳 unwrap  

这是 **Desktop 官方 executor 体验**，不是上游官方方言。

---

## 7. Compact 与工具历史

| 策略 | 行为 |
|------|------|
| 默认 local compact | 当前模型摘要 → `spur1:` envelope；**所有 kind**（含 OpenAI）可读 |
| OpenAI + sticky cloud compact | 透传官方 Remote Compact V2 密文 |
| 摘要 transcript | 含 message + function/custom tool 轨迹 |

注意：compact 在 sanitization **之前**跑，故 transcript **必须直接认识** `custom_tool_call`（b0b7043 已修；此前 freeform 轨迹会从摘要消失）。

---

## 8. 跨供应商同线程（P0，与工具并行的 sanitization）

Codex App 同线程切换模型会 **整段重放 input[]**。已知失败：

| 场景 | 典型错误 | Spur 对策 |
|------|----------|-----------|
| Kimi 后 OpenAI | message id 须 `msg*` | OpenAI 车道 rewrite id |
| GPT 后 Grok | 无法解密 encrypted_content | 双方向 drop reasoning/密文 |
| 代理 401/断流 | UI「已处理」假成功 | 产品侧错误暴露（独立问题） |

**工具转换不能单独解决跨供应商语义**；history sanitize + 产品「换厂商开新线程」策略共同负责。

---

## 9. 供应商对照矩阵（落地）

| 供应商 | 车道 | 协议路径 | tools port | history freeform rewrite | inbound freeform restore | 备注 |
|--------|------|----------|------------|--------------------------|--------------------------|------|
| OpenAI 官方/JSON/API | Official | /responses | 否（native） | 否（保留 custom） | 可选 normalize | 最近官方 |
| Grok API Key | Native | api.x.ai /responses | 是 | 是 | 是 | |
| Grok OAuth 订阅 | Native | cli-chat-proxy /responses | 是 | 是 | 是 | CLI 身份头 |
| DeepSeek Flash/Pro | Native | /responses | 是 | 是 | 是 | 类官方 DS 脚本 |
| DeepSeek 旧 chat | Chat 桥 | /chat/completions | nested function | 桥内 map | 是 | |
| Kimi | Chat 桥 | /chat/completions | nested function | 桥内 map | 是 | system collapse |
| MiniMax | Chat 桥 | /chat/completions | nested function | 桥内 map | 是 | 独立 MiniMax API；官方亦有 Responses；Spur 仍硬编码桥 |
| OpenCode Go（默认） | Chat 桥 | /chat/completions | nested function | 桥内 map | 是 | Grok/GLM/Kimi/DeepSeek/MiMo/Hy3 |
| OpenCode Go Luna | Native | /responses | 是 | 否（keep freeform） | 是 | `gpt-5.6-luna` |
| OpenCode Go MiniMax/Qwen | Anthropic 桥 | /messages | input_schema tools | 桥内 map | 是 | Bearer + x-api-key |
| custom + Responses | Native | /responses | 是 | 是 | 是 | |
| custom 默认 | Chat 桥 | /chat/completions | nested function | 桥内 map | 是 | |

---

## 10. 关键代码地图

| 主题 | 位置 |
|------|------|
| 车道枚举与路由 | `src-tauri/src/providers.rs` → `UpstreamProtocolLane`, `upstream_protocol_lane` |
| OpenCode Go 模型分流 | `src-tauri/src/opencode_go.rs` → `wire_protocol_for_model` |
| 请求分发 | `src-tauri/src/proxy.rs` → responses handler 内 compact / chat / anthropic / responses 分支 |
| Responses 转发 | `forward_responses_compatible` |
| Chat 桥转发 | `forward_chat_compatible`, `responses_to_chat_completions` |
| Anthropic 桥转发 | `forward_anthropic_messages_compatible`, `responses_to_anthropic_messages` |
| tools port | `port_codex_tools`, `sanitize_responses_tools_for_upstream`, `responses_tools_to_chat_tools` |
| input sanitize (non-OpenAI) | `sanitize_responses_input_for_upstream` |
| input sanitize (OpenAI) | `sanitize_openai_responses_input` |
| freeform 注册与往返 | `src-tauri/src/tool_roundtrip.rs` |
| custom→function 历史 | `custom_tool_call_to_function_call`, `custom_tool_call_output_to_function_call_output` |
| Responses SSE freeform | `FreeformSseRestorer` |
| Chat→Responses SSE | `chat_parsed_to_responses_sse` 等 |
| Compact transcript | `compact_shim::portable_transcript_for_compact` |
| Grok OAuth | `xai_oauth.rs`；base 解析 `resolve_xai_upstream_base` |

---

## 11. 已验证与残留风险

### 11.1 已有单测覆盖（代表）

- xAI：namespace drop、encrypted reasoning drop、**custom_tool_call 历史 rewrite**  
- freeform restore：JSON / SSE / Chat inbound  
- apply_patch 方言 normalize  
- Chat history：apply_patch / exec JSON wrap  
- compact transcript 含 freeform  
- 三车道路由（含 Grok 强制 Native）

### 11.2 残留风险（报告诚实段）

| 项 | 说明 |
|----|------|
| MiniMax 硬编码 Chat 桥 | 与官方 Responses 能力不完全对齐；属产品路由 |
| 仅 ensure 注入 apply_patch | 不自动注入 freeform `exec` |
| 未知 custom 工具名 | Responses/Chat 出站 fail-closed 丢弃 |
| SSE item_id 不对齐 | 依赖 done 事件兜底；极端网关可能缺 input 流式事件 |
| 未默认跑真实配额 smoke | 单测绿 ≠ 线上 Grok/Kimi 配额路径 |
| 跨供应商同线程 | 工具齐了也不等于语义安全 |

---

## 12. 相关提交（工具/协议主线）

| Commit | 主题 |
|--------|------|
| `ebcee2c` | apply_patch freeform roundtrip + DeepSeek native Responses |
| `a173622` | mid-thread policy + OpenAI cloud compact 选项 |
| `16e0516` | third-party apply_patch 方言 normalize |
| `253e253` | Chat vs Responses 路由对齐 CCS |
| `648873a` | 显式三车道 |
| `ade7fe2` | Grok 强制 Responses 文档/保证 |
| `b0b7043` | CCS 式 Responses freeform 历史 rewrite + compact/Chat exec 补洞 |

---

## 13. 一句话总结

```text
Codex App 永远说 Responses。

  OpenAI  → 近透传 + 毒化清理
  Grok/DS Flash/… → Responses 透传 + tools/history 形状 port + 回程恢复 Desktop freeform
  Kimi/MiniMax/… → 完整 Responses↔Chat Completions 桥 + 同样的 freeform 往返契约

工具转换的北极星只有一条：
  出站让上游不 422、不丢历史；
  入站让 Desktop 看到官方 custom_tool_call，apply_patch 能执行。
```

---

*本报告描述实现现状，不构成对第三方服务条款或稳定性的保证。真实上游行为以厂商文档与在线契约为准。*
