<p align="center">
  <img src="src/assets/codex-spur-icon.png" alt="Codex Spur" width="128" height="128">
</p>

<h1 align="center">Codex Spur</h1>

<p align="center">
  <a href="./README.md">English</a> · <b>中文</b>
</p>

<h2 align="center">
  你配好的模型，全部进 Codex 选择器。<br>
  一键切换。
</h2>

<p align="center">
  <em style="font-size: 1.15em; line-height: 1.55;">
    在 Spur 里接好 Kimi、DeepSeek、xAI、OpenAI 多账号或任意兼容网关，启用后点一次 <strong>Review &amp; Apply</strong>——它们就会出现在 <strong>Codex / ChatGPT Desktop 原生模型菜单</strong>里。写代码时想换模型，就在官方选择器里点一下，不用开新窗口、不用改配置、不用记一堆 API 入口。
  </em>
</p>

<p align="center">
  <a href="https://github.com/williamdh457/codex-spur/releases/latest">下载 DMG</a>
  ·
  <a href="./CHANGELOG.md">更新日志</a>
  ·
  <a href="./LICENSE">MIT 许可</a>
</p>

---

## 关于（About）

### 已配置的模型，都在 Codex 选择器里一键切换

这就是产品本身。

<p align="center">
  <img src="docs/images/codex-model-picker.png" alt="Codex 模型选择器中显示由 Codex Spur 发布的 Grok、Kimi、DeepSeek、OpenAI 模型" width="720">
</p>

<p align="center"><sub>你在 Spur 里配置的模型，出现在 Codex / ChatGPT Desktop 原生选择器中——点一下即可切换。</sub></p>

在 Spur 接好供应商、勾选要发布的路由、**Review & Apply**——模型进入 **Codex 原生选择器**。赶速度用 Kimi、控成本用 DeepSeek、硬仗用 OpenAI、私有端点走自定义网关：全部 **一键切换**，不用离开 Codex，也不用反复改配置。

Spur 是 **local-first** 的桌面控制面：管你真正在用的模型，不是把密钥交给云端，也不是去改写 `ChatGPT.app`。

**密钥只留在本机。** API Key、session / refresh token、代理 bearer 加密落盘，不进入 UI，不上传任何 Codex Spur 云服务，也没有凭据遥测。

**不注入客户端。** 仅通过受支持的 seam 接入：

1. 本机 OpenAI Responses 兼容代理  
2. 生成的 `model_catalog_json`  
3. 专用 provider：`codex_select`  

关闭主窗口时菜单栏代理继续运行；退出应用才停止代理并释放租约。v1 **不**安装 LaunchAgent 或特权 helper。

| | |
|---|---|
| 平台 | macOS（Apple Silicon 优先） |
| 技术栈 | Tauri 2 · React · TypeScript · Rust |
| 版本 | **0.1.1** |
| 许可 | MIT |

---

## 功能一览

### 供应商实例

- 同一类型可添加多个实例  
- **添加 → 保存并拉取 → 概览出现新行**  
- OpenAI：官方浏览器 OAuth（PKCE）、API Key、多账号 JSON、配置 JSON  
- Kimi Code 默认 `https://api.kimi.com/coding/v1`  
- 拉取结果为候选；模型页启用后才进入 catalog  

### 路由与调度

多账号 OpenAI 支持 `Pool` / `Fixed`。Pool 顺序：`previous_response_id` 亲和 → session-hash 亲和 → Top-K 加权。不健康账号会 escape。

### Reasoning 八档

```text
none · minimal · low · medium · high · xhigh · max · ultra
```

上游无法区分的档位会如实标注。

### 额度与重置卡

按 `limit_window_seconds` 展示最近 5 小时 / 7 天窗口。消耗重置卡需确认 + 幂等键 + 审计。

### 安全

密钥仅本地；SQLite 存 AES-256-GCM 密文；主密钥为应用数据目录下 `master_key.hex`（`0600`）。

```text
~/Library/Application Support/com.codexspur.desktop/
```

---

## 安装

### 要求

- macOS Apple Silicon（本 release 提供 `aarch64` DMG）  
- 已安装 ChatGPT Desktop / Codex  
- 可访问所配置的上游 API  

### 从 Release 安装

1. 打开 [Releases](https://github.com/williamdh457/codex-spur/releases/latest) 下载 DMG  
2. 拖入「应用程序」  
3. **首次打开若提示「应用已损坏」**：多数情况下不是文件真坏了，而是未签名/未公证 + 隔离属性（Gatekeeper）。处理顺序：
   - 右键 App → **打开** → 仍要打开；或「系统设置 → 隐私与安全性 → 仍要打开」
   - 仍不行时在终端执行一次：
     ```bash
     xattr -cr "/Applications/Codex Spur.app"
     ```
     然后再打开  
4. 使用第三方模型时保持菜单栏进程在线  

> GitHub 公开 DMG 通常是 **ad-hoc 签名、未公证**。要让别人下载后双击即开，需要 Apple 开发者账号（约 $99/年）+ Developer ID 签名 + 公证（notarize）并 staple。免费 Apple ID 无法彻底解决「从网上下载就损坏」的问题。

### 卸载

菜单栏退出 → 删除 App →（可选）删除 Application Support 数据 → 按需恢复 `~/.codex` 备份。

---

## 快速开始

1. 添加供应商并拉取模型  
2. 在模型页启用路由  
3. Review & Apply 写入 `codex_select` 与 catalog  
4. 在 Codex 模型选择器中一键切换  
5. 保持 Spur 代理运行  

### Desktop 可见性

| 登录 | 位置 | 用途 |
|------|------|------|
| Desktop 官方登录 | `~/.codex/auth.json` | GUI 是否显示第三方模型 |
| Spur 凭据 | 本地 vault | 仅代理上游鉴权 |

---

## 从源码构建

```bash
npm install
npm run dev:app
npm run typecheck && npm run lint && npm run test
npm run bundle:dmg
```

详情见英文 [README](./README.md)。

---

## 架构与文档

- [`AGENTS.md`](./AGENTS.md) · [`DESIGN.md`](./DESIGN.md) · [`IMPLEMENTATION.md`](./IMPLEMENTATION.md)  
- [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md) · [`CHANGELOG.md`](./CHANGELOG.md) · [`LICENSE`](./LICENSE)  

---

## 免责声明

本工具为本地集成助手。请遵守上游服务条款；配额、账号与备份责任由使用者自负。
