import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import brandIcon from "./assets/codex-spur-icon.png";
import modelPickerShot from "./assets/codex-model-picker.png";
import {
  applyCodexConfig,
  cancelOpenAiBrowserLogin,
  disableKimiPublish,
  enableKimiPublish,
  cancelXaiDeviceLogin,
  clearProxyRequestEvents,
  completeOpenAiOauthCallbackUrl,
  completeXaiDeviceLogin,
  createProviderInstance,
  deleteCredential,
  deleteProviderInstance,
  discoverProviderModels,
  getAppSnapshot,
  getCachedOpenAiQuota,
  getDiagnosticsMaxEvents,
  getPoolSchedulerConfig,
  getProviderRouting,
  importCredentialsJson,
  importOpenCodeGoCredential,
  inspectOpenCodeGoCredential,
  importSessionJson,
  importProviderConfigJson,
  kimiTargetStatus,
  listCredentials,
  listModelRoutes,
  listPoolMembersDetailed,
  listProxyRequestEvents,
  openExternalUrl,
  pollXaiDeviceLogin,
  previewCodexApply,
  refreshOpenAiQuota,
  renameCredential,
  renameProviderInstance,
  restorePreviousCodexConfig,
  setDiagnosticsMaxEvents,
  setModelEnabled,
  setProviderRouting,
  startOpenAiBrowserLogin,
  startXaiDeviceLogin,
  updatePoolMember,
  updatePoolSchedulerConfig,
} from "./api";
import type { BrowserLoginStart, DeviceLoginStart, OpenAiOAuthFinishedEvent } from "./api";
import type {
  AppSnapshot,
  CredentialSummary,
  ModelRouteSummary,
  NavigationSection,
  OpenAiQuotaSnapshot,
  PoolMemberDetail,
  PoolSchedulerConfig,
  ProviderKind,
  ProviderSummary,
  ProxyRequestEvent,
  QuotaWindow,
  StatusTone,
} from "./types";
import { UsagePage } from "./usage";

const NAVIGATION: Array<{ id: NavigationSection; label: string; icon: string }> = [
  { id: "overview", label: "概览", icon: "◫" },
  { id: "models", label: "模型", icon: "◇" },
  { id: "usage", label: "用量", icon: "▥" },
  { id: "diagnostics", label: "诊断", icon: "⌁" },
  { id: "settings", label: "设置", icon: "⚙" },
];

function statusTone(snapshot: AppSnapshot): StatusTone {
  if (!snapshot.proxy.running || snapshot.binding.state === "invalid") return "error";
  if (
    snapshot.binding.state !== "applied" ||
    snapshot.attentionItems.length > 0 ||
    !snapshot.desktopVisibility.ready
  ) {
    return "warning";
  }
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
  const visibility = snapshot.desktopVisibility;
  const [desktopOpen, setDesktopOpen] = useState(false);
  const [attentionOpen, setAttentionOpen] = useState(false);
  const desktopPanelId = useId();
  const attentionPanelId = useId();
  const failedChecks = visibility.checks.filter((check) => !check.ok);
  const passedChecks = visibility.checks.filter((check) => check.ok);
  const attentionCount = snapshot.attentionItems.length;

  return (
    <div className="page-stack">
      <section className="metrics-grid metrics-grid--5" aria-label="运行摘要">
        <Metric label="本地代理" value={snapshot.proxy.running ? "运行中" : "已停止"} note={snapshot.proxy.baseUrl ?? "未绑定"} />
        <Metric label="Codex 绑定" value={snapshot.binding.state === "applied" ? "已应用" : "待应用"} note={snapshot.binding.providerId} />
        <Metric
          label="Desktop 可见"
          value={visibility.statusLabel}
          note={visibility.ready ? "自定义模型可出现在 GUI" : "缺登录或未应用"}
        />
        <Metric label="已发布模型" value={String(snapshot.publishedModels)} note="右下角高级 → 模型" />
        <Metric label="健康账号" value={String(snapshot.healthyAccounts)} note="可参与调度" />
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
            body="添加 OpenAI（官方订阅 / 账号 JSON / session）或 Kimi / DeepSeek。保存并拉取后会出现在这里。"
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

      <section className="panel panel--disclosure">
        <div className="panel__header panel__header--disclosure">
          <button
            type="button"
            className="panel-disclosure"
            aria-expanded={desktopOpen}
            aria-controls={desktopPanelId}
            onClick={() => setDesktopOpen((open) => !open)}
          >
            <span className="panel-disclosure__chevron" aria-hidden="true">{desktopOpen ? "▾" : "▸"}</span>
            <span className="panel-disclosure__title">
              <strong>Desktop 可见性</strong>
              <span>{desktopOpen ? "收起说明与通过项" : "展开说明与通过项"}</span>
            </span>
          </button>
          <span className={`badge ${visibility.ready ? "badge--success" : "badge--warning"}`}>
            {visibility.statusLabel}
          </span>
        </div>
        {failedChecks.length > 0 ? (
          <div className="readiness-list" role="list" aria-label="Desktop 可见性待处理项">
            {failedChecks.map((check) => (
              <div key={check.id} className="readiness-item readiness-item--bad" role="listitem">
                <span aria-hidden="true">!</span>
                <div>
                  <strong>{check.label}</strong>
                  <p>{check.detail}</p>
                </div>
              </div>
            ))}
          </div>
        ) : null}
        <div id={desktopPanelId} className="panel-disclosure__body" hidden={!desktopOpen}>
          <p className="panel-disclosure__copy">
            ChatGPT 桌面端按官方身份门控自定义 catalog。请先在 <strong>ChatGPT.app</strong> 登录官方账号
            （不是 Spur 的 API Key / 浏览器 OAuth），再 Apply，最后 Cmd+Q 冷启动，在「高级 → 模型」中选择 Kimi / DeepSeek。
          </p>
          {passedChecks.length > 0 ? (
            <div className="readiness-list" role="list" aria-label="Desktop 可见性通过项">
              {passedChecks.map((check) => (
                <div key={check.id} className="readiness-item readiness-item--ok" role="listitem">
                  <span aria-hidden="true">✓</span>
                  <div>
                    <strong>{check.label}</strong>
                    <p>{check.detail}</p>
                  </div>
                </div>
              ))}
            </div>
          ) : null}
          <div className="callout callout--inline">
            <strong>最短路径</strong>
            <p>ChatGPT 官方登录 → 启用模型 → Review &amp; Apply → Cmd+Q 重开 ChatGPT → 高级 → 模型 ›</p>
          </div>
        </div>
      </section>

      <section className="panel panel--disclosure">
        <div className="panel__header panel__header--disclosure">
          <button
            type="button"
            className="panel-disclosure"
            aria-expanded={attentionOpen}
            aria-controls={attentionPanelId}
            onClick={() => setAttentionOpen((open) => !open)}
          >
            <span className="panel-disclosure__chevron" aria-hidden="true">{attentionOpen ? "▾" : "▸"}</span>
            <span className="panel-disclosure__title">
              <strong>需要处理</strong>
              <span>{attentionOpen ? "收起说明" : "展开说明"}</span>
            </span>
          </button>
          <span className={`badge ${attentionCount > 0 ? "badge--warning" : "badge--success"}`}>
            {attentionCount}
          </span>
        </div>
        {attentionCount > 0 ? (
          <div className="attention-list" aria-label="需要处理的问题">
            {snapshot.attentionItems.map((item) => (
              <div className="attention-item" key={item}>
                <span aria-hidden="true">!</span>
                <p>{item}</p>
              </div>
            ))}
          </div>
        ) : null}
        <div id={attentionPanelId} className="panel-disclosure__body" hidden={!attentionOpen}>
          <p className="panel-disclosure__copy">只列出会阻止路由、Apply 或 Desktop 可见性的问题。</p>
          {attentionCount === 0 ? (
            <div className="attention-list">
              <div className="attention-item attention-item--ok">
                <span aria-hidden="true">✓</span>
                <p>当前没有需要处理的问题。</p>
              </div>
            </div>
          ) : null}
        </div>
      </section>
    </div>
  );
}


type AddMethodId =
  | "openai-official"
  | "openai-accounts"
  | "openai-session"
  | "xai-official"
  | "xai-api"
  | "kimi"
  | "deepseek"
  | "minimax"
  | "opencode-go"
  | "custom"
  | "custom-config-json";

/** Entry-method category for list badges: browser / file import / form API key. */
type EntryCategory = "official" | "json" | "api";

type AddMethod = {
  id: AddMethodId;
  kind: ProviderKind;
  title: string;
  hint: string;
  mode: "api" | "configJson" | "accounts" | "session" | "oauth";
  category: EntryCategory;
};

const CATEGORY_BADGE: Record<EntryCategory, string> = {
  official: "官方订阅",
  json: "JSON",
  api: "API",
};

/** Normalize legacy pool/config stamps and unknown values for Overview badges. */
function normalizeEntryCategory(
  raw: ProviderSummary["entryCategory"],
): EntryCategory | null {
  if (raw === "official" || raw === "api" || raw === "json") return raw;
  // Legacy: account-pool import and provider-config import both surface as JSON.
  if (raw === "pool" || raw === "config") return "json";
  return null;
}

function entryCategoryBadge(
  provider: ProviderSummary,
): { category: EntryCategory; label: string } | null {
  let category = normalizeEntryCategory(provider.entryCategory);
  // Browser official login is single-account. Multi-account "official" is a
  // JSON import mis-stamp (and/or legacy oauth kind counting bugs).
  if (category === "official" && provider.credentialCount >= 2) {
    category = "json";
  }
  if (!category) return null;
  return { category, label: CATEGORY_BADGE[category] };
}

function ProviderRow({ provider, onSelect }: { provider: ProviderSummary; onSelect?: () => void }) {
  const entry = entryCategoryBadge(provider);
  return (
    <button className="data-row provider-row" type="button" onClick={onSelect}>
      <span className="provider-mark" aria-hidden="true">{provider.name.slice(0, 1)}</span>
      <span className="data-row__main">
        <strong>{provider.name}</strong>
        <small>{provider.kind} · {provider.region} · {provider.protocol}</small>
      </span>
      <span className="provider-row__badges">
        {entry && (
          <span className={`method-badge method-badge--${entry.category}`}>{entry.label}</span>
        )}
        <span className={`badge ${provider.configured ? "badge--success" : "badge--neutral"}`}>
          {provider.configured ? "已配置" : "未配置"}
        </span>
      </span>
      <span className="provider-count">{provider.selectedModels}/{provider.discoveredModels} 模型 · {provider.healthyCredentialCount}/{provider.credentialCount} 账号</span>
      <span className="chevron" aria-hidden="true">›</span>
    </button>
  );
}

const ADD_METHODS: AddMethod[] = [
  {
    id: "openai-official",
    kind: "openai",
    title: "OpenAI · 官方订阅",
    hint: "浏览器登录 ChatGPT（PKCE）",
    mode: "oauth",
    category: "official",
  },
  {
    id: "openai-accounts",
    kind: "openai",
    title: "OpenAI · 导入账号 JSON",
    hint: "单/多账号 JSON → Agent Identity / OAuth 入库",
    mode: "accounts",
    category: "json",
  },
  {
    id: "openai-session",
    kind: "openai",
    title: "OpenAI · 导入 session 文件",
    hint: "ChatGPT session dump → 注册 Agent Identity",
    mode: "session",
    category: "json",
  },
  {
    id: "xai-official",
    kind: "xai",
    title: "Grok",
    hint: "浏览器登录 xAI / SuperGrok",
    mode: "oauth",
    category: "official",
  },
  {
    id: "xai-api",
    kind: "xai",
    title: "Grok",
    hint: "api.x.ai 密钥",
    mode: "api",
    category: "api",
  },
  {
    id: "kimi",
    kind: "kimi",
    title: "Kimi Code",
    hint: "API Key（coding 端点）",
    mode: "api",
    category: "api",
  },
  {
    id: "deepseek",
    kind: "deepseek",
    title: "DeepSeek",
    hint: "API Key + Base URL",
    mode: "api",
    category: "api",
  },
  {
    id: "minimax",
    kind: "minimax",
    title: "MiniMax",
    hint: "API Key + Base URL",
    mode: "api",
    category: "api",
  },
  {
    id: "opencode-go",
    kind: "opencode-go",
    title: "OpenCode Go",
    hint: "自动导入本机凭据，或手动填写 API Key",
    mode: "api",
    category: "api",
  },
  {
    id: "custom",
    kind: "custom",
    title: "自定义",
    hint: "OpenAI-compatible API",
    mode: "api",
    category: "api",
  },
  {
    id: "custom-config-json",
    kind: "custom",
    title: "自定义 · 导入配置 JSON",
    hint: "供应商配置 JSON（不是账号 JSON）",
    mode: "configJson",
    category: "json",
  },
];

const DEFAULT_BASE_URL: Record<ProviderKind, string> = {
  openai: "https://api.openai.com/v1",
  xai: "https://api.x.ai/v1",
  kimi: "https://api.kimi.com/coding/v1",
  deepseek: "https://api.deepseek.com/v1",
  minimax: "https://api.minimaxi.com/v1",
  "opencode-go": "https://opencode.ai/zen/go/v1",
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

function useEscapeClose(onClose: () => void, enabled = true) {
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [enabled, onClose]);
}

type ConfirmRequest = {
  title: string;
  body: string;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void | Promise<void>;
};

/** In-app confirmation (never window.confirm — blocked/silent inside Tauri modals). */
function ConfirmSheet({
  request,
  busy,
  onCancel,
}: {
  request: ConfirmRequest;
  busy: boolean;
  onCancel: () => void;
}) {
  const titleId = useId();
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    confirmRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        event.stopPropagation();
        onCancel();
      }
    };
    document.addEventListener("keydown", onKeyDown, true);
    return () => document.removeEventListener("keydown", onKeyDown, true);
  }, [busy, onCancel]);

  return (
    <div
      className="confirm-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) onCancel();
      }}
    >
      <section
        className="confirm-sheet"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <header className="confirm-sheet__header">
          <h3 id={titleId}>{request.title}</h3>
          <p>{request.body}</p>
        </header>
        <footer className="confirm-sheet__footer">
          <button type="button" className="button button--secondary" disabled={busy} onClick={onCancel}>
            取消
          </button>
          <button
            ref={confirmRef}
            type="button"
            className={`button ${request.danger ? "button--danger" : "button--primary"}`}
            disabled={busy}
            onClick={() => void request.onConfirm()}
          >
            {busy ? "处理中…" : request.confirmLabel}
          </button>
        </footer>
      </section>
    </div>
  );
}

function resolveAddMethod(id: AddMethodId): AddMethod {
  const found = ADD_METHODS.find((item) => item.id === id);
  if (found) return found;
  return {
    id: "openai-official",
    kind: "openai",
    title: "OpenAI",
    hint: "浏览器登录 ChatGPT · 写入本实例账号",
    mode: "oauth",
    category: "official",
  };
}

function supportsOfficialQuota(account: CredentialSummary): boolean {
  // Official ChatGPT usage windows require a subscription-style credential, not a plain API key.
  // Agent Identity is durable subscription auth without OAuth refresh tokens.
  const kind = account.kind.toLowerCase();
  if (kind === "agent_identity" || kind === "agentidentity") return true;
  return kind !== "api_key" && kind !== "apikey";
}

function describeCredentialKind(account: CredentialSummary): string {
  const kind = account.kind.toLowerCase();
  if (kind === "agent_identity" || kind === "agentidentity") return "Agent Identity";
  if (kind === "oauth" || kind === "o_auth") {
    return account.refreshable ? "OAuth · 可刷新" : "OAuth · 仅访问";
  }
  if (kind === "chatgpt_web_session" || kind === "chat_gpt_web_session") {
    return account.refreshable ? "Web Session · 可刷新" : "Web Session · 仅访问";
  }
  if (kind === "api_key" || kind === "apikey") return "API Key";
  return account.refreshable ? `${account.kind} · 可刷新` : `${account.kind} · 仅访问`;
}

function accountDisplayName(account: CredentialSummary): string {
  return account.label ?? account.maskedEmail ?? account.maskedAccountId ?? account.fingerprintPrefix;
}

function formatQuotaReset(resetAt: number | null): string {
  if (resetAt == null) return "—";
  const date = new Date(resetAt * 1000);
  if (Number.isNaN(date.getTime())) return "—";
  const now = new Date();
  const sameDay =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();
  if (sameDay) {
    return `重置 ${date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}`;
  }
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function describeFetched(fetchedAt: number, nowMs: number): { label: string; stale: boolean } {
  const ageMs = nowMs - fetchedAt * 1000;
  const stale = Number.isFinite(ageMs) && ageMs > 30 * 60 * 1000;
  if (!Number.isFinite(ageMs) || ageMs < 0) return { label: "刚刚", stale: false };
  const minutes = Math.floor(ageMs / 60_000);
  if (minutes < 1) return { label: "刚刚", stale: false };
  if (minutes < 60) return { label: `${minutes} 分钟前`, stale };
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return { label: `${hours} 小时前`, stale };
  return { label: `${Math.floor(hours / 24)} 天前`, stale };
}

function QuotaLine({ label, window }: { label: string; window: QuotaWindow | null | undefined }) {
  if (!window) {
    return (
      <div className="quota-line">
        <strong>{label}</strong>
        <div className="quota-track" aria-hidden="true"><span style={{ width: "0%" }} /></div>
        <span>—</span>
      </div>
    );
  }
  const used = Math.max(0, Math.min(100, window.usedPercent));
  const remaining = Math.max(0, Math.min(100, window.remainingPercent));
  return (
    <div className="quota-line">
      <strong>{label}</strong>
      <div className="quota-track" aria-hidden="true">
        <span style={{ width: `${used}%` }} />
      </div>
      <span title={`${remaining.toFixed(0)}% 剩余 · ${formatQuotaReset(window.resetAt)}`}>
        {used.toFixed(0)}% · {formatQuotaReset(window.resetAt)}
      </span>
    </div>
  );
}

function AccountQuotaBlock({
  account,
  snapshot,
  busy,
  error,
  onRefresh,
  nowMs,
}: {
  account: CredentialSummary;
  snapshot: OpenAiQuotaSnapshot | null | undefined;
  busy: boolean;
  error: string | null | undefined;
  onRefresh: () => void;
  nowMs: number;
}) {
  if (!supportsOfficialQuota(account)) {
    return (
      <div className="account-quota account-quota--modal">
        <p className="account-quota__note">API Key 无官方 5h / 7d 额度。</p>
      </div>
    );
  }

  const fetched = snapshot ? describeFetched(snapshot.fetchedAt, nowMs) : null;
  const credits = snapshot?.resetCredits?.availableCount;

  return (
    <div className="account-quota account-quota--modal">
      <div className="account-quota__rows">
        <QuotaLine label="5h" window={snapshot?.fiveHour ?? null} />
        <QuotaLine label="7d" window={snapshot?.sevenDay ?? null} />
      </div>
      <div className="quota-actions">
        <button type="button" disabled={busy} onClick={onRefresh}>
          {busy ? "刷新中…" : "刷新额度"}
        </button>
        <span>重置卡 {credits == null ? "—" : credits}</span>
        {snapshot && fetched ? (
          <span className={fetched.stale ? "quota-actions__stale" : undefined}>
            {fetched.stale ? "已过时 · " : ""}
            {fetched.label}
            {snapshot.planType ? ` · ${snapshot.planType}` : ""}
          </span>
        ) : (
          <span>尚无缓存，点刷新拉取 5h / 7d</span>
        )}
      </div>
      {error ? <p className="account-quota__error">{error}</p> : null}
    </div>
  );
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
  const [openCodeGoStatus, setOpenCodeGoStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  /** OpenAI official subscription — browser PKCE session (no secrets). */
  const [browserLogin, setBrowserLogin] = useState<BrowserLoginStart | null>(null);
  /** xAI / Grok still uses device-code (user_code). */
  const [deviceLogin, setDeviceLogin] = useState<DeviceLoginStart | null>(null);
  const [loginStatus, setLoginStatus] = useState<string | null>(null);
  const [callbackUrl, setCallbackUrl] = useState("");
  /** Paste buffer for account JSON (text import; never logged). */
  const [accountsJsonText, setAccountsJsonText] = useState("");
  const pollRef = useRef<number | null>(null);
  const finishingLoginRef = useRef(false);

  useEscapeClose(onClose);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<OpenAiOAuthFinishedEvent>("openai-oauth-finished", (event) => {
      if (finishingLoginRef.current) return;
      finishingLoginRef.current = true;
      void (async () => {
        try {
          const payload = event.payload;
          if (!payload.ok) {
            setMessage(payload.message ?? "登录失败，请重试。");
            setBrowserLogin(null);
            setLoginStatus(null);
            setBusy(false);
            return;
          }
          setLoginStatus("登录成功，正在刷新列表…");
          await onCreated();
          if (payload.modelError) {
            setMessage(payload.modelError);
            setBrowserLogin(null);
            setLoginStatus(null);
          } else {
            const name = payload.provider?.name ?? "OpenAI";
            setMessage(`已添加 ${name}，拉取 ${payload.modelCount} 个模型候选。`);
            onClose();
          }
        } finally {
          setBusy(false);
          finishingLoginRef.current = false;
        }
      })();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
      if (pollRef.current !== null) window.clearTimeout(pollRef.current);
      void cancelOpenAiBrowserLogin().catch(() => undefined);
    };
  }, [onClose, onCreated]);

  const selectMethod = (nextId: AddMethodId) => {
    const next = resolveAddMethod(nextId);
    setMethodId(nextId);
    setMessage(null);
    setApiKey("");
    setOpenCodeGoStatus(null);
    if (next.kind === "opencode-go") {
      void inspectOpenCodeGoCredential()
        .then((status) => setOpenCodeGoStatus(`${status.message}（${status.pathLabel}）`))
        .catch((error) => setOpenCodeGoStatus(error instanceof Error ? error.message : String(error)));
    }
    if (browserLogin) {
      void cancelOpenAiBrowserLogin().catch(() => undefined);
    }
    if (deviceLogin) {
      void cancelXaiDeviceLogin(deviceLogin.deviceCode).catch(() => undefined);
    }
    if (pollRef.current !== null) {
      window.clearTimeout(pollRef.current);
      pollRef.current = null;
    }
    setBrowserLogin(null);
    setDeviceLogin(null);
    setLoginStatus(null);
    setCallbackUrl("");
    setBaseUrl(DEFAULT_BASE_URL[next.kind]);
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

  const submitOpenCodeGoImport = async () => {
    setBusy(true);
    setMessage(null);
    let createdId: string | null = null;
    try {
      const created = await createProviderInstance("opencode-go", displayName.trim() || undefined);
      createdId = created.id;
      await importOpenCodeGoCredential(created.id);
      const routes = await discoverProviderModels(created.id, DEFAULT_BASE_URL["opencode-go"], undefined);
      const count = routes.filter((route) => route.providerId === created.id).length;
      await finishCreate(created, count);
    } catch (error) {
      if (createdId) await rollback(createdId);
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
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

  const submitAccountsJson = async (raw: string, clearFile = false, asSession = false) => {
    const input = raw.trim();
    if (!input) {
      setMessage(asSession ? "请粘贴 session JSON，或选择 session 文件。" : "请粘贴账号 JSON，或选择 JSON 文件。");
      return;
    }
    setBusy(true);
    setMessage(null);
    let createdId: string | null = null;
    let accountsImported = false;
    try {
      const created = await createProviderInstance(method.kind, displayName.trim() || undefined);
      createdId = created.id;
      const imported = asSession
        ? await importSessionJson(created.id, input)
        : await importCredentialsJson(created.id, input);
      if (imported.length === 0) {
        throw new Error(asSession ? "未从 session 解析到账号。" : "未解析到任何账号，请检查 JSON。");
      }
      accountsImported = true;
      setAccountsJsonText("");
      try {
        // Empty base_url → official ChatGPT Codex discovery path.
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
      if (clearFile && accountFileRef.current) accountFileRef.current.value = "";
    }
  };

  const submitAccounts = async (file: File) => {
    await submitAccountsJson(await file.text(), true, method.mode === "session");
  };

  const stopPolling = () => {
    if (pollRef.current !== null) {
      window.clearTimeout(pollRef.current);
      pollRef.current = null;
    }
  };

  const scheduleXaiPoll = (deviceCode: string, intervalSecs: number) => {
    stopPolling();
    pollRef.current = window.setTimeout(() => {
      void (async () => {
        try {
          const result = await pollXaiDeviceLogin(deviceCode);
          if (result.status === "pending") {
            setLoginStatus("等待浏览器完成登录…");
            const nextInterval =
              typeof result.intervalSecs === "number" ? result.intervalSecs : intervalSecs;
            scheduleXaiPoll(deviceCode, nextInterval);
            return;
          }
          if (result.status === "success" && result.tokens) {
            setLoginStatus("登录成功，正在保存并拉取模型…");
            setBusy(true);
            const complete = await completeXaiDeviceLogin(
              result.tokens,
              displayName.trim() || undefined,
            );
            await onCreated();
            if (complete.modelError) {
              setMessage(complete.modelError);
              setDeviceLogin(null);
            } else {
              setMessage(`已添加 ${complete.provider.name}，拉取 ${complete.modelCount} 个模型候选。`);
              onClose();
            }
            setBusy(false);
            return;
          }
          setMessage(result.message ?? "登录失败，请重试。");
          setDeviceLogin(null);
          setBusy(false);
        } catch (error) {
          setMessage(error instanceof Error ? error.message : String(error));
          // Keep session alive; retry after a short backoff.
          scheduleXaiPoll(deviceCode, Math.max(intervalSecs, 5));
        }
      })();
    }, Math.max(3, intervalSecs) * 1000);
  };

  const startOpenAiOfficialLogin = async () => {
    setBusy(true);
    setMessage(null);
    setLoginStatus(null);
    finishingLoginRef.current = false;
    try {
      const started = await startOpenAiBrowserLogin(displayName.trim() || undefined);
      setBrowserLogin(started);
      setLoginStatus("已打开浏览器，请在页面用 ChatGPT 账号授权。完成后会自动返回本应用。");
      try {
        await openExternalUrl(started.authUrl);
      } catch {
        // User can open manually.
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
      setBrowserLogin(null);
    } finally {
      setBusy(false);
    }
  };

  const startXaiOfficialLogin = async () => {
    setBusy(true);
    setMessage(null);
    setLoginStatus(null);
    try {
      const started = await startXaiDeviceLogin();
      setDeviceLogin(started);
      setLoginStatus("已打开浏览器，请在 xAI 页面输入下方代码完成 Grok 订阅授权。");
      try {
        await openExternalUrl(started.verificationUri);
      } catch {
        // User can open manually.
      }
      scheduleXaiPoll(started.deviceCode, started.intervalSecs);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const cancelOpenAiLogin = async () => {
    try {
      await cancelOpenAiBrowserLogin();
    } catch {
      // ignore
    }
    setBrowserLogin(null);
    setLoginStatus(null);
    setCallbackUrl("");
  };

  const cancelXaiLogin = async () => {
    stopPolling();
    if (deviceLogin) {
      try {
        await cancelXaiDeviceLogin(deviceLogin.deviceCode);
      } catch {
        // ignore
      }
    }
    setDeviceLogin(null);
    setLoginStatus(null);
  };

  const submitCallbackUrl = async () => {
    const url = callbackUrl.trim();
    if (!url) {
      setMessage("请粘贴浏览器地址栏中的回调链接。");
      return;
    }
    setBusy(true);
    setMessage(null);
    try {
      finishingLoginRef.current = true;
      const complete = await completeOpenAiOauthCallbackUrl(url);
      await onCreated();
      if (complete.modelError) {
        setMessage(complete.modelError);
        setBrowserLogin(null);
        setLoginStatus(null);
      } else {
        setMessage(`已添加 ${complete.provider.name}，拉取 ${complete.modelCount} 个模型候选。`);
        onClose();
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      finishingLoginRef.current = false;
      setBusy(false);
    }
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
                <span className="method-card__title">
                  <strong>{item.title}</strong>
                  <span className={`method-badge method-badge--${item.category}`}>
                    {CATEGORY_BADGE[item.category]}
                  </span>
                </span>
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

            {method.mode === "oauth" && method.kind === "openai" && (
              <section className="modal-section" aria-label="OpenAI 官方订阅登录">
                <div className="callout">
                  <strong>
                    OpenAI
                    <span className="method-badge method-badge--official">官方订阅</span>
                  </strong>
                  <p>
                    <strong>浏览器登录</strong> ChatGPT（与原生 Codex 相同 PKCE 流程）。成功后在本实例写入
                    <strong>一个</strong>官方账号，可查看 5 小时 / 7 天额度，并拉取官方模型。
                  </p>
                  {!browserLogin ? (
                    <button type="button" className="button button--primary" disabled={busy} onClick={() => void startOpenAiOfficialLogin()}>
                      {busy ? "正在启动登录…" : "用 ChatGPT 登录"}
                    </button>
                  ) : (
                    <>
                      <p>授权页已打开。若未自动跳转，可手动打开：</p>
                      <p><code className="url-break">{browserLogin.authUrl}</code></p>
                      {loginStatus && <p>{loginStatus}</p>}
                      <div className="form-actions">
                        <button type="button" className="button button--secondary" disabled={busy} onClick={() => void openExternalUrl(browserLogin.authUrl)}>再次打开页面</button>
                        <button type="button" className="button button--secondary" disabled={busy} onClick={() => void cancelOpenAiLogin()}>取消登录</button>
                      </div>
                      <label className="field" style={{ marginTop: "12px" }}>
                        <span>回调链接兜底（可选）</span>
                        <input
                          value={callbackUrl}
                          onChange={(event) => setCallbackUrl(event.target.value)}
                          placeholder="若浏览器停在 localhost 回调页，粘贴完整地址"
                          spellCheck={false}
                        />
                      </label>
                      <div className="form-actions">
                        <button type="button" className="button button--secondary" disabled={busy || !callbackUrl.trim()} onClick={() => void submitCallbackUrl()}>
                          粘贴回调完成登录
                        </button>
                      </div>
                    </>
                  )}
                </div>
              </section>
            )}

            {method.mode === "oauth" && method.kind === "xai" && (
              <section className="modal-section" aria-label="Grok 官方订阅登录">
                <div className="callout">
                  <strong>
                    Grok
                    <span className="method-badge method-badge--official">官方订阅</span>
                  </strong>
                  <p>
                    <strong>浏览器登录</strong> xAI / SuperGrok（Device Code）。成功后新建 Grok 实例，access token 加密保存，上游 <code>api.x.ai</code>。
                  </p>
                  {!deviceLogin ? (
                    <button type="button" className="button button--primary" disabled={busy} onClick={() => void startXaiOfficialLogin()}>
                      {busy ? "正在启动登录…" : "打开 Grok / xAI 登录"}
                    </button>
                  ) : (
                    <>
                      <p>在浏览器打开： <code>{deviceLogin.verificationUri}</code></p>
                      <p>输入代码： <strong className="user-code">{deviceLogin.userCode}</strong></p>
                      {loginStatus && <p>{loginStatus}</p>}
                      <div className="form-actions">
                        <button type="button" className="button button--secondary" disabled={busy} onClick={() => void openExternalUrl(deviceLogin.verificationUri)}>再次打开页面</button>
                        <button type="button" className="button button--secondary" disabled={busy} onClick={() => void cancelXaiLogin()}>取消登录</button>
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
                {method.kind === "xai" && (
                  <p className="panel-hint">
                    默认端点 <code>https://api.x.ai/v1</code>（API Key）。订阅用户请优先用「Grok · 官方订阅」（走 <code>cli-chat-proxy.grok.com</code>）；此处用于 xAI API Key。
                  </p>
                )}
                {method.kind === "opencode-go" && (
                  <div className="callout">
                    <strong>从 OpenCode 自动导入</strong>
                    <p>读取本机 <code>opencode-go</code> API 凭据，重新加密保存到 Spur；不会读取 OpenCode Zen 凭据。</p>
                    {openCodeGoStatus && <p className="panel-hint">{openCodeGoStatus}</p>}
                    <button type="button" className="button button--primary" disabled={busy} onClick={() => void submitOpenCodeGoImport()}>
                      {busy ? "正在导入并拉取…" : "从 OpenCode 导入并拉取模型"}
                    </button>
                  </div>
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
                    {busy ? "正在保存并拉取…" : method.kind === "opencode-go" ? "使用手动 Key 保存并拉取" : "保存并拉取模型"}
                  </button>
                </div>
              </section>
            )}

            {method.mode === "configJson" && (
              <section className="modal-section" aria-label="导入供应商配置 JSON">
                <div className="callout">
                  <strong>
                    导入供应商配置 JSON
                    <span className="method-badge method-badge--json">JSON</span>
                  </strong>
                  <p>
                    这是<strong>供应商配置</strong>（base_url / api_key / models），不是账号池。
                    若文件是 access_token / accounts / auth.json，请改用左侧「导入账号 JSON」或「官方订阅（浏览器登录）」。
                  </p>
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
              <section className="modal-section" aria-label="导入账号 JSON">
                <div className="callout">
                  <strong>
                    OpenAI · 导入账号 JSON
                    <span className="method-badge method-badge--json">JSON</span>
                  </strong>
                  <p>
                    粘贴或上传<strong>账号</strong> JSON（单对象 / 数组 / <code>accounts[]</code> / Codex auth.json / Sub2API 导出 / Agent Identity）。
                    ChatGPT access 会自动注册为 <strong>Agent Identity</strong>，导入后立刻拉取官方模型。
                    Session dump 请改用「导入 session 文件」。
                  </p>
                  <label className="field">
                    <span>粘贴账号 JSON</span>
                    <textarea
                      className="field-textarea field-textarea--code"
                      rows={8}
                      value={accountsJsonText}
                      onChange={(event) => setAccountsJsonText(event.target.value)}
                      placeholder={'{"access_token":"…"} 或 accounts 数组 / auth_mode=agentIdentity'}
                      spellCheck={false}
                      autoComplete="off"
                      disabled={busy}
                    />
                  </label>
                  <div className="form-actions form-actions--wrap">
                    <button
                      type="button"
                      className="button button--primary"
                      disabled={busy || !accountsJsonText.trim()}
                      onClick={() => void submitAccountsJson(accountsJsonText, false, false)}
                    >
                      {busy ? "导入中…" : "从文本导入并添加"}
                    </button>
                    <button
                      type="button"
                      className="button button--secondary"
                      disabled={busy}
                      onClick={() => accountFileRef.current?.click()}
                    >
                      选择 JSON 文件
                    </button>
                  </div>
                  <input
                    ref={accountFileRef}
                    className="visually-hidden"
                    type="file"
                    accept=".json,application/json,.txt,text/plain"
                    onChange={(event) => {
                      const file = event.target.files?.[0];
                      if (file) void submitAccounts(file);
                    }}
                  />
                </div>
              </section>
            )}

            {method.mode === "session" && (
              <section className="modal-section" aria-label="导入 session 文件">
                <div className="callout">
                  <strong>
                    OpenAI · 导入 session 文件
                    <span className="method-badge method-badge--json">Session</span>
                  </strong>
                  <p>
                    从浏览器打开 <code>https://chatgpt.com/api/auth/session</code>，复制整页 JSON
                    （含 <code>WARNING_BANNER</code> 也可）。将注册 <strong>Agent Identity</strong>
                    （无需接码），并立刻拉取官方模型。
                  </p>
                  <label className="field">
                    <span>粘贴 session JSON</span>
                    <textarea
                      className="field-textarea field-textarea--code"
                      rows={8}
                      value={accountsJsonText}
                      onChange={(event) => setAccountsJsonText(event.target.value)}
                      placeholder={'{"accessToken":"…","user":{…},"account":{…}}'}
                      spellCheck={false}
                      autoComplete="off"
                      disabled={busy}
                    />
                  </label>
                  <div className="form-actions form-actions--wrap">
                    <button
                      type="button"
                      className="button button--primary"
                      disabled={busy || !accountsJsonText.trim()}
                      onClick={() => void submitAccountsJson(accountsJsonText, false, true)}
                    >
                      {busy ? "注册并导入中…" : "导入 session 并添加"}
                    </button>
                    <button
                      type="button"
                      className="button button--secondary"
                      disabled={busy}
                      onClick={() => accountFileRef.current?.click()}
                    >
                      选择 session 文件
                    </button>
                  </div>
                  <input
                    ref={accountFileRef}
                    className="visually-hidden"
                    type="file"
                    accept=".json,application/json,.txt,text/plain"
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

function mergeSchedulerConfig(partial: Partial<PoolSchedulerConfig> | null | undefined): PoolSchedulerConfig {
  const d = defaultSchedulerConfig();
  if (!partial) return d;
  return {
    ...d,
    ...partial,
    stickyEscape: { ...d.stickyEscape, ...(partial.stickyEscape ?? {}) },
    scoreWeights: { ...d.scoreWeights, ...(partial.scoreWeights ?? {}) },
    fallbackSelectionMode:
      partial.fallbackSelectionMode === "random" ? "random" : d.fallbackSelectionMode,
  };
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
      quotaHeadroom: 1,
      upstreamCost: 0,
      previousResponse: 5,
      sessionSticky: 3,
    },
    stickyEscape: { enabled: true, ttftMs: 15000, errorRate: 0.5 },
    preferSoonestReset: false,
    default429CooldownSecs: 30,
    maxFailoverSwitches: 10,
    leaseTtlSecs: 900,
    excludeZeroQuota: true,
    quotaAutoPause5h: 1,
    quotaAutoPause7d: 1,
    stickyWaitEnabled: true,
    stickyWaitTimeoutSecs: 120,
    stickyWaitMaxWaiting: 3,
    fallbackWaitEnabled: true,
    fallbackWaitTimeoutSecs: 30,
    fallbackMaxWaiting: 100,
    fallbackSelectionMode: "last_used",
    stickyWeightedEnabled: false,
    rateLimit429CooldownEnabled: true,
    overload529CooldownSecs: 600,
    failoverOn400: false,
  };
}

function EditProviderSheet({
  provider,
  onClose,
  onChanged,
}: {
  provider: ProviderSummary;
  onClose: () => void;
  /** Quiet refresh after mutations (does not flash global loading). */
  onChanged: () => Promise<void>;
}) {
  const accountFileRef = useRef<HTMLInputElement>(null);
  const sessionFileRef = useRef<HTMLInputElement>(null);
  const configFileRef = useRef<HTMLInputElement>(null);
  const [accountsJsonText, setAccountsJsonText] = useState("");
  const [name, setName] = useState(provider.name);
  // Non-OpenAI still uses apiKey form; OpenAI always uses official discovery path.
  const [source] = useState<"official" | "apiKey">(
    provider.kind === "openai" ? "official" : "apiKey",
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
  const [deletingCredentialId, setDeletingCredentialId] = useState<string | null>(null);
  const [deletingProvider, setDeletingProvider] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [quotas, setQuotas] = useState<Record<string, OpenAiQuotaSnapshot | null>>({});
  const [quotaBusy, setQuotaBusy] = useState<Record<string, boolean>>({});
  const [quotaErrors, setQuotaErrors] = useState<Record<string, string | null>>({});
  const [quotaClockMs, setQuotaClockMs] = useState(() => Date.now());
  const [renamingCredentialId, setRenamingCredentialId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [renameBusy, setRenameBusy] = useState(false);
  const deleteBusy = deletingCredentialId != null || deletingProvider;

  useEscapeClose(onClose, !confirm && !deleteBusy);

  useEffect(() => {
    const timer = window.setInterval(() => setQuotaClockMs(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const loadCachedQuotas = useCallback(async (nextAccounts: CredentialSummary[]) => {
    if (provider.kind !== "openai") {
      setQuotas({});
      setQuotaErrors({});
      return;
    }
    const entries = await Promise.all(
      nextAccounts.map(async (account) => {
        if (!supportsOfficialQuota(account)) {
          return [account.id, null] as const;
        }
        try {
          const cached = await getCachedOpenAiQuota(account.id);
          return [account.id, cached] as const;
        } catch {
          return [account.id, null] as const;
        }
      }),
    );
    setQuotas(Object.fromEntries(entries));
  }, [provider.kind]);

  const refreshQuotaFor = useCallback(async (credentialId: string) => {
    setQuotaBusy((prev) => ({ ...prev, [credentialId]: true }));
    setQuotaErrors((prev) => ({ ...prev, [credentialId]: null }));
    try {
      const snapshot = await refreshOpenAiQuota(credentialId);
      setQuotas((prev) => ({ ...prev, [credentialId]: snapshot }));
    } catch (error) {
      setQuotaErrors((prev) => ({
        ...prev,
        [credentialId]: error instanceof Error ? error.message : String(error),
      }));
    } finally {
      setQuotaBusy((prev) => ({ ...prev, [credentialId]: false }));
    }
  }, []);

  const refreshAllQuotas = useCallback(async (nextAccounts: CredentialSummary[]) => {
    const eligible = nextAccounts.filter(supportsOfficialQuota);
    for (const account of eligible) {
      await refreshQuotaFor(account.id);
    }
  }, [refreshQuotaFor]);

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
      await loadCachedQuotas(nextAccounts);
    },
    [loadCachedQuotas],
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
        if (active) await loadCachedQuotas(nextAccounts);
      } catch {
        if (active) {
          setAccounts([]);
          setMembers([]);
          setQuotas({});
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [loadCachedQuotas, provider.activePoolId, provider.fixedCredentialId, provider.id, provider.routingMode]);

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
    if (provider.kind === "openai") {
      if (accounts.length === 0) {
        setMessage("请先用官方订阅 / 账号 JSON / session 添加至少一个账号。");
        return;
      }
      setBusy(true);
      setMessage(null);
      try {
        const routes = await discoverProviderModels(provider.id, "", undefined);
        const count = routes.filter((route) => route.providerId === provider.id).length;
        setMessage(`已拉取 ${count} 个官方模型。`);
        await applyAccountSnapshot(provider.id, provider.activePoolId, provider.routingMode, provider.fixedCredentialId);
        await onChanged();
      } catch (error) {
        setMessage(error instanceof Error ? error.message : String(error));
      } finally {
        setBusy(false);
      }
      return;
    }
    if (source === "apiKey" && !baseUrl.trim() && !provider.defaultBaseUrl && !provider.baseUrl) {
      setMessage("请填写 Base URL。");
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

  const importAccountsJson = async (raw: string, clearFile = false, asSession = false) => {
    const input = raw.trim();
    if (!input) {
      setMessage(asSession ? "请粘贴 session JSON，或选择 session 文件。" : "请粘贴账号 JSON，或选择 JSON 文件。");
      return;
    }
    setBusy(true);
    setMessage(null);
    try {
      const imported = asSession
        ? await importSessionJson(provider.id, input)
        : await importCredentialsJson(provider.id, input);
      setAccountsJsonText("");
      await applyAccountSnapshot(provider.id, provider.activePoolId, provider.routingMode, provider.fixedCredentialId);
      // Immediately refresh official models after account/session import.
      let modelNote = "";
      try {
        const routes = await discoverProviderModels(provider.id, "", undefined);
        const count = routes.filter((route) => route.providerId === provider.id).length;
        modelNote = `，已拉取 ${count} 个模型`;
      } catch (modelError) {
        modelNote = `，模型拉取失败：${modelError instanceof Error ? modelError.message : String(modelError)}`;
      }
      setMessage(`已导入 ${imported.length} 个账号到此实例${modelNote}。`);
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      if (clearFile && accountFileRef.current) accountFileRef.current.value = "";
    }
  };

  const importAccountFile = async (file: File, asSession = false) => {
    await importAccountsJson(await file.text(), true, asSession);
  };

  const importConfigFile = async (file: File) => {
    if (provider.kind === "openai") {
      setMessage("OpenAI 已不再支持导入供应商配置 JSON，请用账号 JSON 或 session。");
      return;
    }
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
        member.upstreamCostRate ?? 1,
      );
      setMembers(await listPoolMembersDetailed(poolId));
      setMessage("已更新账号调度参数。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const executeDeleteProvider = async () => {
    setDeletingProvider(true);
    setMessage(null);
    try {
      await deleteProviderInstance(provider.id);
      setConfirm(null);
      onClose();
      void onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
      setConfirm(null);
    } finally {
      setDeletingProvider(false);
    }
  };

  const requestDeleteProvider = () => {
    setConfirm({
      title: `删除供应商「${provider.name}」？`,
      body: "此操作不可恢复：账号凭据、调度池成员与模型候选会一并删除。",
      confirmLabel: "删除供应商",
      danger: true,
      onConfirm: () => executeDeleteProvider(),
    });
  };

  const executeDeleteAccount = async (account: CredentialSummary, identity: string) => {
    setDeletingCredentialId(account.id);
    setMessage(null);
    // Optimistic remove so the row disappears even if snapshot refresh is slow.
    setAccounts((prev) => prev.filter((item) => item.id !== account.id));
    setMembers((prev) => prev.filter((item) => item.credentialId !== account.id));
    setQuotas((prev) => {
      const next = { ...prev };
      delete next[account.id];
      return next;
    });
    if (fixedCredentialId === account.id) {
      setFixedCredentialId(null);
      setRoutingMode("pool");
    }
    try {
      const result = await deleteCredential(account.id);
      setConfirm(null);
      try {
        const routing = await getProviderRouting(provider.id);
        await applyAccountSnapshot(
          provider.id,
          routing?.activePoolId ?? provider.activePoolId,
          routing?.routingMode ?? "pool",
          routing?.fixedCredentialId ?? null,
        );
      } catch {
        // Local optimistic state already updated.
      }
      void onChanged();

      if (result.remainingAccounts === 0) {
        setMessage("账号已删除。此实例暂无账号。");
        setConfirm({
          title: `「${provider.name}」已无账号`,
          body: "是否一并删除此供应商实例？删除后模型候选也会移除。",
          confirmLabel: "删除供应商",
          danger: true,
          onConfirm: () => executeDeleteProvider(),
        });
      } else {
        setMessage(`已删除账号「${identity}」。`);
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
      setConfirm(null);
      // Re-sync list from storage after a failed/partial delete attempt.
      try {
        await applyAccountSnapshot(
          provider.id,
          provider.activePoolId,
          provider.routingMode,
          provider.fixedCredentialId,
        );
      } catch {
        // ignore
      }
    } finally {
      setDeletingCredentialId(null);
    }
  };

  const requestDeleteAccount = (account: CredentialSummary) => {
    const identity = accountDisplayName(account);
    setConfirm({
      title: `删除账号「${identity}」？`,
      body: "此操作不可恢复：本地加密凭据将永久删除，并从调度池移出。",
      confirmLabel: "删除账号",
      danger: true,
      onConfirm: () => executeDeleteAccount(account, identity),
    });
  };

  const beginRenameAccount = (account: CredentialSummary) => {
    setRenamingCredentialId(account.id);
    setRenameDraft(account.label ?? "");
  };

  const cancelRenameAccount = () => {
    setRenamingCredentialId(null);
    setRenameDraft("");
  };

  const commitRenameAccount = async (credentialId: string) => {
    if (renameBusy) return;
    setRenameBusy(true);
    setMessage(null);
    try {
      const updated = await renameCredential(credentialId, renameDraft);
      setAccounts((prev) => prev.map((item) => (item.id === credentialId ? updated : item)));
      setMembers((prev) =>
        prev.map((item) =>
          item.credentialId === credentialId ? { ...item, label: updated.label } : item,
        ),
      );
      setRenamingCredentialId(null);
      setRenameDraft("");
      setMessage("账号名称已更新。");
      await onChanged();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setRenameBusy(false);
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
              {provider.kind === "openai" ? (
                <div className="callout">
                  <strong>OpenAI 官方通道</strong>
                  <p>
                    仅支持三种加号方式：官方订阅、账号 JSON、session 文件。健康账号：
                    {provider.healthyCredentialCount}/{provider.credentialCount}。
                  </p>
                </div>
              ) : (
                <>
                  <label className="field"><span>Base URL</span><input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} spellCheck={false} /></label>
                  <label className="field"><span>API Key{accounts.length > 0 ? "（可留空，使用已有账号）" : ""}</span><input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder="只写入本地加密存储" autoComplete="off" /></label>
                </>
              )}
              <div className="form-actions">
                <button type="button" className="button button--primary" disabled={busy} onClick={() => void configureApi()}>
                  {busy ? "正在保存并拉取…" : provider.kind === "openai" ? "重新拉取官方模型" : "保存并拉取模型"}
                </button>
              </div>
            </section>

            <section className="modal-section" aria-label="账号">
              <div className="modal-section__header">
                <div>
                  <h3>账号{accounts.length > 1 ? " · 实例内调度池" : ""}</h3>
                  <p>
                    {provider.kind === "openai"
                      ? "可继续：导入账号 JSON、导入 session，或使用官方订阅登录再加一号。导入后自动拉模型。"
                      : accounts.length > 1
                        ? "多账号时在实例内做池调度（粘性 → Top-K）。"
                        : "可继续导入账号 JSON 组成实例内调度池。"}
                  </p>
                </div>
                <span className="badge badge--neutral">{accounts.length} 个</span>
              </div>
              <label className="field">
                <span>{provider.kind === "openai" ? "粘贴账号 JSON 或 session JSON" : "粘贴账号 JSON"}</span>
                <textarea
                  className="field-textarea field-textarea--code"
                  rows={5}
                  value={accountsJsonText}
                  onChange={(event) => setAccountsJsonText(event.target.value)}
                  placeholder={
                    provider.kind === "openai"
                      ? '账号 JSON：{"access_token":"…"} / accounts[]\nsession：{"accessToken":"…","user":{…},"account":{…}}'
                      : '{"access_token":"…"} / accounts 数组'
                  }
                  spellCheck={false}
                  autoComplete="off"
                  disabled={busy}
                />
              </label>
              <div className="form-actions form-actions--wrap">
                <button
                  type="button"
                  className="button button--primary"
                  disabled={busy || !accountsJsonText.trim()}
                  onClick={() => void importAccountsJson(accountsJsonText, false, false)}
                >
                  导入账号 JSON
                </button>
                {provider.kind === "openai" ? (
                  <button
                    type="button"
                    className="button button--primary"
                    disabled={busy || !accountsJsonText.trim()}
                    onClick={() => void importAccountsJson(accountsJsonText, false, true)}
                  >
                    导入 session
                  </button>
                ) : null}
                <button type="button" className="button button--secondary" disabled={busy} onClick={() => accountFileRef.current?.click()}>选择账号 JSON 文件</button>
                {provider.kind === "openai" ? (
                  <button type="button" className="button button--secondary" disabled={busy} onClick={() => sessionFileRef.current?.click()}>选择 session 文件</button>
                ) : (
                  <button type="button" className="button button--secondary" disabled={busy} onClick={() => configFileRef.current?.click()}>导入供应商配置 JSON</button>
                )}
                {provider.kind === "openai" && accounts.some(supportsOfficialQuota) ? (
                  <button
                    type="button"
                    className="button button--secondary"
                    disabled={busy || Object.values(quotaBusy).some(Boolean)}
                    onClick={() => void refreshAllQuotas(accounts)}
                  >
                    刷新全部额度
                  </button>
                ) : null}
                <input ref={accountFileRef} className="visually-hidden" type="file" accept=".json,application/json,.txt,text/plain" onChange={(event) => { const file = event.target.files?.[0]; if (file) void importAccountFile(file, false); }} />
                <input ref={sessionFileRef} className="visually-hidden" type="file" accept=".json,application/json,.txt,text/plain" onChange={(event) => { const file = event.target.files?.[0]; if (file) void importAccountFile(file, true); }} />
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
                      池调度
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
                      固定账号
                    </button>
                  </div>
                  <small>
                    {routingMode === "fixed"
                      ? "固定账号：所有请求只走选中账号。"
                      : "池调度：previous_response → session → Top-K 加权。池级参数在「设置 → 账号池设置」。"}
                  </small>
                </div>
              ) : null}

              {accounts.length === 0 ? (
                <div className="empty-inline">此实例还没有账号。可导入账号 JSON（账号池），或用官方订阅/API Key 添加。</div>
              ) : (
                <div className="modal-account-list modal-account-list--editable">
                  {accounts.map((account) => {
                    const member = members.find((item) => item.credentialId === account.id);
                    const selectedFixed = routingMode === "fixed" && fixedCredentialId === account.id;
                    return (
                      <div className={`modal-account-row modal-account-row--edit${selectedFixed ? " modal-account-row--fixed" : ""}`} key={account.id}>
                        <div className="modal-account-row__head">
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
                          <span className="modal-account-row__meta">
                            {renamingCredentialId === account.id ? (
                              <input
                                className="account-rename-input"
                                value={renameDraft}
                                autoFocus
                                disabled={renameBusy}
                                maxLength={64}
                                placeholder={account.maskedEmail ?? account.maskedAccountId ?? "账号显示名"}
                                aria-label="账号显示名"
                                onChange={(event) => setRenameDraft(event.target.value)}
                                onKeyDown={(event) => {
                                  if (event.key === "Enter") {
                                    event.preventDefault();
                                    void commitRenameAccount(account.id);
                                  } else if (event.key === "Escape") {
                                    event.preventDefault();
                                    cancelRenameAccount();
                                  }
                                }}
                              />
                            ) : (
                              <strong title="双击重命名" onDoubleClick={() => beginRenameAccount(account)}>
                                {accountDisplayName(account)}
                              </strong>
                            )}
                            <small>
                              {describeCredentialKind(account)}
                              {member ? ` · ${member.scheduleState}` : ""}
                              {member?.cooldownUntil ? " · cooldown" : ""}
                            </small>
                          </span>
                          <span className={`badge ${account.healthy ? "badge--success" : "badge--error"}`}>{account.healthy ? "可用" : "失效"}</span>
                          <button
                            type="button"
                            className="button button--secondary"
                            disabled={deleteBusy || renameBusy}
                            onClick={(event) => {
                              event.stopPropagation();
                              if (renamingCredentialId === account.id) {
                                void commitRenameAccount(account.id);
                              } else {
                                beginRenameAccount(account);
                              }
                            }}
                          >
                            {renamingCredentialId === account.id ? (renameBusy ? "保存中…" : "保存") : "重命名"}
                          </button>
                          <button
                            type="button"
                            className="button button--secondary button--danger-text"
                            disabled={deleteBusy || renameBusy}
                            onClick={(event) => {
                              event.stopPropagation();
                              requestDeleteAccount(account);
                            }}
                          >
                            {deletingCredentialId === account.id ? "删除中…" : "删除"}
                          </button>
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
                              <label title="上游计费倍率（调度评分用；1=中性）">
                                <span>成本</span>
                                <input
                                  type="number"
                                  min={0.01}
                                  step={0.1}
                                  value={member.upstreamCostRate ?? 1}
                                  disabled={busy}
                                  onChange={(event) => {
                                    const upstreamCostRate = Number(event.target.value) || 1;
                                    setMembers((prev) =>
                                      prev.map((item) =>
                                        item.credentialId === member.credentialId
                                          ? { ...item, upstreamCostRate }
                                          : item,
                                      ),
                                    );
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
                        {provider.kind === "openai" ? (
                          <AccountQuotaBlock
                            account={account}
                            snapshot={quotas[account.id]}
                            busy={Boolean(quotaBusy[account.id])}
                            error={quotaErrors[account.id]}
                            onRefresh={() => void refreshQuotaFor(account.id)}
                            nowMs={quotaClockMs}
                          />
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
          <button
            type="button"
            className="button button--secondary button--danger-text"
            disabled={deleteBusy}
            onClick={() => requestDeleteProvider()}
          >
            {deletingProvider ? "删除中…" : "删除此供应商"}
          </button>
          <span>模型发布请到「模型」页开启。</span>
          <button type="button" className="button button--secondary" disabled={deleteBusy} onClick={onClose}>
            完成
          </button>
        </footer>
      </section>
      {confirm ? (
        <ConfirmSheet
          request={confirm}
          busy={deleteBusy}
          onCancel={() => {
            if (!deleteBusy) setConfirm(null);
          }}
        />
      ) : null}
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

/** Models list title: `{供应商名}.{模型名}` (falls back to providerId if name empty). */
function modelListLabel(route: ModelRouteSummary): string {
  const provider = route.providerName.trim() || route.providerId;
  return `${provider}.${route.displayName}`;
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
  const filtered = routes.filter((route) =>
    `${modelListLabel(route)} ${route.upstreamModel} ${route.providerName} ${route.providerId}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );

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
                  <span className="data-row__main"><strong>{modelListLabel(route)}</strong><small><code>{route.id}</code> · {route.protocol}</small></span>
                  <span className="badge badge--neutral">{route.providerName.trim() || route.providerId}</span>
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
          按序检查：① ChatGPT.app 已登录官方账号（~/.codex/auth.json，不是 Spur OAuth）；② config 中
          name = &quot;OpenAI&quot; 且 requires_openai_auth = true；③ catalog snake_case 且 tools 为空数组；
          ④ 本地代理在跑；⑤ Cmd+Q 冷启动；⑥ 在「高级 → 模型」里找（Power 滑动条只有官方阶梯）。
          若 CC Switch 抢回 custom provider，请重新 Review &amp; Apply。概览页的「Desktop 可见性」清单可逐项对绿。
        </p>
      </div>
      <div className="callout">
        <strong>协议覆盖状态</strong>
        <p>Responses 路由支持透传；Chat Completions 已提供非流式转换骨架，流式 SSE 工具调用转换仍会明确返回未实现错误，不会静默伪装为成功。</p>
      </div>
    </div>
  );
}

type FieldHelp = {
  key: string;
  label: string;
  meaning: string;
  effect: string;
};

const SCHEDULER_FIELD_HELP = {
  lbTopK: {
    key: "lbTopK",
    label: "候选 Top-K",
    meaning: "Load-balance Top-K",
    effect: "加权选号前，先取分数最高的 K 个账号参与抽选。",
  },
  maxFailoverSwitches: {
    key: "maxFailoverSwitches",
    label: "失败换号次数",
    meaning: "Max failover switches",
    effect: "单次请求失败后最多切换账号的次数。",
  },
  default429CooldownSecs: {
    key: "default429CooldownSecs",
    label: "429 冷却（秒）",
    meaning: "Default 429 cooldown",
    effect: "账号触发限流（429）后进入冷却的秒数，冷却期内不会再被选中。",
  },
  excludeZeroQuota: {
    key: "excludeZeroQuota",
    label: "排除额度≈0",
    meaning: "Exclude zero quota",
    effect: "当新鲜配额快照显示剩余额度接近 0 时，跳过该账号。",
  },
  stickySessionTtlSecs: {
    key: "stickySessionTtlSecs",
    label: "Session 粘性 TTL",
    meaning: "Session sticky TTL (seconds)",
    effect: "按 session-hash 把后续请求粘到同一账号的有效秒数。",
  },
  stickyResponseIdTtlSecs: {
    key: "stickyResponseIdTtlSecs",
    label: "Response 粘性 TTL",
    meaning: "Response sticky TTL (seconds)",
    effect: "按 previous_response_id 亲和绑定到同一账号的有效秒数。",
  },
  leaseTtlSecs: {
    key: "leaseTtlSecs",
    label: "租约 Lease TTL",
    meaning: "Lease TTL (seconds)",
    effect: "账号租约最长占用秒数；崩溃或异常断开后超时自动释放。",
  },
  stickyWaitEnabled: {
    key: "stickyWaitEnabled",
    label: "Sticky 并发等待",
    meaning: "Sticky wait enabled",
    effect: "粘性账号并发已满时，优先等待空位而不是立刻换号。",
  },
  stickyWaitTimeoutSecs: {
    key: "stickyWaitTimeoutSecs",
    label: "Sticky 等待秒",
    meaning: "Sticky wait timeout (seconds)",
    effect: "粘性并发等待的最长时间（Sub2API 默认 120s）；超时后可逃逸到其他账号。",
  },
  stickyWaitMaxWaiting: {
    key: "stickyWaitMaxWaiting",
    label: "Sticky 最大排队",
    meaning: "Sticky max waiting",
    effect: "同一粘性账号允许同时等待的请求数；超出则立刻换号（Sub2API=3）。",
  },
  fallbackWaitEnabled: {
    key: "fallbackWaitEnabled",
    label: "兜底并发等待",
    meaning: "Fallback wait enabled",
    effect: "全部账号并发满时，排队等待任意空位再选号。",
  },
  fallbackWaitTimeoutSecs: {
    key: "fallbackWaitTimeoutSecs",
    label: "兜底等待秒",
    meaning: "Fallback wait timeout",
    effect: "兜底并发等待最长时间（Sub2API 默认 30s）。",
  },
  fallbackMaxWaiting: {
    key: "fallbackMaxWaiting",
    label: "兜底最大排队",
    meaning: "Fallback max waiting",
    effect: "兜底等待队列上限（Sub2API 默认 100）。",
  },
  fallbackSelectionMode: {
    key: "fallbackSelectionMode",
    label: "兜底选号方式",
    meaning: "Fallback selection mode",
    effect: "last_used=最久未用优先；random=随机（Sub2API）。",
  },
  stickyWeightedEnabled: {
    key: "stickyWeightedEnabled",
    label: "Sticky 加权模式",
    meaning: "Sticky weighted",
    effect: "开启后粘性进入打分（可跨号）；关闭则硬绑定 previous_response/session。",
  },
  rateLimit429CooldownEnabled: {
    key: "rateLimit429CooldownEnabled",
    label: "429 冷却开关",
    meaning: "Rate-limit cooldown enabled",
    effect: "收到 429/额度耗尽时是否写入冷却；关闭仍可换号但不记冷却。",
  },
  overload529CooldownSecs: {
    key: "overload529CooldownSecs",
    label: "529 冷却（秒）",
    meaning: "Overload 529 cooldown",
    effect: "上游 529 过载后暂停调度的秒数（Sub2API 默认 10 分钟=600）。",
  },
  failoverOn400: {
    key: "failoverOn400",
    label: "400 允许换号",
    meaning: "Failover on 400",
    effect: "开启后部分 400 也会切换账号重试（Sub2API 默认关，慎用）。",
  },
  stickyEscapeEnabled: {
    key: "stickyEscape.enabled",
    label: "Sticky 逃逸",
    meaning: "Sticky escape",
    effect: "粘性账号不健康（慢/高错误/不可用）时允许解绑并重新选号。",
  },
  stickyEscapeTtftMs: {
    key: "stickyEscape.ttftMs",
    label: "逃逸 TTFT 阈值",
    meaning: "Escape TTFT (ms)",
    effect: "首 token 延迟（TTFT）超过该毫秒数时视为过慢，触发 sticky 逃逸。",
  },
  stickyEscapeErrorRate: {
    key: "stickyEscape.errorRate",
    label: "逃逸错误率阈值",
    meaning: "Escape error rate",
    effect: "滑动错误率超过该比例（0–1）时触发 sticky 逃逸。",
  },
  preferSoonestReset: {
    key: "preferSoonestReset",
    label: "优先临近重置",
    meaning: "Prefer soonest reset",
    effect: "评分时倾向选择额度窗口即将重置的账号（用掉快过期的额度）。",
  },
  quotaAutoPause5h: {
    key: "quotaAutoPause5h",
    label: "5h 额度暂停阈值",
    meaning: "Quota auto-pause 5h",
    effect: "5 小时窗口已用比例 ≥ 该值时跳过账号；0 表示关闭。",
  },
  quotaAutoPause7d: {
    key: "quotaAutoPause7d",
    label: "7d 额度暂停阈值",
    meaning: "Quota auto-pause 7d",
    effect: "7 天窗口已用比例 ≥ 该值时跳过账号；0 表示关闭。",
  },
  wPriority: {
    key: "scoreWeights.priority",
    label: "权重 · 优先级",
    meaning: "W·priority",
    effect: "Top-K 评分中账号 priority 因子的权重；越高越偏向高优先级账号。",
  },
  wLoad: {
    key: "scoreWeights.load",
    label: "权重 · 负载",
    meaning: "W·load",
    effect: "评分中并发负载因子权重；越高越偏向当前更空闲的账号。",
  },
  wQueue: {
    key: "scoreWeights.queue",
    label: "权重 · 队列",
    meaning: "W·queue",
    effect: "评分中排队/槽位占用因子权重。",
  },
  wError: {
    key: "scoreWeights.errorRate",
    label: "权重 · 错误率",
    meaning: "W·errorRate",
    effect: "评分中错误率因子权重；越高越惩罚高错误账号。",
  },
  wTtft: {
    key: "scoreWeights.ttft",
    label: "权重 · TTFT",
    meaning: "W·ttft",
    effect: "评分中首 token 延迟因子权重；越高越偏向更快的账号。",
  },
  wReset: {
    key: "scoreWeights.reset",
    label: "权重 · 重置时间",
    meaning: "W·reset",
    effect: "评分中额度重置临近度权重；配合「优先临近重置」使用。",
  },
  wQuota: {
    key: "scoreWeights.quotaHeadroom",
    label: "权重 · 额度余量",
    meaning: "W·quotaHeadroom",
    effect: "评分中剩余额度因子权重；越高越偏向仍有配额的账号。",
  },
  wUpstreamCost: {
    key: "scoreWeights.upstreamCost",
    label: "权重 · 上游成本",
    meaning: "W·upstreamCost",
    effect: "倾向上游计费倍率更低的账号；0=关闭（Sub2API 默认）。",
  },
  wPreviousResponse: {
    key: "scoreWeights.previousResponse",
    label: "权重 · previous_response",
    meaning: "W·previousResponse",
    effect: "Sticky 加权模式下，绑定 previous_response 的账号加分（默认 5）。",
  },
  wSessionSticky: {
    key: "scoreWeights.sessionSticky",
    label: "权重 · session sticky",
    meaning: "W·sessionSticky",
    effect: "Sticky 加权模式下，绑定 session 的账号加分（默认 3）。",
  },
} as const satisfies Record<string, FieldHelp>;

function FieldHint({ help }: { help: FieldHelp }) {
  const tipId = useId();
  return (
    <span className="field-hint">
      <button
        type="button"
        className="field-hint__btn"
        aria-label={`${help.label}说明`}
        aria-describedby={tipId}
      >
        i
      </button>
      <span className="field-hint__tip" role="tooltip" id={tipId}>
        <code className="field-hint__key">{help.key}</code>
        <strong className="field-hint__meaning">{help.meaning}</strong>
        <span className="field-hint__effect">{help.effect}</span>
      </span>
    </span>
  );
}

function FieldLabel({ help }: { help: FieldHelp }) {
  return (
    <span className="field__label-row">
      <span>{help.label}</span>
      <FieldHint help={help} />
    </span>
  );
}

function NumberField({
  help,
  value,
  min,
  max,
  step,
  onChange,
}: {
  help: FieldHelp;
  value: number;
  min?: number;
  max?: number;
  step?: number | string;
  onChange: (value: number) => void;
}) {
  return (
    <label className="field">
      <FieldLabel help={help} />
      <input
        type="number"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value) || 0)}
      />
    </label>
  );
}

function CheckField({
  help,
  checked,
  onChange,
}: {
  help: FieldHelp;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="field field--check">
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      <FieldLabel help={help} />
    </label>
  );
}

/** Experimental: two-button Kimi publish (enable / disable). */
function KimiPublishPanel() {
  const [busy, setBusy] = useState(false);
  /** Sole source of the status line after user actions / load. */
  const [stateLabel, setStateLabel] = useState("状态：…");
  const [message, setMessage] = useState<string | null>(null);
  const [messageTone, setMessageTone] = useState<"ok" | "warn" | "err">("ok");
  const loadStatus = async () => {
    try {
      const s = await kimiTargetStatus();
      if (s.publishActive) {
        setStateLabel(
          `状态：已启用${s.lastModelCount != null ? ` · ${s.lastModelCount} 个模型` : ""} · 请完全退出并重开 Kimi`,
        );
      } else {
        setStateLabel("状态：未启用");
      }
    } catch (error) {
      setStateLabel("状态：未启用");
      setMessage(error instanceof Error ? error.message : String(error));
      setMessageTone("err");
    }
  };

  const runEnable = async () => {
    if (busy) return;
    setBusy(true);
    setMessage(null);
    setStateLabel("状态：启用中…");
    try {
      const result = await enableKimiPublish();
      setStateLabel(
        result.enabled
          ? `状态：已启用${result.modelCount > 0 ? ` · ${result.modelCount} 个模型` : ""}（仅写盘）`
          : "状态：未启用",
      );
      setMessage(result.message + (result.warnings.length ? `\n· ${result.warnings.slice(0, 4).join("\n· ")}` : ""));
      setMessageTone("ok");
    } catch (error) {
      setStateLabel("状态：未启用");
      setMessage(`启用失败：${error instanceof Error ? error.message : String(error)}`);
      setMessageTone("err");
    } finally {
      setBusy(false);
    }
  };

  const runDisable = async () => {
    if (busy) return;
    setBusy(true);
    setMessage(null);
    setStateLabel("状态：关闭中…");
    try {
      const result = await disableKimiPublish();
      setStateLabel("状态：未启用");
      setMessage(result.message + (result.warnings.length ? `\n· ${result.warnings.slice(0, 3).join("\n· ")}` : ""));
      setMessageTone("ok");
    } catch (error) {
      setMessage(`关闭失败：${error instanceof Error ? error.message : String(error)}`);
      setMessageTone("err");
      await loadStatus();
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="panel" aria-label="发布到 Kimi App">
      <div className="panel__header">
        <div>
          <h2>发布到 Kimi App（实验）</h2>
          <p>
            启用 = 只写入 Kimi 缓存/配置（不改系统代理）。关闭 = 恢复备份。右下角要显示 Spur
            模型需另做路径拦截，见 docs/kimi-app-selective-block.md。
          </p>
        </div>
      </div>
      <div className="settings-body">
        <p
          className="panel-hint"
          style={{
            fontSize: 13,
            fontWeight: 600,
            color: stateLabel.includes("已启用") ? "var(--success, #2a8)" : undefined,
          }}
        >
          {stateLabel}
        </p>
        <div className="settings-body--inline">
          <button type="button" className="button button--primary" disabled={busy} onClick={() => void runEnable()}>
            {busy ? "处理中…" : "启用发布"}
          </button>
          <button type="button" className="button button--secondary" disabled={busy} onClick={() => void runDisable()}>
            关闭发布
          </button>
          <button type="button" className="button button--secondary" disabled={busy} onClick={() => void loadStatus()}>
            刷新状态
          </button>
        </div>
        {message ? (
          <div
            className="empty-inline"
            style={{
              textAlign: "left",
              whiteSpace: "pre-wrap",
              borderColor:
                messageTone === "err"
                  ? "var(--danger, #c44)"
                  : messageTone === "warn"
                    ? "var(--warning, #b80)"
                    : undefined,
            }}
          >
            {message}
          </div>
        ) : (
          <p className="panel-hint">
            点启用只写盘，Kimi 应能正常联网打开。勿开「整站代理 www.kimi.com」。稳定多模型请用
            Codex Apply。
          </p>
        )}
      </div>
    </section>
  );
}

function SettingsPage({ providers }: { providers: ProviderSummary[] }) {
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
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

  const isAdvancedCustomized = useMemo(() => {
    const d = defaultSchedulerConfig();
    const c = schedulerConfig;
    return (
      c.stickySessionTtlSecs !== d.stickySessionTtlSecs ||
      c.stickyResponseIdTtlSecs !== d.stickyResponseIdTtlSecs ||
      c.leaseTtlSecs !== d.leaseTtlSecs ||
      c.stickyWaitEnabled !== d.stickyWaitEnabled ||
      c.stickyWaitTimeoutSecs !== d.stickyWaitTimeoutSecs ||
      c.stickyWaitMaxWaiting !== d.stickyWaitMaxWaiting ||
      c.fallbackWaitEnabled !== d.fallbackWaitEnabled ||
      c.fallbackWaitTimeoutSecs !== d.fallbackWaitTimeoutSecs ||
      c.fallbackMaxWaiting !== d.fallbackMaxWaiting ||
      c.fallbackSelectionMode !== d.fallbackSelectionMode ||
      c.stickyWeightedEnabled !== d.stickyWeightedEnabled ||
      c.rateLimit429CooldownEnabled !== d.rateLimit429CooldownEnabled ||
      c.overload529CooldownSecs !== d.overload529CooldownSecs ||
      c.failoverOn400 !== d.failoverOn400 ||
      c.stickyEscape.enabled !== d.stickyEscape.enabled ||
      c.stickyEscape.ttftMs !== d.stickyEscape.ttftMs ||
      c.stickyEscape.errorRate !== d.stickyEscape.errorRate ||
      c.preferSoonestReset !== d.preferSoonestReset ||
      c.quotaAutoPause5h !== d.quotaAutoPause5h ||
      c.quotaAutoPause7d !== d.quotaAutoPause7d ||
      c.scoreWeights.priority !== d.scoreWeights.priority ||
      c.scoreWeights.load !== d.scoreWeights.load ||
      c.scoreWeights.queue !== d.scoreWeights.queue ||
      c.scoreWeights.errorRate !== d.scoreWeights.errorRate ||
      c.scoreWeights.ttft !== d.scoreWeights.ttft ||
      c.scoreWeights.reset !== d.scoreWeights.reset ||
      c.scoreWeights.quotaHeadroom !== d.scoreWeights.quotaHeadroom ||
      c.scoreWeights.upstreamCost !== d.scoreWeights.upstreamCost ||
      c.scoreWeights.previousResponse !== d.scoreWeights.previousResponse ||
      c.scoreWeights.sessionSticky !== d.scoreWeights.sessionSticky
    );
  }, [schedulerConfig]);

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
      if (active) setSchedulerConfig(mergeSchedulerConfig(config));
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
      setMessage("已保存账号池设置。");
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

      <KimiPublishPanel />

      <section className="panel" aria-label="账号池设置">
        <div className="panel__header">
          <div>
            <h2>账号池设置</h2>
            <p>多账号实例的池级调度参数（Sub2API 风格）。日常 Pool/Fixed 与账号 weight 仍在供应商编辑里。</p>
          </div>
        </div>
        <div className="settings-body">
          {poolProviders.length === 0 ? (
            <div className="empty-inline">暂无带账号的供应商实例。导入多账号后可在此配置账号池参数。</div>
          ) : (
            <>
              <label className="field field--provider">
                <span className="field__label-row">
                  <span>供应商实例</span>
                </span>
                <select
                  className="select-control"
                  value={effectiveProviderId}
                  onChange={(event) => setSelectedProviderId(event.target.value)}
                >
                  {poolProviders.map((provider) => (
                    <option key={provider.id} value={provider.id}>
                      {provider.name} ({provider.credentialCount} 账号)
                    </option>
                  ))}
                </select>
              </label>

              <div className="settings-group">
                <h3 className="settings-group__title">常用</h3>
                <div className="scheduler-grid">
                  <NumberField
                    help={SCHEDULER_FIELD_HELP.lbTopK}
                    min={1}
                    max={64}
                    value={schedulerConfig.lbTopK}
                    onChange={(value) => setSchedulerConfig({ ...schedulerConfig, lbTopK: value || 1 })}
                  />
                  <NumberField
                    help={SCHEDULER_FIELD_HELP.maxFailoverSwitches}
                    min={1}
                    max={20}
                    value={schedulerConfig.maxFailoverSwitches}
                    onChange={(value) => setSchedulerConfig({ ...schedulerConfig, maxFailoverSwitches: value || 1 })}
                  />
                  <NumberField
                    help={SCHEDULER_FIELD_HELP.default429CooldownSecs}
                    min={1}
                    value={schedulerConfig.default429CooldownSecs}
                    onChange={(value) => setSchedulerConfig({ ...schedulerConfig, default429CooldownSecs: value || 1 })}
                  />
                  <NumberField
                    help={SCHEDULER_FIELD_HELP.overload529CooldownSecs}
                    min={1}
                    value={schedulerConfig.overload529CooldownSecs}
                    onChange={(value) =>
                      setSchedulerConfig({ ...schedulerConfig, overload529CooldownSecs: value || 1 })
                    }
                  />
                </div>
                <div className="scheduler-grid scheduler-grid--toggles">
                  <CheckField
                    help={SCHEDULER_FIELD_HELP.excludeZeroQuota}
                    checked={schedulerConfig.excludeZeroQuota}
                    onChange={(checked) => setSchedulerConfig({ ...schedulerConfig, excludeZeroQuota: checked })}
                  />
                  <CheckField
                    help={SCHEDULER_FIELD_HELP.rateLimit429CooldownEnabled}
                    checked={schedulerConfig.rateLimit429CooldownEnabled}
                    onChange={(checked) =>
                      setSchedulerConfig({ ...schedulerConfig, rateLimit429CooldownEnabled: checked })
                    }
                  />
                  <CheckField
                    help={SCHEDULER_FIELD_HELP.failoverOn400}
                    checked={schedulerConfig.failoverOn400}
                    onChange={(checked) =>
                      setSchedulerConfig({ ...schedulerConfig, failoverOn400: checked })
                    }
                  />
                </div>
              </div>

              <div className={`scheduler-advanced${advancedOpen ? " scheduler-advanced--open" : ""}`}>
                <button
                  type="button"
                  className="scheduler-advanced__toggle"
                  aria-expanded={advancedOpen}
                  onClick={() => setAdvancedOpen((open) => !open)}
                >
                  <span className="scheduler-advanced__chevron" aria-hidden>
                    {advancedOpen ? "▾" : "▸"}
                  </span>
                  <span className="scheduler-advanced__heading">
                    <strong>高级设置</strong>
                    <small>粘性、逃逸、额度阈值与评分权重</small>
                  </span>
                  {isAdvancedCustomized ? <span className="badge badge--neutral">已自定义</span> : null}
                </button>

                {advancedOpen ? (
                  <div className="scheduler-advanced__body">
                    <div className="settings-group">
                      <h3 className="settings-group__title">粘性与租约</h3>
                      <div className="scheduler-grid">
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.stickySessionTtlSecs}
                          min={60}
                          value={schedulerConfig.stickySessionTtlSecs}
                          onChange={(value) =>
                            setSchedulerConfig({ ...schedulerConfig, stickySessionTtlSecs: value || 60 })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.stickyResponseIdTtlSecs}
                          min={60}
                          value={schedulerConfig.stickyResponseIdTtlSecs}
                          onChange={(value) =>
                            setSchedulerConfig({ ...schedulerConfig, stickyResponseIdTtlSecs: value || 60 })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.leaseTtlSecs}
                          min={60}
                          value={schedulerConfig.leaseTtlSecs}
                          onChange={(value) => setSchedulerConfig({ ...schedulerConfig, leaseTtlSecs: value || 60 })}
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.stickyWaitTimeoutSecs}
                          min={0}
                          max={300}
                          value={schedulerConfig.stickyWaitTimeoutSecs}
                          onChange={(value) =>
                            setSchedulerConfig({ ...schedulerConfig, stickyWaitTimeoutSecs: value || 0 })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.stickyWaitMaxWaiting}
                          min={1}
                          max={64}
                          value={schedulerConfig.stickyWaitMaxWaiting}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              stickyWaitMaxWaiting: value || 1,
                            })
                          }
                        />
                      </div>
                      <div className="scheduler-grid scheduler-grid--toggles">
                        <CheckField
                          help={SCHEDULER_FIELD_HELP.stickyWaitEnabled}
                          checked={schedulerConfig.stickyWaitEnabled}
                          onChange={(checked) =>
                            setSchedulerConfig({ ...schedulerConfig, stickyWaitEnabled: checked })
                          }
                        />
                        <CheckField
                          help={SCHEDULER_FIELD_HELP.stickyWeightedEnabled}
                          checked={schedulerConfig.stickyWeightedEnabled}
                          onChange={(checked) =>
                            setSchedulerConfig({ ...schedulerConfig, stickyWeightedEnabled: checked })
                          }
                        />
                      </div>
                    </div>

                    <div className="settings-group">
                      <h3 className="settings-group__title">兜底排队</h3>
                      <div className="scheduler-grid scheduler-grid--toggles">
                        <CheckField
                          help={SCHEDULER_FIELD_HELP.fallbackWaitEnabled}
                          checked={schedulerConfig.fallbackWaitEnabled}
                          onChange={(checked) =>
                            setSchedulerConfig({ ...schedulerConfig, fallbackWaitEnabled: checked })
                          }
                        />
                      </div>
                      <div className="scheduler-grid">
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.fallbackWaitTimeoutSecs}
                          min={0}
                          max={300}
                          value={schedulerConfig.fallbackWaitTimeoutSecs}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              fallbackWaitTimeoutSecs: value || 0,
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.fallbackMaxWaiting}
                          min={1}
                          max={1000}
                          value={schedulerConfig.fallbackMaxWaiting}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              fallbackMaxWaiting: value || 1,
                            })
                          }
                        />
                        <label className="field">
                          <FieldLabel help={SCHEDULER_FIELD_HELP.fallbackSelectionMode} />
                          <select
                            value={schedulerConfig.fallbackSelectionMode}
                            onChange={(event) =>
                              setSchedulerConfig({
                                ...schedulerConfig,
                                fallbackSelectionMode:
                                  event.target.value === "random" ? "random" : "last_used",
                              })
                            }
                          >
                            <option value="last_used">last_used（最久未用）</option>
                            <option value="random">random（随机）</option>
                          </select>
                        </label>
                      </div>
                    </div>

                    <div className="settings-group">
                      <h3 className="settings-group__title">逃逸与额度</h3>
                      <div className="scheduler-grid scheduler-grid--toggles">
                        <CheckField
                          help={SCHEDULER_FIELD_HELP.stickyEscapeEnabled}
                          checked={schedulerConfig.stickyEscape.enabled}
                          onChange={(checked) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              stickyEscape: { ...schedulerConfig.stickyEscape, enabled: checked },
                            })
                          }
                        />
                        <CheckField
                          help={SCHEDULER_FIELD_HELP.preferSoonestReset}
                          checked={schedulerConfig.preferSoonestReset}
                          onChange={(checked) =>
                            setSchedulerConfig({ ...schedulerConfig, preferSoonestReset: checked })
                          }
                        />
                      </div>
                      <div className="scheduler-grid">
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.stickyEscapeTtftMs}
                          min={0}
                          value={schedulerConfig.stickyEscape.ttftMs}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              stickyEscape: { ...schedulerConfig.stickyEscape, ttftMs: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.stickyEscapeErrorRate}
                          min={0}
                          max={1}
                          step={0.05}
                          value={schedulerConfig.stickyEscape.errorRate}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              stickyEscape: { ...schedulerConfig.stickyEscape, errorRate: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.quotaAutoPause5h}
                          min={0}
                          max={1}
                          step={0.05}
                          value={schedulerConfig.quotaAutoPause5h}
                          onChange={(value) => setSchedulerConfig({ ...schedulerConfig, quotaAutoPause5h: value || 0 })}
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.quotaAutoPause7d}
                          min={0}
                          max={1}
                          step={0.05}
                          value={schedulerConfig.quotaAutoPause7d}
                          onChange={(value) => setSchedulerConfig({ ...schedulerConfig, quotaAutoPause7d: value || 0 })}
                        />
                      </div>
                    </div>

                    <div className="settings-group">
                      <h3 className="settings-group__title">评分权重</h3>
                      <p className="settings-group__hint">Top-K 加权选号时各因子的相对权重，通常保持默认即可。</p>
                      <div className="scheduler-grid">
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wPriority}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.priority}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: { ...schedulerConfig.scoreWeights, priority: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wLoad}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.load}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: { ...schedulerConfig.scoreWeights, load: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wQueue}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.queue}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: { ...schedulerConfig.scoreWeights, queue: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wError}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.errorRate}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: { ...schedulerConfig.scoreWeights, errorRate: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wTtft}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.ttft}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: { ...schedulerConfig.scoreWeights, ttft: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wReset}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.reset}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: { ...schedulerConfig.scoreWeights, reset: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wQuota}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.quotaHeadroom}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: { ...schedulerConfig.scoreWeights, quotaHeadroom: value || 0 },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wUpstreamCost}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.upstreamCost}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: {
                                ...schedulerConfig.scoreWeights,
                                upstreamCost: value || 0,
                              },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wPreviousResponse}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.previousResponse}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: {
                                ...schedulerConfig.scoreWeights,
                                previousResponse: value || 0,
                              },
                            })
                          }
                        />
                        <NumberField
                          help={SCHEDULER_FIELD_HELP.wSessionSticky}
                          min={0}
                          step={0.1}
                          value={schedulerConfig.scoreWeights.sessionSticky}
                          onChange={(value) =>
                            setSchedulerConfig({
                              ...schedulerConfig,
                              scoreWeights: {
                                ...schedulerConfig.scoreWeights,
                                sessionSticky: value || 0,
                              },
                            })
                          }
                        />
                      </div>
                    </div>
                  </div>
                ) : null}
              </div>

              <div className="form-actions form-actions--gap">
                <button
                  type="button"
                  className="button button--secondary"
                  disabled={busy}
                  onClick={() => setSchedulerConfig(defaultSchedulerConfig())}
                >
                  恢复默认
                </button>
                <button
                  type="button"
                  className="button button--primary"
                  disabled={busy}
                  onClick={() => void saveScheduler()}
                >
                  保存账号池设置
                </button>
              </div>
            </>
          )}
        </div>
      </section>

      <section className="panel" aria-label="诊断保留">
        <div className="panel__header">
          <div>
            <h2>诊断</h2>
            <p>代理请求事件环形保留条数（50–1000）。</p>
          </div>
        </div>
        <div className="settings-body settings-body--inline">
          <label className="field field--compact">
            <span>最大条数</span>
            <input
              type="number"
              min={50}
              max={1000}
              value={diagMax}
              onChange={(event) => setDiagMax(Number(event.target.value) || 200)}
            />
          </label>
          <button type="button" className="button button--secondary" disabled={busy} onClick={() => void saveDiagMax()}>
            保存
          </button>
        </div>
      </section>

      {message && <div className={message.startsWith("已") ? "inline-success" : "inline-warning"}>{message}</div>}

      <section className="panel about-panel" aria-label="About Codex Spur">
        <div className="panel__header">
          <div className="about-panel__brand">
            <img className="about-panel__icon" src={brandIcon} alt="" width={56} height={56} />
            <div>
              <h2>About</h2>
              <p>Codex Spur v0.1.0 · local-first</p>
            </div>
          </div>
        </div>
        <div className="about-panel__body">
          <p className="about-panel__lead">
            All your models. One Codex picker. One click to switch.
          </p>
          <figure className="about-panel__shot">
            <img
              src={modelPickerShot}
              alt="Codex model picker listing Grok, Kimi, DeepSeek, and OpenAI models"
            />
            <figcaption>
              Your configured models in the native Codex picker — one click to switch.
            </figcaption>
          </figure>
          <p>
            Wire up Kimi, DeepSeek, xAI, OpenAI multi-account, or any compatible gateway once. Enable what you
            want, Review &amp; Apply — and every selected model lands in the <strong>native Codex / ChatGPT
            Desktop model menu</strong>. Flip mid-flow the same way you switch official models: no extra tabs,
            no config rewrites, no client injection.
          </p>
          <p>
            <strong>Local-first privacy.</strong> API keys, session tokens, refresh tokens, and proxy bearers stay
            on this Mac — encrypted at rest, never exposed to the UI, never uploaded to a Codex Spur cloud.
          </p>
          <p>
            Closing the main window only hides the UI; the menu-bar proxy keeps running. Quit the app to stop the
            proxy and release leases.
          </p>
        </div>
      </section>
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

  const refresh = useCallback(async (opts?: { quiet?: boolean }) => {
    const quiet = opts?.quiet === true;
    if (!quiet) setLoading(true);
    try {
      setSnapshot(await getAppSnapshot());
    } finally {
      if (!quiet) setLoading(false);
    }
  }, []);

  const quietRefresh = useCallback(async () => {
    await refresh({ quiet: true });
  }, [refresh]);

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
      const authBlocked = (preview.warnings ?? []).some((warning) =>
        /auth\.json|官方登录/.test(warning),
      );
      if (authBlocked) {
        pushToast(
          "error",
          "Apply 已拦截：请先在 ChatGPT.app 登录官方账号（不是 Spur OAuth/API Key），再 Cmd+Q 重开后重试。",
        );
        for (const warning of preview.warnings ?? []) {
          if (/auth\.json|官方登录/.test(warning)) continue;
          pushToast("warning", warning);
        }
        return;
      }
      const outcome = await applyCodexConfig();
      await refresh();
      const labels = outcome.modelLabels ?? [];
      const hasCustom = labels.some((label) => /kimi|deepseek|minimax|k3|k2/i.test(label));
      const pathNote = outcome.configPath.includes(".codex")
        ? "已写入 ~/.codex"
        : `已写入 ${outcome.configPath}`;
      const listed =
        labels.length > 0
          ? labels.slice(0, 8).join(" · ") + (labels.length > 8 ? " …" : "")
          : `${outcome.modelCount} 个模型`;
      const customNote = hasCustom ? "（含第三方）" : "";
      const chatgptStillRunning = (outcome.warnings ?? []).some((warning) =>
        /仍在运行|Cmd\+Q|完全退出/.test(warning),
      );
      if (chatgptStillRunning) {
        pushToast(
          "success",
          `${pathNote}${customNote}：已发布 ${outcome.modelCount} 个模型（${listed}）。`,
        );
        // Catalog is loaded only at ChatGPT cold start — only nag when it is still running.
        pushToast(
          "error",
          "重要：请现在 Cmd+Q 退出 ChatGPT（不要只关窗口）。重开后在「高级 → 模型」中选择 Kimi / DeepSeek。",
        );
      } else {
        pushToast(
          "success",
          `${pathNote}${customNote}：已发布 ${outcome.modelCount} 个模型（${listed}）。打开 ChatGPT 后在「高级 → 模型」中应能看到完整列表。`,
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
            <button type="button" className="icon-button" aria-label="刷新" onClick={() => void refresh({})}>
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
      {addOpen && <AddProviderWizard onClose={() => setAddOpen(false)} onCreated={quietRefresh} />}
      {editProvider && (
        <EditProviderSheet
          key={editProvider.id}
          provider={snapshot.providers.find((item) => item.id === editProvider.id) ?? editProvider}
          onClose={() => setEditProvider(null)}
          onChanged={quietRefresh}
        />
      )}
    </div>
  );
}
