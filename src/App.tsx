import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import brandIcon from "./assets/codex-spur-icon.png";
import {
  applyCodexConfig,
    discoverProviderModels,
  addAccountToPool,
  createAccountPool,
  getAppSnapshot,
    importCredentialsJson,
  listAccountPools,
  listPoolMemberIds,
  listCredentials,
  listModelRoutes,
  removeAccountFromPool,
  getUsageSnapshot,
  previewCodexApply,
    restorePreviousCodexConfig,
  setModelEnabled,
  } from "./api";
import type {
  AccountPoolSummary,
  AppSnapshot,
  ApplyPreview,
  CredentialSummary,
  ModelRouteSummary,
  NavigationSection,
  ProviderSummary,
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
        <small>{provider.region} · {provider.protocol}</small>
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

function Overview({ snapshot, onOpenProviders }: { snapshot: AppSnapshot; onOpenProviders: (provider?: ProviderSummary) => void }) {
  return <div className="page-stack"><section className="metrics-grid" aria-label="运行摘要"><Metric label="本地代理" value={snapshot.proxy.running ? "运行中" : "已停止"} note={snapshot.proxy.baseUrl ?? "未绑定"} /><Metric label="Codex 绑定" value={snapshot.binding.state === "applied" ? "已应用" : "待应用"} note={snapshot.binding.providerId} /><Metric label="已发布模型" value={String(snapshot.publishedModels)} note="右下角模型选择器" /><Metric label="健康账号" value={String(snapshot.healthyAccounts)} note="可参与调度" /></section><section className="panel"><div className="panel__header"><div><h2>需要处理</h2><p>只列出会阻止路由或应用的问题。</p></div><span className="badge badge--warning">{snapshot.attentionItems.length}</span></div><div className="attention-list">{snapshot.attentionItems.length === 0 ? <div className="attention-item attention-item--ok">当前没有需要处理的问题。</div> : snapshot.attentionItems.map((item) => <div className="attention-item" key={item}><span aria-hidden="true">!</span><p>{item}</p></div>)}</div></section><section className="panel"><div className="panel__header"><div><h2>供应商</h2><p>配置供应商后，模型会出现在供应商下方，可逐个控制是否显示在 Codex。</p></div><button type="button" className="button button--secondary" onClick={() => onOpenProviders()}>配置供应商</button></div><div className="rows">{snapshot.providers.map((provider) => <ProviderRow key={provider.id} provider={provider} onSelect={() => onOpenProviders(provider)} />)}</div></section></div>;
}

const PROVIDER_SOURCE_LABELS: Record<string, string> = {
  openai: "官方账号 / API Key",
  kimi: "API Key",
  deepseek: "API Key",
  minimax: "API Key",
  custom: "API Key",
};

function providerUrl(provider: ProviderSummary): string {
  if (provider.id === "kimi" && (!provider.baseUrl || provider.baseUrl.includes("www.kimi.com/code") || provider.baseUrl.includes("api.moonshot.cn"))) {
    return "https://api.kimi.com/coding/v1";
  }
  return provider.baseUrl ?? provider.defaultBaseUrl ?? "";
}

function ProviderConfigModal({ providers, provider, onClose, onChanged }: { providers: ProviderSummary[]; provider: ProviderSummary; onClose: () => void; onChanged: () => Promise<void> }) {
  const fileRef = useRef<HTMLInputElement>(null);
  const [selectedId, setSelectedId] = useState(provider.id);
  const selectedProvider = providers.find((item) => item.id === selectedId) ?? provider;
  const [source, setSource] = useState<"official" | "apiKey">(selectedProvider.id === "openai" ? "official" : "apiKey");
  const [baseUrl, setBaseUrl] = useState(providerUrl(selectedProvider));
  const [apiKey, setApiKey] = useState("");
  const [accounts, setAccounts] = useState<CredentialSummary[]>([]);
  const [pools, setPools] = useState<AccountPoolSummary[]>([]);
  const [selectedPoolId, setSelectedPoolId] = useState(`default-${selectedProvider.id}`);
  const [memberIds, setMemberIds] = useState<string[]>([]);
  const [newPoolName, setNewPoolName] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const loadProviderData = useCallback(async () => {
    const [nextAccounts, nextPools] = await Promise.all([listCredentials(selectedProvider.id), listAccountPools()]);
    const providerPools = nextPools.filter((pool) => pool.providerId === selectedProvider.id);
    const nextPoolId = providerPools.some((pool) => pool.id === selectedPoolId)
      ? selectedPoolId
      : providerPools[0]?.id ?? `default-${selectedProvider.id}`;
    const nextMemberIds = nextPoolId ? await listPoolMemberIds(nextPoolId) : [];
    return { accounts: nextAccounts, pools: providerPools, poolId: nextPoolId, memberIds: nextMemberIds };
  }, [selectedProvider.id, selectedPoolId]);

  useEffect(() => {
    let active = true;
    void loadProviderData().then((data) => {
      if (!active) return;
      setAccounts(data.accounts);
      setPools(data.pools);
      setSelectedPoolId(data.poolId);
      setMemberIds(data.memberIds);
    });
    return () => { active = false; };
  }, [loadProviderData]);

  const selectProvider = (id: string) => {
    setSelectedId(id);
    const nextProvider = providers.find((item) => item.id === id) ?? provider;
    setSource(nextProvider.id === "openai" ? "official" : "apiKey");
    setBaseUrl(providerUrl(nextProvider));
    setSelectedPoolId(`default-${nextProvider.id}`);
    setMessage(null);
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") onClose(); };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const changePool = async (poolId: string) => {
    setSelectedPoolId(poolId);
    setMemberIds(await listPoolMemberIds(poolId));
  };

  const importFile = async (file: File) => {
    setMessage(null);
    try {
      const imported = await importCredentialsJson(selectedProvider.id, await file.text());
      setMessage(`已安全导入 ${imported.length} 个账号，并加入默认账号池。`);
      const data = await loadProviderData();
      setAccounts(data.accounts);
      setPools(data.pools);
      setSelectedPoolId(data.poolId);
      setMemberIds(data.memberIds);
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      if (fileRef.current) fileRef.current.value = "";
    }
  };

  const configure = async () => {
    if (source === "apiKey" && !baseUrl.trim()) {
      setMessage("请填写 Base URL。");
      return;
    }
    setBusy(true);
    setMessage(null);
    try {
      const routes = await discoverProviderModels(selectedProvider.id, source === "official" ? "" : baseUrl, source === "apiKey" ? apiKey : undefined);
      setApiKey("");
      setMessage(`已配置 ${selectedProvider.name}，拉取 ${routes.filter((route) => route.providerId === selectedProvider.id).length} 个模型。`);
      const data = await loadProviderData();
      setAccounts(data.accounts);
      setPools(data.pools);
      setSelectedPoolId(data.poolId);
      setMemberIds(data.memberIds);
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const toggleMember = async (accountId: string, checked: boolean) => {
    if (!selectedPoolId) return;
    try {
      if (checked) await addAccountToPool(selectedPoolId, accountId);
      else await removeAccountFromPool(selectedPoolId, accountId);
      setMemberIds((current) => checked ? [...new Set([...current, accountId])] : current.filter((id) => id !== accountId));
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  const createPool = async () => {
    const name = newPoolName.trim();
    if (!name) return;
    try {
      const id = await createAccountPool(selectedProvider.id, name);
      setNewPoolName("");
      const data = await loadProviderData();
      setAccounts(data.accounts);
      setPools(data.pools);
      setSelectedPoolId(data.poolId);
      setMemberIds(data.memberIds);
      await changePool(id);
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <section className="provider-modal" role="dialog" aria-modal="true" aria-labelledby="provider-config-title">
        <header className="provider-modal__header">
          <div><small>CONFIGURE PROVIDER</small><h2 id="provider-config-title">配置供应商</h2><p>选择供应商、保存连接，并管理它的账号池。</p></div>
          <button type="button" className="icon-button" aria-label="关闭配置供应商" onClick={onClose}>×</button>
        </header>
        <div className="provider-modal__body">
          <div className="provider-picker" aria-label="选择供应商">
            {providers.map((item) => <button key={item.id} type="button" className={`provider-picker__item ${item.id === selectedProvider.id ? "provider-picker__item--active" : ""}`} onClick={() => selectProvider(item.id)}>
              <span className="provider-mark" aria-hidden="true">{item.name.slice(0, 1)}</span><span><strong>{item.name}</strong><small>{PROVIDER_SOURCE_LABELS[item.id] ?? "API Key"}</small></span>
            </button>)}
          </div>
          <div className="provider-modal__content">
            <section className="modal-section">
              <div className="modal-section__header"><div><h3>{selectedProvider.name}</h3><p>{selectedProvider.protocol} · {selectedProvider.region}</p></div><span className={`badge ${selectedProvider.configured ? "badge--success" : "badge--neutral"}`}>{selectedProvider.configured ? "已配置" : "未配置"}</span></div>
              {selectedProvider.id === "openai" && <div className="segmented-control" role="tablist" aria-label="OpenAI 凭据来源"><button type="button" className={source === "official" ? "segmented-control__item--active" : ""} onClick={() => { setSource("official"); setBaseUrl(selectedProvider.defaultBaseUrl ?? "https://chatgpt.com/backend-api/codex"); }}>官方账号 account.json</button><button type="button" className={source === "apiKey" ? "segmented-control__item--active" : ""} onClick={() => { setSource("apiKey"); setBaseUrl("https://api.openai.com/v1"); }}>OpenAI API Key</button></div>}
              {source === "official" ? <div className="callout"><strong>官方账号通道</strong><p>导入官方 account.json 后，模型发现会使用官方 Codex 账号通道。Base URL 已自动预置，无需手填。</p><button type="button" className="button button--secondary" onClick={() => fileRef.current?.click()}>选择 account.json</button></div> : <>
                <label className="field"><span>Base URL</span><input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://provider.example.com/v1" spellCheck={false} /></label>
                <label className="field"><span>API Key（可选，保存后加密）</span><input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder="只写入本地加密账号池，不返回前端" autoComplete="off" /></label>
                <div className="form-actions"><button type="button" className="button button--primary" disabled={busy} onClick={() => void configure()}>{busy ? "正在保存并拉取…" : "保存并拉取模型"}</button><button type="button" className="button button--secondary" onClick={() => fileRef.current?.click()}>导入 JSON</button></div>
              </>}
              <input ref={fileRef} className="visually-hidden" type="file" accept=".json,application/json" onChange={(event) => { const file = event.target.files?.[0]; if (file) void importFile(file); }} />
            </section>
            <section className="modal-section">
              <div className="modal-section__header"><div><h3>账号池</h3><p>配置完后可以继续加入或移出账号。</p></div><span className="badge badge--neutral">{accounts.length} 个账号</span></div>
              <div className="pool-toolbar"><select className="select-control" value={selectedPoolId} onChange={(event) => void changePool(event.target.value)}>{pools.map((pool) => <option key={pool.id} value={pool.id}>{pool.name} · {pool.accountCount} 个账号</option>)}</select><input value={newPoolName} onChange={(event) => setNewPoolName(event.target.value)} placeholder="新账号池名称" /><button type="button" className="button button--secondary" disabled={!newPoolName.trim()} onClick={() => void createPool()}>新建池</button></div>
              {accounts.length === 0 ? <div className="empty-inline">还没有账号。可以导入 JSON 或保存 API Key。</div> : <div className="modal-account-list">{accounts.map((account) => <label className="modal-account-row" key={account.id}><input type="checkbox" checked={memberIds.includes(account.id)} onChange={(event) => void toggleMember(account.id, event.target.checked)} /><StatusDot tone={account.healthy ? "healthy" : "error"} /><span><strong>{account.label ?? account.maskedEmail ?? account.maskedAccountId ?? account.fingerprintPrefix}</strong><small>{account.kind} · {account.refreshable ? "可刷新" : "仅访问"}</small></span><span className={`badge ${account.healthy ? "badge--success" : "badge--error"}`}>{account.healthy ? "可用" : "失效"}</span></label>)}</div>}
            </section>
            {message && <div className={message.startsWith("已") ? "inline-success" : "inline-warning"}>{message}</div>}
          </div>
        </div>
        <footer className="provider-modal__footer"><span>模型发现后仍需在供应商页面开启要显示在 Codex 中的模型。</span><button type="button" className="button button--secondary" onClick={onClose}>完成</button></footer>
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
          <EmptyState title="还没有模型" body="先在供应商页输入 Base URL，拉取实时模型列表，再回来选择要发布的模型。" action="等待供应商配置" />
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

function DiagnosticsPage({ snapshot }: { snapshot: AppSnapshot }) {
  return (
    <div className="page-stack">
      <section className="panel">
        <div className="panel__header"><div><h2>运行诊断</h2><p>所有日志必须脱敏；这里不显示凭据或代理 bearer token。</p></div><StatusDot tone={snapshot.proxy.running ? "healthy" : "error"} /></div>
        <dl className="diagnostic-grid">
          <div><dt>Proxy</dt><dd>{snapshot.proxy.baseUrl}</dd></div><div><dt>Catalog revision</dt><dd>{snapshot.proxy.catalogRevision}</dd></div>
          <div><dt>Codex home</dt><dd>{snapshot.binding.codexHome}</dd></div><div><dt>Catalog</dt><dd>{snapshot.binding.catalogPath}</dd></div>
        </dl>
      </section>
      <div className="callout"><strong>协议覆盖状态</strong><p>Responses 路由支持透传；Chat Completions 已提供非流式转换骨架，流式 SSE 工具调用转换仍会明确返回未实现错误，不会静默伪装为成功。</p></div>
    </div>
  );
}

function SettingsPage() {
  const [message, setMessage] = useState<string | null>(null);
  const restore = async () => {
    try {
      const path = await restorePreviousCodexConfig();
      setMessage(path ? `已从 ${path} 恢复。退出并重新登录 Codex 后生效。` : "没有可恢复的 Codex 配置备份。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };
  return (
    <div className="page-stack">
      <section className="panel">
        <div className="panel__header"><div><h2>Codex 配置恢复</h2><p>每次应用前都会保存原 config.toml，且不会覆盖其他 provider。</p></div><button type="button" className="button button--secondary" onClick={() => void restore()}>恢复最近备份</button></div>
        {message && <div className="inline-warning">{message}</div>}
      </section>
      <div className="callout"><strong>桌面端原则</strong><p>关闭主窗口只隐藏应用；菜单栏代理继续运行。只有“退出应用”会停止代理。不会注入或修改 ChatGPT.app。</p></div>
    </div>
  );
}

function ApplyInspector({ preview, onClose, onApplied }: { preview: ApplyPreview; onClose: () => void; onApplied: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const apply = async () => {
    setBusy(true);
    setMessage(null);
    try {
      const outcome = await applyCodexConfig();
      setMessage(`已写入 ${outcome.catalogPath}。请退出并重新登录 Codex。`);
      await onApplied();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };
  return (
    <aside className="inspector" aria-label="应用预览">
      <div className="inspector__header"><div><small>APPLY PREVIEW</small><h2>应用到 Codex</h2></div><button type="button" className="icon-button" aria-label="关闭应用预览" onClick={onClose}>×</button></div>
      <div className="inspector__body">
        <dl className="definition-list"><div><dt>Provider</dt><dd>{preview.providerId}</dd></div><div><dt>Proxy</dt><dd>{preview.baseUrl}</dd></div><div><dt>Catalog</dt><dd>{preview.catalogPath}</dd></div><div><dt>Models</dt><dd>{preview.modelCount}</dd></div></dl>
        <div className="code-preview"><pre>{preview.tomlPreview}</pre></div>
        {preview.warnings.map((warning) => <div className="inline-warning" key={warning}>{warning}</div>)}
        {message && <div className={message.startsWith("已写入") ? "inline-success" : "inline-warning"}>{message}</div>}
      </div>
      <div className="inspector__footer"><button type="button" className="button button--secondary" onClick={onClose}>关闭</button><button type="button" className="button button--primary" disabled={preview.modelCount === 0 || busy} onClick={() => void apply()}>{busy ? "正在应用…" : "验证并应用"}</button></div>
    </aside>
  );
}

export default function App() {
  const [section, setSection] = useState<NavigationSection>("overview");
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [preview, setPreview] = useState<ApplyPreview | null>(null);
  const [configProvider, setConfigProvider] = useState<ProviderSummary | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    try { setSnapshot(await getAppSnapshot()); } finally { setLoading(false); }
  }, []);

  useEffect(() => {
    let active = true;
    void getAppSnapshot().then((value) => {
      if (active) {
        setSnapshot(value);
        setLoading(false);
      }
    });
    return () => { active = false; };
  }, []);
  const tone = useMemo(() => (snapshot ? statusTone(snapshot) : "muted"), [snapshot]);
  const openPreview = useCallback(async () => setPreview(await previewCodexApply()), []);

  if (!snapshot) return <main className="boot-state">正在启动本地代理与数据库…</main>;
  const title = NAVIGATION.find((item) => item.id === section)?.label ?? "Codex Spur";

  return (
    <div className={`app-shell ${preview ? "app-shell--inspector" : ""}`}>
      <aside className="sidebar">
        <div className="brand"><img className="brand__mark" src={brandIcon} alt="" /><span><strong>Codex Spur</strong><small>Model Router</small></span></div>
        <nav aria-label="主导航">{NAVIGATION.map((item) => <button key={item.id} type="button" className={`nav-item ${section === item.id ? "nav-item--active" : ""}`} onClick={() => setSection(item.id)}><span aria-hidden="true">{item.icon}</span>{item.label}</button>)}</nav>
        <div className="sidebar__footer"><div className="proxy-status"><StatusDot tone={tone} /><span><strong>{snapshot.proxy.running ? "代理运行中" : "代理已停止"}</strong><small>{snapshot.proxy.baseUrl ?? "未绑定"}</small></span></div><small className="version">v0.1.0 · local only</small></div>
      </aside>
      <main className="workspace">
        <header className="toolbar"><div><small>CODEX SPUR</small><h1>{title}</h1></div><div className="toolbar__actions"><button type="button" className="icon-button" aria-label="刷新" onClick={() => void refresh()}>{loading ? "…" : "↻"}</button><button type="button" className="button button--primary" onClick={() => void openPreview()}>Review & Apply</button></div></header>
        <div className="workspace__content">
          {section === "overview" && <Overview snapshot={snapshot} onOpenProviders={(provider) => setConfigProvider(provider ?? snapshot.providers[0] ?? null)} />}
          {section === "models" && <ModelsPage refreshSnapshot={refresh} />}
          {section === "usage" && <UsagePage />}
          {section === "diagnostics" && <DiagnosticsPage snapshot={snapshot} />}
          {section === "settings" && <SettingsPage />}
        </div>
      </main>
      {preview && <ApplyInspector preview={preview} onClose={() => setPreview(null)} onApplied={refresh} />}
      {configProvider && <ProviderConfigModal key={configProvider.id} providers={snapshot.providers} provider={configProvider} onClose={() => setConfigProvider(null)} onChanged={refresh} />}
    </div>
  );
}
