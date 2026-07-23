# Kimi App：方案 B（写盘 + 可选路径拦截）

## 原则

| 做 | 不做 |
|----|------|
| Spur **启用发布** 只写用户目录 | **不**改系统 HTTP/HTTPS 代理 |
| 本地 `127.0.0.1:<port>/coding/v1` 网关 | **不**整站拦 `www.kimi.com`（会弄挂 Kimi） |
| 可选：只拦 `DescribeKimiWorkConfig` | 不改 `Kimi.app` / asar |

日常稳定多供应商：**Spur → Codex Review & Apply**。

---

## 急救：Kimi 显示「请检查网络连接」

旧版误把系统代理指到 Spur 整站拦截端口。

1. **系统设置 → 网络 → Wi‑Fi → 详细信息 → 代理** → 关闭所有代理  
2. Spur 点 **关闭发布**（或重启 Spur，启动时会尝试清残留代理）  
3. 完全退出并重开 Kimi  

---

## 启用发布（只写盘）

1. Spur 运行中，模型页勾选要发布的模型  
2. 设置 → **启用发布**  
3. 状态应显示「已启用（仅写盘）」  
4. Kimi **应能正常打开**（不依赖拦截）  

此时配置/cache 已有 spur 条目；**在线时右下角仍可能只有官方列表**（云端列表优先）。

---

## 可选：右下角要出现 Spur 模型

只拦这一条 path（不要拦整站）：

```text
https://www.kimi.com/apiv2/kimi.gateway.config.v1.ConfigService/DescribeKimiWorkConfig
```

### Proxyman（推荐）

- 仅监控 **Kimi.app**  
- Block path 包含 `DescribeKimiWorkConfig`  
- **不要** block `agent-gw.kimi.com`  
- **不要** 全局系统代理指到整站拦截  

### mitmproxy

```bash
cd /path/to/codex_select
mitmdump -s scripts/kimi_block_work_model_config.py --listen-port 8080
```

仅让 Kimi 走该代理（App 级），装 CA 后拦截 path；失败则 Kimi 回退 cache。

---

## 关闭发布

Spur → **关闭发布** → 恢复备份 / 清注入 → 重开 Kimi。

---

## 影响对照

| 拦截范围 | Kimi 启动 | 右下角 |
|----------|-----------|--------|
| 无 | 正常 | 在线多为官方 3 个 |
| 仅 DescribeKimiWorkConfig | 正常 | 可用 cache 中的 spur |
| 整站 www.kimi.com | **无网** | — |
