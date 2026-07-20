import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import brandIcon from "./assets/codex-spur-icon.png";
import {
  applyCodexConfig,
  cancelOpenAiDeviceLogin,
  clearProxyRequestEvents,
  completeOpenAiDeviceLogin,
  createProviderInstance,
  deleteProviderInstance,
  discoverProviderModels,
  getAppSnapshot,
  getDiagnosticsMaxEvents,
  getPoolSchedulerConfig,
  getUsageSnapshot,
  importCredentialsJson,
  importProviderConfigJson,
  listCredentials,
  listModelRoutes,
  listPoolMembersDetailed,
  listProxyRequestEvents,
  openExternalUrl,
  pollOpenAiDeviceLogin,
  previewCodexApply,
  renameProviderInstance,
  restorePreviousCodexConfig,
  setDiagnosticsMaxEvents,
  setModelEnabled,
  setProviderRouting,
  startOpenAiDeviceLogin,
  updatePoolMember,
  updatePoolSchedulerConfig,
} from "./api";
import type { DeviceLoginStart } from "./api";
import type {
  AppSnapshot,
  CredentialSummary,
  ModelRouteSummary,
  NavigationSection,
  PoolMemberDetail,
  PoolSchedulerConfig,
  ProviderKind,
  ProviderSummary,
  ProxyRequestEvent,
  StatusTone,
  UsageSnapshot,
} from "./types";

const NAVIGATION: Array<{ id: NavigationSection; label: string; icon: string }> = [
  { id: "overview", label: "概览", icon: "◫" },
  { id: "models", label: "模型", icon: "◇" },
  { id: "usage", label: "用量", icon: "▥" },
  { id: "diagnostics", label: "诊断", icon: "⌁" },
  { id: "settings", label: "设置", icon: "⚙" },
];

function statusTone(snapshot: AppSnapshot): StatusTone {
  if (!snapshot.proxy.running || snapshot.binding.state === "invalid") return "error";
  if (snapshot.binding.state !== "applied" || snapshot.attentionItems.length > 0) return "warning";
  return "healthy";
}

function StatusDot({ tone }: { tone: StatusTone }) {
  return <span className={`status-dot status-dot--${tone}`} aria-hidden="true" />;
}

function Metric({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div className="metric">
      <span className="metric__label">{label}</span>
      <strong className="metric__value">{value}</strong>
      <span className="metric__note">{note}</span>
    </div>
  );
}

function ProviderRow({ provider, onSelect }: { provider: ProviderSummary; onSelect?: () => void }) {
  return (
    <button className="data-row provider-row" type="button" onClick={onSelect}>
      <span className="provider-mark" aria-hidden="true">{provider.name.slice(0, 1)}</span>
      <span className="data-row__main">
        <strong>{provider.name}</strong>
        <small>{provider.kind} · {provider.region} · {provider.protocol}</small>
      </span>
      <span className={`badge ${provider.configured ? "badge--success" : "badge--neutral"}`}>
        {provider.configured ? "已配置" : "未配置"}
      </span>
      <span className="provider-count">{provider.selectedModels}/{provider.discoveredModels} 模型 · {provider.healthyCredentialCount}/{provider.credentialCount} 账号</span>
      <span className="chevron" aria-hidden="true">›</span>
    </button>
  );
}

function EmptyState({ title, body, action, onAction }: { title: string; body: string; action: string; onAction?: () => void }) {
  return <div className="empty-state"><div className="empty-state__symbol" aria-hidden="true">＋</div><strong>{title}</strong><p>{body}</p><button type="button" className="button button--secondary" onClick={onAction}>{action}</button></div>;
}

function Overview({
  snapshot,
  onAddProvider,
  onEditProvider,
}: {
  snapshot: AppSnapshot;
  onAddProvider: () => void;
  onEditProvider: (provider: ProviderSummary) => void;
}) {
  return (
    <div className="page-stack">
      <section className="metrics-grid" aria-label="运行摘要">
        <Metric label="本地代理" value={snapshot.proxy.running ? "运行中" : "已停止"} note={snapshot.proxy.baseUrl ?? "未绑定"} />
        <Metric label="Codex 绑定" value={snapshot.binding.state === "applied" ? "已应用" : "待应用"} note={snapshot.binding.providerId} />
        <Metric label="已发布模型" value={String(snapshot.publishedModels)} note="右下角模型选择器" />
        <Metric label="健康账号" value={String(snapshot.healthyAccounts)} note="可参与调度" />
      </section>
      <section className="panel">
        <div className="panel__header">
          <div><h2>需要处理</h2><p>只列出会阻止路由或应用的问题。</p></div>
          <span className="badge badge--warning">{snapshot.attentionItems.length}</span>
        </div>
        <div className="attention-list">
          {snapshot.attentionItems.length === 0 ? (
            <div className="attention-item attention-item--ok">当前没有需要处理的问题。</div>
          ) : (
            snapshot.attentionItems.map((item) => (
              <div className="attention-item" key={item}><span aria-hidden="true">!</span><p>{item}</p></div>
            ))
          )}
        </div>
      </section>
      <section className="panel">
        <div className="panel__header">
          <div>
            <h2>供应商</h2>
            <p>可添加多个 OpenAI / Kimi / DeepSeek… 每个实例在列表里占一行。</p>
          </div>
          <button type="button" className="button button--primary" onClick={onAddProvider}>添加供应商</button>
        </div>
        {snapshot.providers.length === 0 ? (
          <EmptyState
            title="还没有供应商"
            body="像 CC Switch 一样添加：OpenAI、导入 JSON、导入账号，或 Kimi / DeepSeek。保存并拉取后会出现在这里。"
            action="添加供应商"
            onAction={onAddProvider}
          />
        ) : (
          <div className="rows">
            {snapshot.providers.map((provider) => (
              <ProviderRow key={provider.id} provider={provider} onSelect={() => onEditProvider(provider)} />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

type AddMethodId =
  | "openai-official"
  | "openai-api"
  | "openai-accounts"
  | "openai-config-json"
  | "kimi"
  | "deepseek"
  | "minimax"
  | "custom"
  | "custom-config-json";

type AddMethod = {
  id: AddMethodId;
  kind: ProviderKind;
  title: string;
  hint: string;
  mode: "api" | "configJson" | "accounts" | "oauth";
};

const ADD_METHODS: AddMethod[] = [
  { id: "openai-official", kind: "openai", title: "OpenAI · 官方订阅", hint: "打开浏览器登录 ChatGPT", mode: "oauth" },
  { id: "openai-api", kind: "openai", title: "OpenAI · API Key", hint: "api.openai.com 密钥", mode: "api" },
  { id: "openai-accounts", kind: "openai", title: "OpenAI · 导入账号", hint: "多账号凭据 JSON → 一个实例", mode: "accounts" },
  { id: "openai-config-json", kind: "openai", title: "OpenAI · 导入配置 JSON", hint: "base_url + api_key / models", mode: "configJson" },
  { id: "kimi", kind: "kimi", title: "Kimi Code", hint: "API Key（coding 端点）", mode: "api" },
  { id: "deepseek", kind: "deepseek", title: "DeepSeek", hint: "API Key + Base URL", mode: "api" },
  { id: "minimax", kind: "minimax", title: "MiniMax", hint: "API Key + Base URL", mode: "api" },
  { id: "custom", kind: "custom", title: "自定义", hint: "OpenAI-compatible", mode: "api" },
  { id: "custom-config-json", kind: "custom", title: "自定义 · 导入 JSON", hint: "供应商配置 JSON", mode: "configJson" },
];

const DEFAULT_BASE_URL: Record<ProviderKind, string> = {
  openai: "https://api.openai.com/v1",
  kimi: "https://api.kimi.com/coding/v1",
  deepseek: "https://api.deepseek.com/v1",
  minimax: "https://api.minimaxi.com/v1",
  custom: "",
};

function providerUrl(provider: ProviderSummary): string {
  if (provider.kind === "kimi" && (!provider.baseUrl || provider.baseUrl.includes("www.kimi.com/code") || provider.baseUrl.includes("api.moonshot.cn"))) {
    return "https://api.kimi.com/coding/v1";
  }
  if (provider.kind === "openai" && provider.baseUrl) {
    return provider.baseUrl;
  }
  return provider.baseUrl ?? provider.defaultBaseUrl ?? DEFAULT_BASE_URL[provider.kind] ?? "";
}

function useEscapeClose(onClose: () => void) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);
}

function resolveAddMethod(id: AddMethodId): AddMethod {
  const found = ADD_METHODS.find((item) => item.id === id);
  if (found) return found;
  return {
    id: "openai-official",
    kind: "openai",
    title: "OpenAI · 官方订阅",
    hint: "打开浏览器登录 ChatGPT",
    mode: "oauth",
  };
}

function AddProviderWizard({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => Promise<void>;
}) {
  const accountFileRef = useRef<HTMLInputElement>(null);
  const configFileRef = useRef<HTMLInputElement>(null);
  const [methodId, setMethodId] = useState<AddMethodId>("openai-official");
  const method = resolveAddMethod(methodId);
  const [displayName, setDisplayName] = useState("");
  const [baseUrl, setBaseUrl] = useState(DEFAULT_BASE_URL.openai);
  const [apiKey, setApiKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [login, setLogin] = useState<DeviceLoginStart | null>(null);
  const [loginStatus, setLoginStatus] = useState<string | null>(null);
  const pollRef = useRef<number | null>(null);

  useEscapeClose(onClose);

  useEffect(() => {
    return () => {
      if (pollRef.current !== null) window.clearTimeout(pollRef.current);
    };
  }, []);

  const selectMethod = (nextId: AddMethodId) => {
    const next = resolveAddMethod(nextId);
    setMethodId(nextId);
    setMessage(null);
    setApiKey("");
    setLogin(null);
    setLoginStatus(null);
    setBaseUrl(DEFAULT_BASE_URL[next.kind]);
    if (pollRef.current !== null) {
      window.clearTimeout(pollRef.current);
      pollRef.current = null;
    }
  };

  const rollback = async (providerId: string) => {
    try {
      await deleteProviderInstance(providerId);
    } catch {
      // Best-effort cleanup if configure failed after create.
    }
  };

  const finishCreate = async (created: ProviderSummary, modelCount: number, warning?: string) => {
    setMessage(warning ?? `已添加 ${created.name}，拉取 ${modelCount} 个模型候选。`);
    await onCreated();
    if (!warning) onClose();
  };

  const submitApi = async () => {
    setBusy(true);
    setMessage(null);
    let createdId: string | null = null;
    try {
      if (!baseUrl.trim() && method.kind === "custom") {
        setMessage("请填写 Base URL。");
        return;
      }
      const created = await createProviderInstance(method.kind, displayName.trim() || undefined);
      createdId = created.id;
      const routes = await discoverProviderModels(created.id, baseUrl, apiKey || undefined);
      const count = routes.filter((route) => route.providerId === created.id).length;
      await finishCreate(created, count);
    } catch (error) {
      if (createdId) await rollback(createdId);
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const submitConfigJson = async (file: File) => {
    setBusy(true);
    setMessage(null);
    let createdId: string | null = null;
    try {
      const created = await createProviderInstance(method.kind, displayName.trim() || undefined);
      createdId = created.id;
      const routes = await importProviderConfigJson(created.id, await file.text());
      const count = routes.filter((route) => route.providerId === created.id).length;
      await finishCreate(created, count);
    } catch (error) {
      if (createdId) await rollback(createdId);
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      if (configFileRef.current) configFileRef.current.value = "";
    }
  };

  const submitAccounts = async (file: File) => {
    setBusy(true);
    setMessage(null);
    let createdId: string | null = null;
    let accountsImported = false;
    try {
      const created = await createProviderInstance(method.kind, displayName.trim() || undefined);
      createdId = created.id;
      const imported = await importCredentialsJson(created.id, await file.text());
      if (imported.length === 0) {
        throw new Error("未解析到任何账号，请检查 JSON。");
      }
      accountsImported = true;
      try {
        const routes = await discoverProviderModels(created.id, "", undefined);
        const count = routes.filter((route) => route.providerId === created.id).length;
        await finishCreate(created, count);
      } catch (modelError) {
        await onCreated();
        setMessage(
          `已导入 ${imported.length} 个账号到「${created.name}」，模型拉取失败：${
            modelError instanceof Error ? modelError.message : String(modelError)
          }。实例已保留，可稍后重试。`,
        );
      }
    } catch (error) {
      if (createdId && !accountsImported) await rollback(createdId);
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      if (accountFileRef.current) accountFileRef.current.value = "";
    }
  };

  const stopPolling = () => {
    if (pollRef.current !== null) {
      window.clearTimeout(pollRef.current);
      pollRef.current = null;
    }
  };

  const schedulePoll = (deviceCode: string, intervalSecs: number) => {
    stopPolling();
    pollRef.current = window.setTimeout(() => {
      void (async () => {
        try {
          const result = await pollOpenAiDeviceLogin(deviceCode);
          if (result.status === "pending") {
            setLoginStatus("等待浏览器完成登录…");
            schedulePoll(deviceCode, intervalSecs);
            return;
          }
          if (result.status === "success" && result.tokens) {
            setLoginStatus("登录成功，正在保存并拉取模型…");
            setBusy(true);
            const complete = await completeOpenAiDeviceLogin(result.tokens, displayName.trim() || undefined);
            await onCreated();
            if (complete.modelError) {
              setMessage(complete.modelError);
              setLogin(null);
            } else {
              setMessage(`已添加 ${complete.provider.name}，拉取 ${complete.modelCount} 个模型候选。`);
              onClose();
            }
            setBusy(false);
            return;
          }
          setMessage(result.message ?? "登录失败，请重试。");
          setLogin(null);
          setBusy(false);
        } catch (error) {
          setMessage(error instanceof Error ? error.message : String(error));
          setBusy(false);
        }
      })();
    }, Math.max(3, intervalSecs) * 1000);
  };

  const startOfficialLogin = async () => {
    setBusy(true);
    setMessage(null);
    setLoginStatus(null);
    try {
      const started = await startOpenAiDeviceLogin();
      setLogin(started);
      setLoginStatus("已打开浏览器，请在页面输入下方代码完成登录。");
      try {
        await openExternalUrl(started.verificationUri);
      } catch {
        // User can open manually.
      }
      schedulePoll(started.deviceCode, started.intervalSecs);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const cancelLogin = async () => {
    stopPolling();
    if (login) {
      try {
        await cancelOpenAiDeviceLogin(login.deviceCode);
      } catch {
        // ignore
      }
    }
    setLogin(null);
    setLoginStatus(null);
  };

  return (
    <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <section className="provider-modal provider-modal--wizard" role="dialog" aria-modal="true" aria-labelledby="add-provider-title">
        <header className="provider-modal__header">
          <div>
            <small>ADD PROVIDER</small>
            <h2 id="add-provider-title">添加供应商</h2>
            <p>选类型与方式，保存并拉取后主列表会多一行。同一类型可添加无数个。</p>
          </div>
          <button type="button" className="icon-button" aria-label="关闭添加供应商" onClick={onClose}>×</button>
        </header>
        <div className="provider-modal__body provider-modal__body--wizard">
          <div className="method-grid" role="listbox" aria-label="供应商类型与方式">
            {ADD_METHODS.map((item) => (
              <button
                key={item.id}
                type="button"
                role="option"
                aria-selected={item.id === method.id}
                className={`method-card ${item.id === method.id ? "method-card--active" : ""}`}
                onClick={() => selectMethod(item.id)}
              >
                <strong>{item.title}</strong>
                <small>{item.hint}</small>
              </button>
            ))}
          </div>
          <div className="provider-modal__content">
            <label className="field">
              <span>显示名称（可选）</span>
              <input
                value={displayName}
                onChange={(event) => setDisplayName(event.target.value)}
                placeholder={`默认：${method.kind === "custom" ? "自定义供应商" : method.title.split(" · ")[0]} / 第 2 个起自动编号`}
              />
            </label>

            {method.mode === "oauth" && (
              <section className="modal-section" aria-label="OpenAI 官方订阅登录">
                <div className="callout">
                  <strong>OpenAI · 官方订阅</strong>
                  <p>与 CC Switch 类似：打开 OpenAI 设备登录页，用 ChatGPT 账号授权。成功后会新建一个供应商实例并拉取官方模型。</p>
                  {!login ? (
                    <button type="button" className="button button--primary" disabled={busy} onClick={() => void startOfficialLogin()}>
                      {busy ? "正在启动登录…" : "打开 OpenAI 登录"}
                    </button>
                  ) : (
                    <>
                      <p>在浏览器打开： <code>{login.verificationUri}</code></p>
                      <p>输入代码： <strong className="user-code">{login.userCode}</strong></p>
                      {loginStatus && <p>{loginStatus}</p>}
                      <div className="form-actions">
                        <button type="button" className="button button--secondary" disabled={busy} onClick={() => void openExternalUrl(login.verificationUri)}>再次打开页面</button>
                        <button type="button" className="button button--secondary" disabled={busy} onClick={() => void cancelLogin()}>取消登录</button>
                      </div>
                    </>
                  )}
                </div>
              </section>
            )}

            {method.mode === "api" && (
              <section className="modal-section" aria-label="API 配置">
                {method.kind === "kimi" && (
                  <p className="panel-hint">Kimi Code 默认端点 <code>https://api.kimi.com/coding/v1</code>。只需填 API Key。</p>
                )}
                <label className="field">
                  <span>Base URL</span>
                  <input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://provider.example.com/v1" spellCheck={false} />
                </label>
                <label className="field">
                  <span>API Key</span>
                  <input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder="只写入本地加密存储" autoComplete="off" />
                </label>
                <div className="form-actions">
                  <button type="button" className="button button--primary" disabled={busy} onClick={() => void submitApi()}>
                    {busy ? "正在保存并拉取…" : "保存并拉取模型"}
                  </button>
                </div>
              </section>
            )}

            {method.mode === "configJson" && (
              <section className="modal-section" aria-label="导入供应商配置 JSON">
                <div className="callout">
                  <strong>导入供应商配置 JSON</strong>
                  <p>需要配置对象（含 base_url / baseUrl / OPENAI_BASE_URL 等）。若文件是 access_token / accounts，请改用「导入账号」或「官方订阅」。</p>
                  <pre className="code-snippet">{`{
  "base_url": "https://api.example.com/v1",
  "api_key": "sk-...",
  "models": [{ "id": "model-a", "display_name": "Model A" }]
}`}</pre>
                  <button type="button" className="button button--primary" disabled={busy} onClick={() => configFileRef.current?.click()}>
                    {busy ? "导入中…" : "选择配置 JSON 并添加"}
                  </button>
                  <input
                    ref={configFileRef}
                    className="visually-hidden"
                    type="file"
                    accept=".json,application/json"
                    onChange={(event) => {
                      const file = event.target.files?.[0];
                      if (file) void submitConfigJson(file);
                    }}
                  />
                </div>
              </section>
            )}

            {method.mode === "accounts" && (
              <section className="modal-section" aria-label="导入账号">
                <div className="callout">
                  <strong>OpenAI · 导入账号</strong>
                  <p>选择多账号凭据 JSON（account.json / Sub2API / access_token…）。会新建<strong>一个</strong> OpenAI 实例，账号写入该实例并拉取官方模型。</p>
                  <button type="button" className="button button--primary" disabled={busy} onClick={() => accountFileRef.current?.click()}>
                    {busy ? "导入中…" : "选择账号 JSON 并添加"}
                  </button>
                  <input
                    ref={accountFileRef}
                    className="visually-hidden"
                    type="file"
                    accept=".json,application/json"
                    onChange={(event) => {
                      const file = event.target.files?.[0];
                      if (file) void submitAccounts(file);
                    }}
                  />
                </div>
              </section>
            )}

            {message && <div className={message.startsWith("已") ? "inline-success" : "inline-warning"}>{message}</div>}
          </div>
        </div>
        <footer className="provider-modal__footer">
          <span>模型发现后仍需在「模型」页开启要显示在 Codex 中的项。</span>
          <button type="button" className="button button--secondary" onClick={onClose}>取消</button>
        </footer>
      </section>
    </div>
  );
}

function defaultSchedulerConfig(): PoolSchedulerConfig {
  return {
    lbTopK: 7,
    stickySessionTtlSecs: 3600,
    stickyResponseIdTtlSecs: 3600,
    scoreWeights: {
      priority: 1,
      load: 1,
      queue: 0.7,
      errorRate: 0.8,
      ttft: 0.5,
      reset: 0,
      quotaHeadroom: 0,
    },
    stickyEscape: { enabled: true, ttftMs: 15000, errorRate: 0.5 },
    preferSoonestReset: false,
    default429CooldownSecs: 30,
    maxFailoverSwitches: 3,
    leaseTtlSecs: 900,
  };
}

function EditProviderSheet({
  provider,
  onClose,
  onChanged,
}: {
  provider: ProviderSummary;
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const accountFileRef = useRef<HTMLInputElement>(null);
  const configFileRef = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(provider.name);
  const [source, setSource] = useState<"official" | "apiKey">(
    provider.kind === "openai" && (provider.baseUrl?.includes("chatgpt.com") ?? true) ? "official" : "apiKey",
  );
  const [baseUrl, setBaseUrl] = useState(() => providerUrl(provider));
  const [apiKey, setApiKey] = useState("");
  const [accounts, setAccounts] = useState<CredentialSummary[]>([]);
  const [members, setMembers] = useState<PoolMemberDetail[]>([]);
  const [routingMode, setRoutingMode] = useState<"pool" | "fixed">(
    provider.routingMode === "fixed" ? "fixed" : "pool",
  );
  const [fixedCredentialId, setFixedCredentialId] = useState<string | null>(
    provider.fixedCredentialId,
  );
  const [poolId, setPoolId] = useState<string | null>(provider.activePoolId);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEscapeClose(onClose);

  const applyAccountSnapshot = useCallback(
    async (providerId: string, activePool: string | null, mode: string, fixedId: string | null) => {
      const nextAccounts = await listCredentials(providerId);
      setAccounts(nextAccounts);
      setPoolId(activePool);
      setRoutingMode(mode === "fixed" ? "fixed" : "pool");
      setFixedCredentialId(fixedId);
      if (activePool) {
        try {
          setMembers(await listPoolMembersDetailed(activePool));
        } catch {
          setMembers([]);
        }
      } else {
        setMembers([]);
      }
    },
    [],
  );

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const nextAccounts = await listCredentials(provider.id);
        if (!active) return;
        setAccounts(nextAccounts);
        setPoolId(provider.activePoolId);
        setRoutingMode(provider.routingMode === "fixed" ? "fixed" : "pool");
        setFixedCredentialId(provider.fixedCredentialId);
        if (provider.activePoolId) {
          const nextMembers = await listPoolMembersDetailed(provider.activePoolId);
          if (!active) return;
          setMembers(nextMembers);
        } else {
          setMembers([]);
        }
      } catch {
        if (active) {
          setAccounts([]);
          setMembers([]);
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [provider.activePoolId, provider.fixedCredentialId, provider.id, provider.routingMode]);

  const saveName = async () => {
    if (name.trim() === provider.name || !name.trim()) return;
    try {
      await renameProviderInstance(provider.id, name.trim());
      await onChanged();
      setMessage("已更新显示名称。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  const configureApi = async () => {
    if (source === "apiKey" && !baseUrl.trim() && !provider.defaultBaseUrl && !provider.baseUrl) {
      setMessage("请填写 Base URL。");
      return;
    }
    if (source === "official" && provider.kind === "openai" && accounts.length === 0) {
      setMessage("官方订阅需要先导入账号 JSON。");
      return;
    }
    setBusy(true);
    setMessage(null);
    try {
      const routes = await discoverProviderModels(
        provider.id,
        source === "official" ? "" : baseUrl,
        source === "apiKey" ? (apiKey || undefined) : undefined,
      );
      setApiKey("");
      const count = routes.filter((route) => route.providerId === provider.id).length;
      setMessage(`已保存并拉取 ${count} 个模型。`);
      await applyAccountSnapshot(provider.id, provider.activePoolId, provider.routingMode, provider.fixedCredentialId);
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const importAccountFile = async (file: File) => {
    setBusy(true);
    setMessage(null);
    try {
      const imported = await importCredentialsJson(provider.id, await file.text());
      await applyAccountSnapshot(provider.id, provider.activePoolId, provider.routingMode, provider.fixedCredentialId);
      setMessage(`已导入 ${imported.length} 个账号到此实例。`);
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      if (accountFileRef.current) accountFileRef.current.value = "";
    }
  };

  const importConfigFile = async (file: File) => {
    setBusy(true);
    setMessage(null);
    try {
      const routes = await importProviderConfigJson(provider.id, await file.text());
      const count = routes.filter((route) => route.providerId === provider.id).length;
      setMessage(`已导入供应商配置，当前共 ${count} 个模型候选。`);
      await applyAccountSnapshot(provider.id, provider.activePoolId, provider.routingMode, provider.fixedCredentialId);
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      if (configFileRef.current) configFileRef.current.value = "";
    }
  };

  const applyRouting = async (mode: "pool" | "fixed", fixedId: string | null) => {
    setBusy(true);
    setMessage(null);
    try {
      const next = await setProviderRouting(provider.id, mode, fixedId);
      setRoutingMode(next.routingMode === "fixed" ? "fixed" : "pool");
      setFixedCredentialId(next.fixedCredentialId);
      setMessage(mode === "fixed" ? "已切换为固定账号。" : "已切换为池调度。");
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const saveMember = async (member: PoolMemberDetail) => {
    if (!poolId) return;
    setBusy(true);
    setMessage(null);
    try {
      await updatePoolMember(
        poolId,
        member.credentialId,
        member.weight,
        member.priority,
        member.enabled,
        member.concurrencyLimit,
      );
      setMembers(await listPoolMembersDetailed(poolId));
      setMessage("已更新账号调度参数。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!window.confirm(`确定删除供应商「${provider.name}」？其账号与模型候选会一并删除。`)) return;
    setBusy(true);
    setMessage(null);
    try {
      await deleteProviderInstance(provider.id);
      await onChanged();
      onClose();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <section className="provider-modal provider-modal--edit" role="dialog" aria-modal="true" aria-labelledby="edit-provider-title">
        <header className="provider-modal__header">
          <div>
            <small>EDIT PROVIDER</small>
            <h2 id="edit-provider-title">{provider.name}</h2>
            <p>{provider.kind} · {provider.protocol} · {provider.region}</p>
          </div>
          <button type="button" className="icon-button" aria-label="关闭编辑供应商" onClick={onClose}>×</button>
        </header>
        <div className="provider-modal__body provider-modal__body--single">
          <div className="provider-modal__content">
            <section className="modal-section">
              <div className="modal-section__header">
                <div><h3>实例</h3><p>每个实例独立配置；同 kind 可有多条。</p></div>
                <span className={`badge ${provider.configured ? "badge--success" : "badge--neutral"}`}>
                  {provider.configured ? "已配置" : "未配置"}
                </span>
              </div>
              <label className="field">
                <span>显示名称</span>
                <input value={name} onChange={(event) => setName(event.target.value)} onBlur={() => void saveName()} />
              </label>
            </section>

            <section className="modal-section" aria-label="连接">
              <div className="modal-section__header"><div><h3>连接</h3><p>保存并拉取会刷新此实例的模型候选。</p></div></div>
              {provider.kind === "openai" && (
                <div className="segmented-control" role="tablist" aria-label="OpenAI 通道">
                  <button type="button" className={source === "official" ? "segmented-control__item--active" : ""} onClick={() => { setSource("official"); setBaseUrl(provider.defaultBaseUrl ?? "https://chatgpt.com/backend-api/codex"); }}>官方订阅</button>
                  <button type="button" className={source === "apiKey" ? "segmented-control__item--active" : ""} onClick={() => { setSource("apiKey"); setBaseUrl(provider.baseUrl && !provider.baseUrl.includes("chatgpt.com") ? provider.baseUrl : "https://api.openai.com/v1"); }}>OpenAI API Key</button>
                </div>
              )}
              {source === "official" && provider.kind === "openai" ? (
                <div className="callout">
                  <strong>官方订阅通道</strong>
                  <p>使用此实例已导入的账号拉取官方模型。健康账号：{provider.healthyCredentialCount}/{provider.credentialCount}</p>
                </div>
              ) : (
                <>
                  <label className="field"><span>Base URL</span><input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} spellCheck={false} /></label>
                  <label className="field"><span>API Key{accounts.length > 0 ? "（可留空，使用已有账号）" : ""}</span><input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder="只写入本地加密存储" autoComplete="off" /></label>
                </>
              )}
              <div className="form-actions">
                <button type="button" className="button button--primary" disabled={busy} onClick={() => void configureApi()}>
                  {busy ? "正在保存并拉取…" : "保存并拉取模型"}
                </button>
              </div>
            </section>

            <section className="modal-section" aria-label="账号">
              <div className="modal-section__header">
                <div>
                  <h3>账号</h3>
                  <p>账号属于此实例。多账号时在实例内做 Sub2API 风格调度（粘性 → Top-K 加权），不是全局账号池产品。</p>
                </div>
                <span className="badge badge--neutral">{accounts.length} 个</span>
              </div>
              <div className="form-actions form-actions--wrap">
                <button type="button" className="button button--secondary" disabled={busy} onClick={() => accountFileRef.current?.click()}>导入账号 JSON</button>
                <button type="button" className="button button--secondary" disabled={busy} onClick={() => configFileRef.current?.click()}>导入供应商配置 JSON</button>
                <input ref={accountFileRef} className="visually-hidden" type="file" accept=".json,application/json" onChange={(event) => { const file = event.target.files?.[0]; if (file) void importAccountFile(file); }} />
                <input ref={configFileRef} className="visually-hidden" type="file" accept=".json,application/json" onChange={(event) => { const file = event.target.files?.[0]; if (file) void importConfigFile(file); }} />
              </div>

              {accounts.length > 1 ? (
                <div className="routing-toolbar">
                  <div className="segmented-control" role="group" aria-label="路由模式">
                    <button
                      type="button"
                      className={routingMode === "pool" ? "segmented-control__item--active" : undefined}
                      disabled={busy}
                      onClick={() => void applyRouting("pool", null)}
                    >
                      Pool
                    </button>
                    <button
                      type="button"
                      className={routingMode === "fixed" ? "segmented-control__item--active" : undefined}
                      disabled={busy}
                      onClick={() => {
                        const pick = fixedCredentialId ?? accounts[0]?.id ?? null;
                        if (pick) void applyRouting("fixed", pick);
                      }}
                    >
                      Fixed
                    </button>
                  </div>
                  <small>
                    {routingMode === "fixed"
                      ? "固定账号：所有请求只走选中账号。"
                      : "池调度：previous_response → session → Top-K 加权。高级参数（Top-K / weights / sticky）在「设置 → 调度」。"}
                  </small>
                </div>
              ) : null}

              {accounts.length === 0 ? (
                <div className="empty-inline">此实例还没有账号。可导入账号 JSON，或在上方填写 API Key 后保存。</div>
              ) : (
                <div className="modal-account-list modal-account-list--editable">
                  {accounts.map((account) => {
                    const member = members.find((item) => item.credentialId === account.id);
                    const selectedFixed = routingMode === "fixed" && fixedCredentialId === account.id;
                    return (
                      <div className={`modal-account-row modal-account-row--edit${selectedFixed ? " modal-account-row--fixed" : ""}`} key={account.id}>
                        {routingMode === "fixed" ? (
                          <input
                            type="radio"
                            name={`fixed-${provider.id}`}
                            checked={selectedFixed}
                            disabled={busy}
                            aria-label="固定此账号"
                            onChange={() => void applyRouting("fixed", account.id)}
                          />
                        ) : (
                          <StatusDot tone={account.healthy ? "healthy" : "error"} />
                        )}
                        <span>
                          <strong>{account.label ?? account.maskedEmail ?? account.maskedAccountId ?? account.fingerprintPrefix}</strong>
                          <small>
                            {account.kind} · {account.refreshable ? "可刷新" : "仅访问"}
                            {member ? ` · ${member.scheduleState}` : ""}
                            {member?.cooldownUntil ? " · cooldown" : ""}
                          </small>
                        </span>
                        <span className={`badge ${account.healthy ? "badge--success" : "badge--error"}`}>{account.healthy ? "可用" : "失效"}</span>
                        {routingMode === "pool" && member ? (
                          <div className="member-knobs" onClick={(event) => event.stopPropagation()}>
                            <label>
                              <span>W</span>
                              <input
                                type="number"
                                min={1}
                                value={member.weight}
                                disabled={busy}
                                onChange={(event) => {
                                  const weight = Number(event.target.value) || 1;
                                  setMembers((prev) => prev.map((item) => item.credentialId === member.credentialId ? { ...item, weight } : item));
                                }}
                                onBlur={() => {
                                  const current = members.find((item) => item.credentialId === member.credentialId);
                                  if (current) void saveMember(current);
                                }}
                              />
                            </label>
                            <label>
                              <span>P</span>
                              <input
                                type="number"
                                value={member.priority}
                                disabled={busy}
                                onChange={(event) => {
                                  const priority = Number(event.target.value) || 0;
                                  setMembers((prev) => prev.map((item) => item.credentialId === member.credentialId ? { ...item, priority } : item));
                                }}
                                onBlur={() => {
                                  const current = members.find((item) => item.credentialId === member.credentialId);
                                  if (current) void saveMember(current);
                                }}
                              />
                            </label>
                            <label>
                              <span>并发</span>
                              <input
                                type="number"
                                min={1}
                                value={member.concurrencyLimit}
                                disabled={busy}
                                onChange={(event) => {
                                  const concurrencyLimit = Number(event.target.value) || 1;
                                  setMembers((prev) => prev.map((item) => item.credentialId === member.credentialId ? { ...item, concurrencyLimit } : item));
                                }}
                                onBlur={() => {
                                  const current = members.find((item) => item.credentialId === member.credentialId);
                                  if (current) void saveMember(current);
                                }}
                              />
                            </label>
                            <label className="member-knobs__enable">
                              <input
                                type="checkbox"
                                checked={member.enabled}
                                disabled={busy}
                                onChange={(event) => {
                                  const enabled = event.target.checked;
                                  const next = { ...member, enabled };
                                  setMembers((prev) => prev.map((item) => item.credentialId === member.credentialId ? next : item));
                                  void saveMember(next);
                                }}
                              />
                              <span>参与</span>
                            </label>
                          </div>
                        ) : null}
                      </div>
                    );
                  })}
                </div>
              )}
            </section>

            {message && <div className={message.startsWith("已") ? "inline-success" : "inline-warning"}>{message}</div>}
          </div>
        </div>
        <footer className="provider-modal__footer">
          <button type="button" className="button button--secondary" disabled={busy} onClick={() => void remove()}>删除此供应商</button>
          <span>模型发布请到「模型」页开启。</span>
          <button type="button" className="button button--secondary" onClick={onClose}>完成</button>
        </footer>
      </section>
    </div>
  );
}

function ReasoningTable({ route }: { route: ModelRouteSummary }) {
  return (
    <div className="reasoning-card">
      <div className="reasoning-card__header"><strong>{route.reasoningProfile.title}</strong><small>完整八档映射</small></div>
      <div className="mapping-grid" role="table" aria-label={`${route.displayName} 推理映射`}>
        {route.reasoningProfile.mappings.map((mapping) => (
          <div className="mapping-row" role="row" key={mapping.codexEffort}>
            <code>{mapping.codexEffort}</code><span aria-hidden="true">→</span><strong>{mapping.upstreamEffort}</strong><small>{mapping.explanation}</small>
          </div>
        ))}
      </div>
    </div>
  );
}

function ModelsPage({ refreshSnapshot }: { refreshSnapshot: () => Promise<void> }) {
  const [routes, setRoutes] = useState<ModelRouteSummary[]>([]);
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => setRoutes(await listModelRoutes()), []);
  useEffect(() => {
    let active = true;
    void listModelRoutes().then((value) => { if (active) setRoutes(value); });
    return () => { active = false; };
  }, []);
  const filtered = routes.filter((route) => `${route.displayName} ${route.upstreamModel} ${route.providerId}`.toLowerCase().includes(query.toLowerCase()));

  const toggle = async (route: ModelRouteSummary) => {
    setBusyId(route.id);
    setError(null);
    try {
      setRoutes(await setModelEnabled(route.id, !route.enabled));
      await refreshSnapshot();
      // Enabling only updates Spur DB/proxy; Codex GUI needs Apply + cold start.
      setError(
        "已更新选择。若要让 Codex 右下角出现这些模型：请到概览点击「Review & Apply」，然后 Cmd+Q 完全退出 ChatGPT 再打开（关窗口不够）。",
      );
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="page-stack">
      <section className="panel toolbar-panel">
        <div className="search-field"><span aria-hidden="true">⌕</span><input aria-label="搜索模型" placeholder="搜索模型或供应商" value={query} onChange={(event) => setQuery(event.target.value)} /></div>
        <span className="toolbar-note">{routes.filter((route) => route.enabled).length} 已选择 / {routes.length} 已发现</span>
        <button type="button" className="button button--secondary" onClick={() => void reload()}>刷新列表</button>
      </section>
      {error && <div className="inline-warning">{error}</div>}
      <section className="panel">
        {filtered.length === 0 ? (
          <EmptyState title="还没有模型" body="先在概览添加供应商并保存拉取模型，再回来选择要发布到 Codex 的项。" action="等待添加供应商" />
        ) : (
          <div className="model-list">
            {filtered.map((route) => (
              <div className={`model-item ${route.enabled ? "model-item--enabled" : ""}`} key={route.id}>
                <div className="model-row">
                  <label className="switch"><input type="checkbox" checked={route.enabled} disabled={busyId === route.id} onChange={() => void toggle(route)} /><span /></label>
                  <span className="data-row__main"><strong>{route.displayName}</strong><small><code>{route.id}</code> · {route.protocol}</small></span>
                  <span className="badge badge--neutral">{route.providerId}</span>
                  <button type="button" className="button button--ghost" aria-expanded={expanded === route.id} onClick={() => setExpanded(expanded === route.id ? null : route.id)}>推理映射</button>
                </div>
                {expanded === route.id && <ReasoningTable route={route} />}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function UsageMetric({ label, value, note }: { label: string; value: string; note: string }) {
  return <div className="usage-metric"><span>{label}</span><strong>{value}</strong><small>{note}</small></div>;
}

function UsagePage() {
  const [usage, setUsage] = useState<UsageSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try { setUsage(await getUsageSnapshot()); setError(null); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
  }, []);

  useEffect(() => {
    let active = true;
    void getUsageSnapshot().then((value) => { if (active) setUsage(value); }).catch((nextError: unknown) => { if (active) setError(nextError instanceof Error ? nextError.message : String(nextError)); });
    return () => { active = false; };
  }, []);

  if (!usage) return <div className="page-stack"><section className="panel"><div className="empty-state"><strong>正在读取本地用量</strong><p>数据只来自本机代理，不会上传。</p></div></section></div>;
  const format = (value: number) => new Intl.NumberFormat("zh-CN").format(value);
  return <div className="page-stack">
    <section className="panel usage-panel">
      <div className="panel__header"><div><h2>本地代理用量</h2><p>按 Codex Spur 本地代理统计；token 是请求体长度估算值，直到上游返回 usage 才会显示精确值。</p></div><button type="button" className="button button--secondary" onClick={() => void reload()}>刷新</button></div>
      <div className="usage-grid">
        <UsageMetric label="请求数" value={format(usage.requestCount)} note="当前代理进程" />
        <UsageMetric label="今日 token" value={format(usage.todayTokens)} note="本地日统计" />
        <UsageMetric label="总 token" value={format(usage.totalTokens)} note="本地累计" />
        <UsageMetric label="7 日 token" value={format(usage.sevenDayTokens)} note="本地累计暂未分日" />
        <UsageMetric label="缓存命中率" value={usage.cacheHitRate === null ? "暂无数据" : `${(usage.cacheHitRate * 100).toFixed(1)}%`} note="上游返回 usage 后统计" />
      </div>
      <div className="usage-chart" role="img" aria-label="最近请求用量趋势"><div className="usage-chart__bars"><span style={{ height: `${Math.max(12, Math.min(100, usage.requestCount * 8 + 12))}%` }} /><span style={{ height: `${Math.max(12, Math.min(100, usage.inputTokens / 20 + 12))}%` }} /><span style={{ height: `${Math.max(12, Math.min(100, usage.outputTokens / 20 + 12))}%` }} /><span style={{ height: `${Math.max(12, Math.min(100, usage.totalTokens / 20 + 12))}%` }} /></div><div className="usage-chart__labels"><span>请求</span><span>输入</span><span>输出</span><span>合计</span></div></div>
    </section>
    {error && <div className="inline-warning">{error}</div>}
  </div>;
}

function layerLabel(layer: string): string {
  switch (layer) {
    case "fixed":
      return "Fixed";
    case "previous_response":
      return "Previous response";
    case "session":
      return "Session sticky";
    case "load_balance":
      return "Load balance";
    default:
      return layer;
  }
}

function DiagnosticsPage({ snapshot }: { snapshot: AppSnapshot }) {
  const [events, setEvents] = useState<ProxyRequestEvent[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      const next = await listProxyRequestEvents(100);
      setEvents(next);
      setSelectedId((current) => current ?? next[0]?.id ?? null);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void listProxyRequestEvents(100).then((next) => {
      if (!active) return;
      setEvents(next);
      setSelectedId(next[0]?.id ?? null);
    });
    return () => {
      active = false;
    };
  }, []);

  const selected = events.find((event) => event.id === selectedId) ?? null;

  return (
    <div className="page-stack">
      <section className="panel">
        <div className="panel__header">
          <div>
            <h2>运行诊断</h2>
            <p>所有日志脱敏；不显示凭据、token、prompt。右侧为调度命中层。</p>
          </div>
          <div className="form-actions form-actions--wrap">
            <button type="button" className="button button--secondary" disabled={busy} onClick={() => void refresh()}>
              {busy ? "刷新中…" : "刷新"}
            </button>
            <button
              type="button"
              className="button button--secondary"
              disabled={busy || events.length === 0}
              onClick={() => {
                void clearProxyRequestEvents().then(() => {
                  setEvents([]);
                  setSelectedId(null);
                });
              }}
            >
              清空
            </button>
            <StatusDot tone={snapshot.proxy.running ? "healthy" : "error"} />
          </div>
        </div>
        <dl className="diagnostic-grid">
          <div><dt>Proxy</dt><dd>{snapshot.proxy.baseUrl}</dd></div>
          <div><dt>Catalog revision</dt><dd>{snapshot.proxy.catalogRevision}</dd></div>
          <div><dt>Codex home</dt><dd>{snapshot.binding.codexHome}</dd></div>
          <div><dt>Spur catalog path</dt><dd>{snapshot.binding.catalogPath}</dd></div>
          <div><dt>Binding state</dt><dd>{snapshot.binding.state}</dd></div>
          <div><dt>Published models</dt><dd>{snapshot.publishedModels}</dd></div>
        </dl>
      </section>

      <section className="panel diagnostics-split" aria-label="请求调度诊断">
        <div className="diagnostics-split__list">
          <div className="panel__header panel__header--compact">
            <div><h3>请求</h3><p>最近代理请求与调度层</p></div>
          </div>
          {events.length === 0 ? (
            <div className="empty-inline">代理收到 Codex 请求后会在此显示调度决策。</div>
          ) : (
            <ul className="diag-event-list">
              {events.map((event) => (
                <li key={event.id}>
                  <button
                    type="button"
                    className={`diag-event-row${event.id === selectedId ? " diag-event-row--active" : ""}`}
                    onClick={() => setSelectedId(event.id)}
                  >
                    <span className="diag-event-row__time">{event.createdAt}</span>
                    <span className="badge badge--neutral">{layerLabel(event.selectionLayer)}</span>
                    <span className={`badge ${event.resultCategory === "ok" ? "badge--success" : "badge--warning"}`}>
                      {event.resultCategory}
                    </span>
                    <small>{event.accountFingerprint ?? "—"}</small>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="diagnostics-split__detail">
          <div className="panel__header panel__header--compact">
            <div><h3>决策详情</h3><p>选中请求的调度与结果</p></div>
          </div>
          {!selected ? (
            <div className="empty-inline">选择左侧一条请求。</div>
          ) : (
            <dl className="definition-list">
              <div><dt>时间</dt><dd>{selected.createdAt}</dd></div>
              <div><dt>Route / model</dt><dd>{selected.upstreamModel ?? selected.routeSlug ?? "—"}</dd></div>
              <div><dt>Provider</dt><dd>{selected.providerId ?? "—"}</dd></div>
              <div><dt>Protocol</dt><dd>{selected.protocol ?? "—"}</dd></div>
              <div><dt>Scheduler layer</dt><dd>{layerLabel(selected.selectionLayer)}</dd></div>
              <div><dt>Sticky escaped</dt><dd>{selected.stickyEscaped ? "是" : "否"}</dd></div>
              <div><dt>Account fingerprint</dt><dd>{selected.accountFingerprint ?? "—"}</dd></div>
              <div><dt>Result</dt><dd>{selected.resultCategory}</dd></div>
              <div><dt>Failover attempt</dt><dd>{selected.failoverAttempt}</dd></div>
              <div><dt>Latency</dt><dd>{selected.latencyMsTotal != null ? `${selected.latencyMsTotal} ms` : "—"}</dd></div>
              <div><dt>Cooldown applied</dt><dd>{selected.cooldownApplied ? "是" : "否"}</dd></div>
              <div><dt>Error</dt><dd>{selected.errorSummary ?? "—"}</dd></div>
            </dl>
          )}
        </div>
      </section>

      <div className="callout">
        <strong>若 Codex 仍只显示 GPT-5.6 三个模型</strong>
        <p>
          请确认已点击「Review &amp; Apply」，并完全退出后重新打开 Codex。若其他工具（如 CC Switch）会改写
          ~/.codex/config.toml，应用后请检查 model_provider 是否仍为 codex_select。
        </p>
      </div>
      <div className="callout">
        <strong>协议覆盖状态</strong>
        <p>Responses 路由支持透传；Chat Completions 已提供非流式转换骨架，流式 SSE 工具调用转换仍会明确返回未实现错误，不会静默伪装为成功。</p>
      </div>
    </div>
  );
}

function SettingsPage({ providers }: { providers: ProviderSummary[] }) {
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const poolProviders = useMemo(
    () => providers.filter((provider) => provider.credentialCount > 0 && provider.activePoolId),
    [providers],
  );
  const [selectedProviderId, setSelectedProviderId] = useState<string>("");
  const effectiveProviderId =
    selectedProviderId && poolProviders.some((item) => item.id === selectedProviderId)
      ? selectedProviderId
      : (poolProviders[0]?.id ?? "");
  const [schedulerConfig, setSchedulerConfig] = useState<PoolSchedulerConfig>(defaultSchedulerConfig);
  const [diagMax, setDiagMax] = useState(200);

  useEffect(() => {
    let active = true;
    void getDiagnosticsMaxEvents().then((value) => {
      if (active) setDiagMax(value);
    });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    const provider = poolProviders.find((item) => item.id === effectiveProviderId);
    if (!provider?.activePoolId) {
      return;
    }
    let active = true;
    void getPoolSchedulerConfig(provider.activePoolId).then((config) => {
      if (active) setSchedulerConfig(config);
    });
    return () => {
      active = false;
    };
  }, [poolProviders, effectiveProviderId]);

  const restore = async () => {
    try {
      const path = await restorePreviousCodexConfig();
      setMessage(path ? `已从 ${path} 恢复。请完全退出并重新打开 Codex 后生效。` : "没有可恢复的 Codex 配置备份。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  const saveScheduler = async () => {
    const provider = poolProviders.find((item) => item.id === effectiveProviderId);
    if (!provider?.activePoolId) {
      setMessage("请选择有账号池的供应商实例。");
      return;
    }
    setBusy(true);
    setMessage(null);
    try {
      const next = await updatePoolSchedulerConfig(provider.activePoolId, schedulerConfig);
      setSchedulerConfig(next);
      setMessage("已保存高级调度设置。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const saveDiagMax = async () => {
    setBusy(true);
    try {
      const next = await setDiagnosticsMaxEvents(diagMax);
      setDiagMax(next);
      setMessage(`诊断保留条数已设为 ${next}。`);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page-stack">
      <section className="panel">
        <div className="panel__header">
          <div>
            <h2>Codex 配置恢复</h2>
            <p>每次应用前都会保存原 config.toml，且不会覆盖其他 provider。</p>
          </div>
          <button type="button" className="button button--secondary" onClick={() => void restore()}>
            恢复最近备份
          </button>
        </div>
      </section>

      <section className="panel" aria-label="高级调度">
        <div className="panel__header">
          <div>
            <h2>调度（高级）</h2>
            <p>Top-K、sticky TTL、score weights、escape、429 冷却。日常 Pool/Fixed 与账号 weight 仍在供应商编辑里。</p>
          </div>
        </div>
        {poolProviders.length === 0 ? (
          <div className="empty-inline">暂无带账号的供应商实例。导入多账号后可在此调高级参数。</div>
        ) : (
          <>
            <label className="field">
              <span>供应商实例</span>
              <select value={effectiveProviderId} onChange={(event) => setSelectedProviderId(event.target.value)}>
                {poolProviders.map((provider) => (
                  <option key={provider.id} value={provider.id}>
                    {provider.name} ({provider.credentialCount} 账号)
                  </option>
                ))}
              </select>
            </label>
            <div className="scheduler-grid">
              <label className="field"><span>Top-K</span><input type="number" min={1} max={64} value={schedulerConfig.lbTopK} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, lbTopK: Number(e.target.value) || 1 })} /></label>
              <label className="field"><span>换号次数</span><input type="number" min={1} max={10} value={schedulerConfig.maxFailoverSwitches} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, maxFailoverSwitches: Number(e.target.value) || 1 })} /></label>
              <label className="field"><span>Session sticky TTL 秒</span><input type="number" min={60} value={schedulerConfig.stickySessionTtlSecs} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, stickySessionTtlSecs: Number(e.target.value) || 60 })} /></label>
              <label className="field"><span>Response sticky TTL 秒</span><input type="number" min={60} value={schedulerConfig.stickyResponseIdTtlSecs} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, stickyResponseIdTtlSecs: Number(e.target.value) || 60 })} /></label>
              <label className="field"><span>429 冷却秒</span><input type="number" min={1} value={schedulerConfig.default429CooldownSecs} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, default429CooldownSecs: Number(e.target.value) || 1 })} /></label>
              <label className="field"><span>Lease TTL 秒</span><input type="number" min={60} value={schedulerConfig.leaseTtlSecs} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, leaseTtlSecs: Number(e.target.value) || 60 })} /></label>
              <label className="field field--check"><span>Sticky escape</span><input type="checkbox" checked={schedulerConfig.stickyEscape.enabled} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, stickyEscape: { ...schedulerConfig.stickyEscape, enabled: e.target.checked } })} /></label>
              <label className="field field--check"><span>Prefer soonest reset</span><input type="checkbox" checked={schedulerConfig.preferSoonestReset} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, preferSoonestReset: e.target.checked })} /></label>
              <label className="field"><span>Escape TTFT ms</span><input type="number" min={0} value={schedulerConfig.stickyEscape.ttftMs} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, stickyEscape: { ...schedulerConfig.stickyEscape, ttftMs: Number(e.target.value) || 0 } })} /></label>
              <label className="field"><span>Escape error rate</span><input type="number" min={0} max={1} step={0.05} value={schedulerConfig.stickyEscape.errorRate} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, stickyEscape: { ...schedulerConfig.stickyEscape, errorRate: Number(e.target.value) || 0 } })} /></label>
              <label className="field"><span>W·priority</span><input type="number" min={0} step={0.1} value={schedulerConfig.scoreWeights.priority} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, scoreWeights: { ...schedulerConfig.scoreWeights, priority: Number(e.target.value) || 0 } })} /></label>
              <label className="field"><span>W·load</span><input type="number" min={0} step={0.1} value={schedulerConfig.scoreWeights.load} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, scoreWeights: { ...schedulerConfig.scoreWeights, load: Number(e.target.value) || 0 } })} /></label>
              <label className="field"><span>W·queue</span><input type="number" min={0} step={0.1} value={schedulerConfig.scoreWeights.queue} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, scoreWeights: { ...schedulerConfig.scoreWeights, queue: Number(e.target.value) || 0 } })} /></label>
              <label className="field"><span>W·error</span><input type="number" min={0} step={0.1} value={schedulerConfig.scoreWeights.errorRate} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, scoreWeights: { ...schedulerConfig.scoreWeights, errorRate: Number(e.target.value) || 0 } })} /></label>
              <label className="field"><span>W·ttft</span><input type="number" min={0} step={0.1} value={schedulerConfig.scoreWeights.ttft} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, scoreWeights: { ...schedulerConfig.scoreWeights, ttft: Number(e.target.value) || 0 } })} /></label>
              <label className="field"><span>W·reset</span><input type="number" min={0} step={0.1} value={schedulerConfig.scoreWeights.reset} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, scoreWeights: { ...schedulerConfig.scoreWeights, reset: Number(e.target.value) || 0 } })} /></label>
              <label className="field"><span>W·quota</span><input type="number" min={0} step={0.1} value={schedulerConfig.scoreWeights.quotaHeadroom} onChange={(e) => setSchedulerConfig({ ...schedulerConfig, scoreWeights: { ...schedulerConfig.scoreWeights, quotaHeadroom: Number(e.target.value) || 0 } })} /></label>
            </div>
            <div className="form-actions">
              <button type="button" className="button button--primary" disabled={busy} onClick={() => void saveScheduler()}>保存调度设置</button>
              <button type="button" className="button button--secondary" disabled={busy} onClick={() => setSchedulerConfig(defaultSchedulerConfig())}>恢复默认</button>
            </div>
          </>
        )}
      </section>

      <section className="panel" aria-label="诊断保留">
        <div className="panel__header">
          <div>
            <h2>诊断</h2>
            <p>代理请求事件环形保留条数（50–1000）。</p>
          </div>
        </div>
        <div className="form-actions form-actions--wrap">
          <label className="field">
            <span>最大条数</span>
            <input type="number" min={50} max={1000} value={diagMax} onChange={(event) => setDiagMax(Number(event.target.value) || 200)} />
          </label>
          <button type="button" className="button button--secondary" disabled={busy} onClick={() => void saveDiagMax()}>保存</button>
        </div>
      </section>

      {message && <div className={message.startsWith("已") ? "inline-success" : "inline-warning"}>{message}</div>}
      <div className="callout"><strong>桌面端原则</strong><p>关闭主窗口只隐藏应用；菜单栏代理继续运行。只有“退出应用”会停止代理。不会注入或修改 ChatGPT.app。</p></div>
    </div>
  );
}

type ToastTone = "success" | "error" | "warning";
type ToastItem = { id: number; tone: ToastTone; message: string };

function ToastStack({ toasts, onDismiss }: { toasts: ToastItem[]; onDismiss: (id: number) => void }) {
  if (toasts.length === 0) return null;
  return (
    <div className="toast-stack" aria-live="polite" aria-relevant="additions text">
      {toasts.map((toast) => (
        <div className={`toast toast--${toast.tone}`} role="status" key={toast.id}>
          <span>{toast.message}</span>
          <button type="button" className="toast__close" aria-label="关闭通知" onClick={() => onDismiss(toast.id)}>
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

export default function App() {
  const [section, setSection] = useState<NavigationSection>("overview");
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [editProvider, setEditProvider] = useState<ProviderSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [applying, setApplying] = useState(false);
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const toastIdRef = useRef(0);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((item) => item.id !== id));
  }, []);

  const pushToast = useCallback(
    (tone: ToastTone, message: string) => {
      const id = ++toastIdRef.current;
      setToasts((prev) => {
        if (prev.some((item) => item.message === message)) return prev;
        return [...prev.slice(-3), { id, tone, message }];
      });
      window.setTimeout(() => dismissToast(id), 5200);
    },
    [dismissToast],
  );

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      setSnapshot(await getAppSnapshot());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void getAppSnapshot().then((value) => {
      if (active) {
        setSnapshot(value);
        setLoading(false);
      }
    });
    return () => {
      active = false;
    };
  }, []);

  const tone = useMemo(() => (snapshot ? statusTone(snapshot) : "muted"), [snapshot]);

  const applyNow = useCallback(async () => {
    if (applying) return;
    setApplying(true);
    try {
      const preview = await previewCodexApply();
      if (preview.modelCount === 0) {
        pushToast("error", "当前没有已启用模型。请到「模型」页勾选后再应用。");
        return;
      }
      const outcome = await applyCodexConfig();
      await refresh();
      const labels = outcome.modelLabels ?? [];
      const hasKimi = labels.some((label) => /kimi|k3/i.test(label));
      const pathNote = outcome.configPath.includes(".codex")
        ? "已写入 ~/.codex"
        : `已写入 ${outcome.configPath}`;
      const listed =
        labels.length > 0
          ? labels.slice(0, 8).join(" · ") + (labels.length > 8 ? " …" : "")
          : `${outcome.modelCount} 个模型`;
      const kimiNote = hasKimi ? "（含 Kimi）" : "";
      const chatgptStillRunning = (outcome.warnings ?? []).some((warning) =>
        /仍在运行|Cmd\+Q|完全退出/.test(warning),
      );
      if (chatgptStillRunning) {
        pushToast(
          "success",
          `${pathNote}${kimiNote}：已发布 ${outcome.modelCount} 个模型（${listed}）。`,
        );
        // Catalog is loaded only at ChatGPT cold start — only nag when it is still running.
        pushToast(
          "error",
          "重要：请现在 Cmd+Q 退出 ChatGPT（不要只关窗口）。不重启则模型选择器仍为空或显示旧列表。",
        );
      } else {
        pushToast(
          "success",
          `${pathNote}${kimiNote}：已发布 ${outcome.modelCount} 个模型（${listed}）。ChatGPT 未在运行，下次打开即可加载新列表。`,
        );
      }
      for (const warning of outcome.warnings ?? []) {
        if (/仍在运行|Cmd\+Q|完全退出/.test(warning)) {
          // Already surfaced as the hard error toast above.
          continue;
        }
        pushToast("warning", warning);
      }
    } catch (error) {
      pushToast("error", error instanceof Error ? error.message : String(error));
    } finally {
      setApplying(false);
    }
  }, [applying, pushToast, refresh]);

  if (!snapshot) return <main className="boot-state">正在启动本地代理与数据库…</main>;
  const title = NAVIGATION.find((item) => item.id === section)?.label ?? "Codex Spur";

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <img className="brand__mark" src={brandIcon} alt="" />
          <span>
            <strong>Codex Spur</strong>
            <small>Model Router</small>
          </span>
        </div>
        <nav aria-label="主导航">
          {NAVIGATION.map((item) => (
            <button
              key={item.id}
              type="button"
              className={`nav-item ${section === item.id ? "nav-item--active" : ""}`}
              onClick={() => setSection(item.id)}
            >
              <span aria-hidden="true">{item.icon}</span>
              {item.label}
            </button>
          ))}
        </nav>
        <div className="sidebar__footer">
          <div className="proxy-status">
            <StatusDot tone={tone} />
            <span>
              <strong>{snapshot.proxy.running ? "代理运行中" : "代理已停止"}</strong>
              <small>{snapshot.proxy.baseUrl ?? "未绑定"}</small>
            </span>
          </div>
          <small className="version">v0.1.0 · local only</small>
        </div>
      </aside>
      <main className="workspace">
        <header className="toolbar">
          <div>
            <small>CODEX SPUR</small>
            <h1>{title}</h1>
          </div>
          <div className="toolbar__actions">
            <button type="button" className="icon-button" aria-label="刷新" onClick={() => void refresh()}>
              {loading ? "…" : "↻"}
            </button>
            <button
              type="button"
              className="button button--primary"
              disabled={applying}
              onClick={() => void applyNow()}
            >
              {applying ? "正在应用…" : "Review & Apply"}
            </button>
          </div>
        </header>
        <div className="workspace__content">
          {section === "overview" && (
            <Overview
              snapshot={snapshot}
              onAddProvider={() => setAddOpen(true)}
              onEditProvider={(provider) => setEditProvider(provider)}
            />
          )}
          {section === "models" && <ModelsPage refreshSnapshot={refresh} />}
          {section === "usage" && <UsagePage />}
          {section === "diagnostics" && <DiagnosticsPage snapshot={snapshot} />}
          {section === "settings" && <SettingsPage providers={snapshot.providers} />}
        </div>
      </main>
      <ToastStack toasts={toasts} onDismiss={dismissToast} />
      {addOpen && <AddProviderWizard onClose={() => setAddOpen(false)} onCreated={refresh} />}
      {editProvider && (
        <EditProviderSheet
          key={editProvider.id}
          provider={snapshot.providers.find((item) => item.id === editProvider.id) ?? editProvider}
          onClose={() => setEditProvider(null)}
          onChanged={refresh}
        />
      )}
    </div>
  );
}
