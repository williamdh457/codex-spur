# Codex Spur

**Local-first model & account router for OpenAI Codex / ChatGPT Desktop (macOS).**

Codex Spur 把你自选的模型（Kimi、DeepSeek、OpenAI 多账号、xAI/Grok、自定义网关等）发布进 Codex 右下角模型列表，**不修改、不注入** `ChatGPT.app`。

| | |
|---|---|
| Platform | macOS (Apple Silicon first) |
| Stack | Tauri 2 · React · TypeScript · Rust |
| Version | **0.1.0** |
| License | MIT |

---

## 它做什么

Codex Spur 通过三条官方/兼容 seam 接入 Codex，而不是改客户端二进制：

1. **本机 OpenAI Responses 兼容代理**（仅绑定 `127.0.0.1`）
2. **生成的 `model_catalog_json`**
3. **专用 provider：`codex_select`**（不会覆盖你现有的 `custom` / Nice Switch / CC Switch 等配置）

关闭主窗口时，菜单栏进程会继续跑代理；**退出应用**才会停代理并释放账号租约。v1 **不**安装 LaunchAgent、特权 helper 或无关后台守护进程。

```text
ChatGPT Desktop / Codex
        │  Responses API
        ▼
  127.0.0.1 proxy  (bearer required)
        │
        ├─ OpenAI / ChatGPT backend
        ├─ Kimi / DeepSeek / MiniMax / xAI
        └─ Custom OpenAI-compatible gateways
```

---

## 功能一览

### 供应商实例（主对象）

- 同一类型可添加多个实例（多个 OpenAI、多个 Kimi…）
- **添加 → 保存并拉取模型 → 概览列表出现新行**
- OpenAI 入口：
  - 官方订阅（浏览器 localhost PKCE OAuth）
  - API Key
  - 多账号凭据 JSON
  - 供应商配置 JSON（`base_url` / 别名 / env）
- Kimi Code 默认 `https://api.kimi.com/coding/v1`
- 拉取结果进入**候选**；模型页**逐个启用**后才进入 catalog / Codex 选择器

### 路由与调度

多账号 OpenAI 实例支持两种模式：

- `Pool` — 池内调度  
- `Fixed` — 固定账号  

Pool 调度顺序（行为契约，独立实现）：

1. `previous_response_id` 亲和  
2. session-hash 亲和  
3. 过滤后负载感知 Top-K 加权选择  

账号须通过能力、token、冷却、额度、并发等检查；不健康时 sticky 会 escape 并重绑。

### Reasoning 八档

Codex 侧固定阶梯：

```text
none · minimal · low · medium · high · xhigh · max · ultra
```

每个模型路由为八档写清上游 patch、实际行为与说明；上游不能关闭/变化的，会如实标注，不会假装档位有效。

### 额度与重置卡

- 按 `limit_window_seconds` 展示最近的 **5 小时 / 7 天** 窗口  
- 刷新失败**不会**自动禁用健康账号  
- 消耗重置卡：显式确认 + 幂等键 + 审计；超时不确定时**禁止**换新键重试  

### 安全与隐私

- 凭据**仅本地**；无遥测上传 secret  
- 前端**永不**收到 access/refresh token、API key、代理 bearer 明文  
- SQLite 存 AES-256-GCM 密文；主密钥在应用数据目录 `master_key.hex`（权限 `0600`）  
- 日志与 UI 错误会脱敏 token / email / Authorization  

数据目录（macOS 典型路径）：

```text
~/Library/Application Support/com.codexspur.desktop/
```

---

## 安装（用户）

### 系统要求

- macOS **Apple Silicon**（本 release 提供 `aarch64` DMG）
- 已安装并可登录的 **ChatGPT Desktop / Codex**（第三方模型要在 GUI 里出现，通常需要有效的 Desktop 官方登录，见下文「Desktop 可见性」）
- 网络可访问你所配置的上游 API

### 从 Release 安装

1. 打开 [Releases](../../releases) 页面，下载最新  
   `Codex Spur_0.1.0_aarch64.dmg`
2. 打开 DMG，将 **Codex Spur** 拖到「应用程序」
3. 首次打开若遇 Gatekeeper 拦截：  
   **系统设置 → 隐私与安全性 → 仍要打开**  
   （或：右键 App → 打开）
4. 启动后菜单栏会驻留；主窗口可关，代理仍在

> 当前构建为**开发/未公证**常见分发形态。若你需要企业分发，请自行签名与公证。

### 卸载

1. 退出 Codex Spur（菜单栏 → 退出，不只是关窗口）  
2. 删除 `/Applications/Codex Spur.app`  
3. （可选）删除本地数据：  
   `~/Library/Application Support/com.codexspur.desktop/`  
4. 如曾 Apply 过配置，可在 App 内恢复备份，或手动检查  
   `~/.codex/config.toml` 与 `~/.codex/codex-select/`

---

## 快速开始（配置 Codex）

1. **添加供应商**  
   概览 → 添加 → 选类型与入口 → 保存并拉取模型  
2. **启用模型**  
   模型页勾选要发布到 Codex 的路由  
3. **预览并 Apply**  
   应用配置前会生成 diff/预览；确认后写入：  
   - `~/.codex/config.toml` 中的 `[model_providers.codex_select]`  
   - catalog：`~/.codex/codex-select/model-catalog.json`  
4. **Desktop 可见性**  
   概览页检查清单为就绪后，重启或刷新 ChatGPT Desktop，在模型列表中选择 Spur 发布的模型  
5. **保持 Spur 在运行**  
   代理必须在线，请求才会转发到上游  

### Desktop 可见性（重要）

| 登录 | 位置 | 用途 |
|------|------|------|
| ChatGPT Desktop 官方登录 | `~/.codex/auth.json` | GUI 身份门控，影响是否显示第三方模型 |
| Spur 供应商凭据 | Spur 本地 vault | **仅**代理上游鉴权，**不能**替代 Desktop 门控 |

Apply 会为 `codex_select` 写入兼容字段（如 `requires_openai_auth = true`、`supports_websockets = false` 等）。catalog 含非官方 slug 且缺少有效 `auth.json` 时，Apply 会**硬拦截**（不会写假 token）。

### CLI 一键发布（可选）

已在 UI 勾选模型后，可用：

```bash
cargo run --manifest-path src-tauri/Cargo.toml --bin codex-spur-publish
```

从 Spur SQLite 重建 catalog 并写入 `~/.codex`（调试/脚本场景）。

---

## 从源码构建

### 依赖

- Node.js 20+（推荐 22）
- Rust stable（`rustc` ≥ 1.86，见 `Cargo.toml`）
- Xcode Command Line Tools / 常规 macOS 原生构建工具链
- [Tauri 2 系统依赖](https://v2.tauri.app/start/prerequisites/)

### 开发

```bash
npm install
npm run dev:app          # Tauri + Vite 热重载
# 或
npm run dev              # 仅前端
```

### 检查

```bash
npm run typecheck
npm run lint
npm run test
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

### 打包 DMG

```bash
npm run bundle:dmg
# 产物：
# src-tauri/target/release/bundle/dmg/Codex Spur_<version>_aarch64.dmg
```

真实上游 smoke / 重置卡测试会消耗配额，**必须显式 opt-in**，默认 CI 不要跑。

---

## 架构边界（给贡献者）

| 层 | 职责 |
|----|------|
| React UI | 展示、交互、可访问性；只调类型化 Tauri 命令 |
| Rust core | 凭据、调度、代理、catalog、Codex 配置写入、备份恢复 |
| Proxy | Responses 兼容；可取消上游；断连释放 lease |
| Codex 集成 | 仅 `codex_select`；`toml_edit` 保留无关段落与注释 |

更细的产品契约见仓库内：

- [`AGENTS.md`](./AGENTS.md) — 工程与安全硬约束  
- [`DESIGN.md`](./DESIGN.md) — 桌面 UI 设计系统  
- [`IMPLEMENTATION.md`](./IMPLEMENTATION.md) — 当前实现说明  
- [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md) — 第三方行为参考声明  
- [`CHANGELOG.md`](./CHANGELOG.md) — 版本记录  

---

## 配置与文件

| 路径 | 说明 |
|------|------|
| `~/Library/Application Support/com.codexspur.desktop/` | Spur 本地 DB、主密钥、代理 bearer 等 |
| `~/.codex/config.toml` | Codex 配置（Apply 时备份后原子更新） |
| `~/.codex/codex-select/model-catalog.json` | 发布的模型目录 |
| `~/.codex/auth.json` | **原生** Codex/Desktop 登录（Spur 正常运行不改它） |

Apply 流水线：预览 → 哈希/锁 → 备份 → 临时文件 → fsync → 原子 rename → 回读校验；失败可回滚。

---

## 许可证与合规

- 本项目以 **MIT** 发布，见 [`LICENSE`](./LICENSE)。  
- **Sub2API（LGPL-3.0）** 仅作行为参考，**未**拷贝其源码。  
- **Codex++（AGPL-3.0）** 仅作架构参考，**未**拷贝源码。  
- 细节见 [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md)。  

请遵守各上游服务的服务条款与当地法律。本工具**不会**协助绕过 CAPTCHA、手机验证、套餐限制或滥用防护。

---

## 免责声明

Codex Spur 以「本地工具」方式集成 Codex 配置与代理协议。API 与 Desktop 行为可能随上游变更；请在使用前自行验证对你账户与工作流的影响。作者不对配额消耗、账号封禁或数据丢失承担责任——请做好备份，并谨慎使用重置卡等不可逆操作。

---

## 发布清单（维护者）

```bash
# 1. 版本号对齐 package.json / tauri.conf.json / Cargo.toml / CHANGELOG
# 2. 测试与 typecheck / clippy
# 3. 打包
npm run bundle:dmg
# 4. 打 tag 并创建 GitHub Release，附上 DMG 与发行说明
git tag -a v0.1.0 -m "v0.1.0"
git push origin main --tags
gh release create v0.1.0 \
  "src-tauri/target/release/bundle/dmg/Codex Spur_0.1.0_aarch64.dmg" \
  --title "Codex Spur 0.1.0" \
  --notes-file CHANGELOG.md
```

---

## 反馈

请通过 GitHub Issues 报告 bug 或需求。提交问题时尽量附上：

- macOS 版本与芯片  
- Spur / Codex Desktop 版本  
- 诊断页中的**已脱敏**事件摘要（勿粘贴 token）  
- 复现步骤  
