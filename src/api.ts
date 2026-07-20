import { invoke } from "@tauri-apps/api/core";
import type {
  AccountPoolSummary,
  PoolMemberDetail,
  PoolSchedulerConfig,
  ProviderRouting,
  ProxyRequestEvent,
  AppSnapshot,
  ApplyPreview,
  CodexApplyOutcome,
  CredentialSummary,
  ModelRouteSummary,
  OpenAiQuotaSnapshot,
  ProviderSummary,
  UsageSnapshot,
} from "./types";

const browserFallback: AppSnapshot = {
  proxy: {
    running: true,
    baseUrl: "http://127.0.0.1:17861/v1",
    port: 17861,
    catalogRevision: "draft",
    lastError: null,
  },
  binding: {
    state: "not_applied",
    codexHome: "~/.codex",
    providerId: "codex_select",
    catalogPath: "~/.codex/codex-select/model-catalog.json",
  },
  // Empty by default — CC Switch style: user adds instances.
  providers: [],
  publishedModels: 0,
  healthyAccounts: 0,
  attentionItems: ["添加供应商并拉取模型后，才能应用到 Codex。"],
  desktopVisibility: {
    ready: false,
    statusLabel: "待应用",
    codexHome: "~/.codex",
    checks: [
      {
        id: "chatgpt_auth",
        label: "ChatGPT 官方登录",
        ok: false,
        detail: "浏览器预览无真实 ~/.codex；请用 Tauri 桌面端查看。",
      },
    ],
  },
};

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export async function getAppSnapshot(): Promise<AppSnapshot> {
  return isTauriRuntime() ? invoke<AppSnapshot>("get_app_snapshot") : browserFallback;
}

export async function previewCodexApply(): Promise<ApplyPreview> {
  if (!isTauriRuntime()) {
    return {
      providerId: "codex_select",
      baseUrl: "http://127.0.0.1:17861/v1",
      catalogPath: "~/.codex/codex-select/model-catalog.json",
      selectedModel: null,
      modelCount: 0,
      tomlPreview: 'model_provider = "codex_select"\nmodel_catalog_json = "~/.codex/codex-select/model-catalog.json"',
      warnings: ["当前没有已选择模型，Apply 会被阻止。"],
    };
  }
  return invoke<ApplyPreview>("preview_codex_apply");
}

export async function applyCodexConfig(): Promise<CodexApplyOutcome> {
  return invoke<CodexApplyOutcome>("apply_codex_config");
}

export async function restorePreviousCodexConfig(): Promise<string | null> {
  return invoke<string | null>("restore_previous_codex_config");
}

export async function listModelRoutes(): Promise<ModelRouteSummary[]> {
  return isTauriRuntime() ? invoke<ModelRouteSummary[]>("list_model_routes") : [];
}

export async function discoverProviderModels(providerId: string, baseUrl: string, apiKey?: string): Promise<ModelRouteSummary[]> {
  if (!isTauriRuntime()) return [];
  return invoke<ModelRouteSummary[]>("discover_provider_models", { providerId, baseUrl, apiKey: apiKey || null });
}

export async function importProviderConfigJson(providerId: string, input: string): Promise<ModelRouteSummary[]> {
  if (!isTauriRuntime()) return [];
  return invoke<ModelRouteSummary[]>("import_provider_config_json", { providerId, input });
}

export async function createProviderInstance(kind: string, name?: string): Promise<ProviderSummary> {
  return invoke<ProviderSummary>("create_provider_instance", { kind, name: name ?? null });
}

export type DeviceLoginStart = {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  intervalSecs: number;
  expiresIn: number;
};

export type DeviceLoginTokens = {
  accessToken: string;
  refreshToken: string | null;
  idToken: string | null;
  accountId: string;
  email: string | null;
  expiresAt: number | null;
};

export type DeviceLoginPoll = {
  status: string;
  tokens: DeviceLoginTokens | null;
  message: string | null;
  /** Present for xAI slow_down responses. */
  intervalSecs?: number | null;
};

export type OpenAiLoginComplete = {
  provider: ProviderSummary;
  modelCount: number;
  modelError: string | null;
};

/** Browser PKCE start — no secrets. */
export type BrowserLoginStart = {
  authUrl: string;
  redirectUri: string;
};

/** Event payload from Rust after browser OAuth finishes (no tokens). */
export type OpenAiOAuthFinishedEvent = {
  ok: boolean;
  provider: ProviderSummary | null;
  modelCount: number;
  modelError: string | null;
  message: string | null;
};

export async function startOpenAiBrowserLogin(name?: string): Promise<BrowserLoginStart> {
  return invoke<BrowserLoginStart>("start_openai_browser_login", {
    name: name ?? null,
  });
}

export async function cancelOpenAiBrowserLogin(): Promise<void> {
  return invoke<void>("cancel_openai_browser_login");
}

/** Manual fallback when localhost redirect is blocked. */
export async function completeOpenAiOauthCallbackUrl(
  callbackUrl: string,
): Promise<OpenAiLoginComplete> {
  return invoke<OpenAiLoginComplete>("complete_openai_oauth_callback_url", {
    callbackUrl,
  });
}

export async function startOpenAiDeviceLogin(): Promise<DeviceLoginStart> {
  return invoke<DeviceLoginStart>("start_openai_device_login");
}

export async function pollOpenAiDeviceLogin(deviceCode: string): Promise<DeviceLoginPoll> {
  return invoke<DeviceLoginPoll>("poll_openai_device_login", { deviceCode });
}

export async function cancelOpenAiDeviceLogin(deviceCode: string): Promise<void> {
  return invoke<void>("cancel_openai_device_login", { deviceCode });
}

export async function completeOpenAiDeviceLogin(
  tokens: DeviceLoginTokens,
  name?: string,
): Promise<OpenAiLoginComplete> {
  return invoke<OpenAiLoginComplete>("complete_openai_device_login", {
    tokens,
    name: name ?? null,
  });
}

export async function startXaiDeviceLogin(): Promise<DeviceLoginStart> {
  return invoke<DeviceLoginStart>("start_xai_device_login");
}

export async function pollXaiDeviceLogin(deviceCode: string): Promise<DeviceLoginPoll> {
  return invoke<DeviceLoginPoll>("poll_xai_device_login", { deviceCode });
}

export async function cancelXaiDeviceLogin(deviceCode: string): Promise<void> {
  return invoke<void>("cancel_xai_device_login", { deviceCode });
}

export async function completeXaiDeviceLogin(
  tokens: DeviceLoginTokens,
  name?: string,
): Promise<OpenAiLoginComplete> {
  return invoke<OpenAiLoginComplete>("complete_xai_device_login", {
    tokens,
    name: name ?? null,
  });
}

export async function openExternalUrl(url: string): Promise<void> {
  return invoke<void>("open_external_url", { url });
}

export async function deleteProviderInstance(providerId: string): Promise<void> {
  return invoke<void>("delete_provider_instance", { providerId });
}

export async function renameProviderInstance(providerId: string, name: string): Promise<ProviderSummary> {
  return invoke<ProviderSummary>("rename_provider_instance", { providerId, name });
}

export async function setActivePool(providerId: string, poolId: string): Promise<void> {
  return invoke<void>("set_active_pool", { providerId, poolId });
}

export async function setModelEnabled(routeId: string, enabled: boolean): Promise<ModelRouteSummary[]> {
  if (!isTauriRuntime()) return [];
  return invoke<ModelRouteSummary[]>("set_model_enabled", { routeId, enabled });
}

export async function listCredentials(providerId?: string): Promise<CredentialSummary[]> {
  return isTauriRuntime() ? invoke<CredentialSummary[]>("list_credentials", { providerId: providerId ?? null }) : [];
}

export async function importCredentialsJson(providerId: string, input: string): Promise<CredentialSummary[]> {
  if (!isTauriRuntime()) return [];
  return invoke<CredentialSummary[]>("import_credentials_json", { providerId, input });
}

export async function testAccount(credentialId: string, modelId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  return invoke<void>("test_account", { credentialId, modelId });
}

export async function listAccountPools(): Promise<AccountPoolSummary[]> {
  return isTauriRuntime() ? invoke<AccountPoolSummary[]>("list_account_pools") : [];
}

export async function createAccountPool(providerId: string, name: string): Promise<string> {
  return invoke<string>("create_account_pool", { providerId, name });
}

export async function addAccountToPool(poolId: string, credentialId: string): Promise<void> {
  return invoke<void>("add_account_to_pool", { poolId, credentialId });
}

export async function removeAccountFromPool(poolId: string, credentialId: string): Promise<void> {
  return invoke<void>("remove_account_from_pool", { poolId, credentialId });
}

export async function listPoolMemberIds(poolId: string): Promise<string[]> {
  return isTauriRuntime() ? invoke<string[]>("list_pool_member_ids", { poolId }) : [];
}

export async function listPoolMembersDetailed(poolId: string): Promise<PoolMemberDetail[]> {
  return isTauriRuntime() ? invoke<PoolMemberDetail[]>("list_pool_members_detailed", { poolId }) : [];
}

export async function updatePoolMember(
  poolId: string,
  credentialId: string,
  weight: number,
  priority: number,
  enabled: boolean,
  concurrencyLimit: number,
): Promise<void> {
  return invoke<void>("update_pool_member", {
    poolId,
    credentialId,
    weight,
    priority,
    enabled,
    concurrencyLimit,
  });
}

export async function getProviderRouting(providerId: string): Promise<ProviderRouting | null> {
  return isTauriRuntime() ? invoke<ProviderRouting | null>("get_provider_routing", { providerId }) : null;
}

export async function setProviderRouting(
  providerId: string,
  routingMode: string,
  fixedCredentialId: string | null,
): Promise<ProviderRouting> {
  return invoke<ProviderRouting>("set_provider_routing", {
    providerId,
    routingMode,
    fixedCredentialId,
  });
}

export async function getPoolSchedulerConfig(poolId: string): Promise<PoolSchedulerConfig> {
  return invoke<PoolSchedulerConfig>("get_pool_scheduler_config", { poolId });
}

export async function updatePoolSchedulerConfig(
  poolId: string,
  config: PoolSchedulerConfig,
): Promise<PoolSchedulerConfig> {
  return invoke<PoolSchedulerConfig>("update_pool_scheduler_config", { poolId, config });
}

export async function listProxyRequestEvents(limit = 100): Promise<ProxyRequestEvent[]> {
  return isTauriRuntime()
    ? invoke<ProxyRequestEvent[]>("list_proxy_request_events", { limit })
    : [];
}

export async function clearProxyRequestEvents(): Promise<void> {
  if (!isTauriRuntime()) return;
  return invoke<void>("clear_proxy_request_events");
}

export async function getDiagnosticsMaxEvents(): Promise<number> {
  return isTauriRuntime() ? invoke<number>("get_diagnostics_max_events") : 200;
}

export async function setDiagnosticsMaxEvents(maxEvents: number): Promise<number> {
  return invoke<number>("set_diagnostics_max_events", { maxEvents });
}

export async function getUsageSnapshot(): Promise<UsageSnapshot> {
  if (!isTauriRuntime()) {
    return { requestCount: 0, inputTokens: 0, outputTokens: 0, totalTokens: 0, todayTokens: 0, sevenDayTokens: 0, cacheHitRate: null, failedRequests: 0, sampledAt: Date.now() };
  }
  return invoke<UsageSnapshot>("get_usage_snapshot");
}

export async function refreshOpenAiQuota(credentialId: string): Promise<OpenAiQuotaSnapshot> {
  return invoke<OpenAiQuotaSnapshot>("refresh_openai_quota", { credentialId });
}

export async function getCachedOpenAiQuota(credentialId: string): Promise<OpenAiQuotaSnapshot | null> {
  return isTauriRuntime() ? invoke<OpenAiQuotaSnapshot | null>("get_cached_openai_quota", { credentialId }) : null;
}

export async function consumeOpenAiResetCredit(credentialId: string, idempotencyKey: string, confirmed: boolean): Promise<OpenAiQuotaSnapshot> {
  return invoke<OpenAiQuotaSnapshot>("consume_openai_reset_credit", { credentialId, idempotencyKey, confirmed });
}
