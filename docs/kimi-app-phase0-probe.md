# Kimi App × Spur — Phase 0 探针报告

**日期**: 2026-07-21  
**目标客户端**: Kimi Desktop GUI `3.1.3` (`com.moonshot.kimichat`)  
**约束**: 仅写用户目录，不改 `Kimi.app` / asar  

## 结论摘要

| 探针项 | 结果 | 说明 |
|---|---|---|
| 模型列表数据源 | **云端优先** | `KimiWorkModelSync` → `GetAvailableModels`；成功时 `updateKimiModelList`，`fromCache=false` |
| 本地 cache | **仅失败回退** | `kimi-work-models-cache.json` 在 `describeKimiWorkConfig` 失败时才使用 |
| 正式包 baseUrl | **写死官方** | `resolveWorkGatewayBaseUrl()` 直接返回 `https://agent-gw.kimi.com/coding/v1` |
| 启动时凭据回写 | **会覆盖官方 provider** | 每次 ready 写 `credentials.kimiCode.baseUrl` 与官方 `daimon-kimi-code` |
| 协议面 | **可 mock** | agent-gw 兼容 `POST …/v1/chat/completions` 与 `POST …/v1/messages` |
| daimon 控制面 | **可二次注入** | `ws://127.0.0.1:<port>/control`，token 在 `runner.state.json`；支持 `conversations.updateKimiModelList` |
| Kill criteria | **未全部通过** | 在线时官方列表会覆盖；官方 baseUrl 无法稳定改写。**但**自定义 provider + control RPC 合并仍有实验路径 |

**产品语义（强制）**: 本集成为 **实验性**。日常稳定多供应商仍以 **Spur → Codex** 为准。不得宣称 Kimi App 右下角在线长期稳定。

## 本地路径（本机实测）

```
~/Library/Application Support/kimi-desktop/
  kimi-agent/kimi-work-models-cache.json
  daimon-share/daimon/config.json
  daimon-share/daimon/runtime/kimi-code/config.toml
  daimon-share/daimon/agents/main/runner.state.json   # control WS endpoint + token
```

## 模型 cache 结构（摘录字段）

每个模型项关键字段：`key` / `displayName` / `modelId` / `modelAlias` / `daimonModelConfig`  
`daimonModelConfig` 含：`maxContextSize`、`capabilities`、`supportEfforts`、`defaultEffort`。

当前官方模型示例：`k3-agent`、`k3-agent-swarm`、`k2d6-agent`。

## 运行时配置

`config.toml` / `config.json` 中官方 provider：

- `daimon-kimi-code`：`type=kimi`，`base_url=https://agent-gw.kimi.com/coding/v1`
- `daimon-kimi-messages`：`type=anthropic`，`base_url=https://agent-gw.kimi.com/coding`

daimon 进程环境（正式包）：

- `KIMI_BASE_URL=https://agent-gw.kimi.com/coding/`
- `DAIMON_KIMI_MESSAGES_BASE_URL=https://agent-gw.kimi.com/coding`
- `AGENT_GW_MCP_URL=https://agent-gw.kimi.com/coding/v1/mcp/`

## 协议（agent_gw 0.2.6 + 源码字符串）

- `POST /v1/chat/completions`（OpenAI 兼容，含 stream SSE）
- `POST /v1/messages`（Anthropic 兼容）
- 鉴权：`Authorization: Bearer <sk-kimi-…>`（或 Spur 本地 token）
- 可选头：`X-Kimi-Chat-Id`、设备相关 `X-Msh-*`

## 云端 sync 行为（asar 反编译字符串）

1. `getKimiWorkAvailableModels()` 调云端 ConfigService  
2. 失败且 cache 非空 → `fromCache=true`  
3. 成功 → `conversations.updateKimiModelList({ models: expected })`，**只含云端返回的 alias**  
4. 日志样例：`sync(daimon-ready): … listChanged=true fromCache=false`

→ **仅改 cache 在线几乎无效**；需要在 sync 之后用 control RPC 再合并 Spur 模型，或接受仅离线/短时。

## baseUrl 结论

```js
function resolveWorkGatewayBaseUrl(debugEnvStore) {
  return DEFAULT_WORK_GATEWAY_BASE_URL; // 官方 prod
}
```

正式包无法通过 debug 环境面板稳定改 baseUrl（函数忽略参数）。  
**可行绕过**：不为官方 provider 改 URL，而是 **新增** `spur-gateway` provider（`type=kimi`）指向 `http://127.0.0.1:<spur-port>/coding/v1`，模型条目绑定该 provider。

## 探针通过标准对照

| 标准 | 状态 |
|---|---|
| ≥1 个非官方模型名出现在右下角 | **待 E2E**（依赖 publish + updateKimiModelList；代码已实现实验路径） |
| 一轮 agent 经本地 mock 成功 | **待 E2E**（网关已挂 `/coding/v1/chat/completions`） |
| 在线 30s 内不被清掉 | **默认不满足**；需 re-apply / 守护（可选，默认关闭） |

## 推荐落地策略（本仓库已按此实现骨架）

1. Spur 发布器只写用户目录；备份可恢复  
2. 注入 `spur-gateway` + 每路由一个 `models.<alias>`  
3. 合并 cache；通过 daimon control WS 调用 `updateKimiModelList`  
4. 本地 gateway 接 chat/completions → 现有 Spur 上游适配  
5. UI 文案标明实验性；与 Codex Apply 分离  

## 实现进度（相对计划）

| 阶段 | 状态 | 产物 |
|---|---|---|
| Phase 0 探针 | **完成（降级实验语义）** | 本文档；kill criteria 未全过 → 不承诺在线长期稳定 |
| Phase 1 发布器 | **代码完成** | `src-tauri/src/kimi_target.rs`；Tauri：`kimi_target_status` / `preview_kimi_publish` / `apply_kimi_publish` / `restore_kimi_publish` / `reapply_kimi_model_list` |
| Phase 1 GUI | **代码完成** | 设置页「发布到 Kimi App（实验）」 |
| Phase 2 兼容网关 | **最小面完成** | `POST /coding/v1/chat/completions`（+ 双 `/v1` 兼容路径）、`GET /coding/healthz`；alias→route 映射；Chat Completions 上游转发 |
| Phase 2 完整协议 | **未完成** | Anthropic `/messages`、thinking/effort 阶梯、Responses 专用上游（ChatGPT backend）适配、历史消毒复用 |
| Phase 3 抗覆盖 | **方案 B** | 启用发布 = **仅写盘**；禁止系统代理/整站 www.kimi.com。可选 path 拦见 `docs/kimi-app-selective-block.md` |
| Phase 4 E2E | **待真机** | 发布 → 路径拦截 → 冷启动 → 右下角出现 spur → 一轮对话 |

## 安全注意

- 探针过程读取了含 `sk-kimi-*` / JWT 的本地配置；**文档与日志已脱敏**  
- Spur 写入 Kimi 目录时 **只放本地 gateway token**，不写上游 refresh token  

