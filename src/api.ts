import { invoke } from "@tauri-apps/api/core";
import type {
  AccountPoolSummary,
  AppSnapshot,
  ApplyPreview,
  CodexApplyOutcome,
  CredentialSummary,
  ModelRouteSummary,
  OpenAiQuotaSnapshot,
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
  providers: [
    { id: "openai", name: "OpenAI", region: "Official", protocol: "Responses", configured: false, selectedModels: 0, discoveredModels: 0, lastFetchedAt: null, baseUrl: null, defaultBaseUrl: "https://chatgpt.com/backend-api/codex", supportsOfficialAccount: true, credentialCount: 0, healthyCredentialCount: 0, poolCount: 0 },
    { id: "kimi", name: "Kimi", region: "中国 / Global", protocol: "Chat Completions", configured: false, selectedModels: 0, discoveredModels: 0, lastFetchedAt: null, baseUrl: null, defaultBaseUrl: "https://api.kimi.com/coding/v1", supportsOfficialAccount: false, credentialCount: 0, healthyCredentialCount: 0, poolCount: 0 },
    { id: "deepseek", name: "DeepSeek", region: "Global", protocol: "Chat Completions", configured: false, selectedModels: 0, discoveredModels: 0, lastFetchedAt: null, baseUrl: null, defaultBaseUrl: "https://api.deepseek.com/v1", supportsOfficialAccount: false, credentialCount: 0, healthyCredentialCount: 0, poolCount: 0 },
    { id: "minimax", name: "MiniMax", region: "中国 / Global", protocol: "Responses preferred", configured: false, selectedModels: 0, discoveredModels: 0, lastFetchedAt: null, baseUrl: null, defaultBaseUrl: "https://api.minimaxi.com/v1", supportsOfficialAccount: false, credentialCount: 0, healthyCredentialCount: 0, poolCount: 0 },
  ],
  publishedModels: 0,
  healthyAccounts: 0,
  attentionItems: ["添加供应商凭据并拉取模型后，才能应用到 Codex。"],
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

export async function getUsageSnapshot(): Promise<UsageSnapshot> {
  if (!isTauriRuntime()) {
    return { requestCount: 0, inputTokens: 0, outputTokens: 0, totalTokens: 0, sevenDayTokens: 0, cacheHitRate: null, sampledAt: Date.now() };
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
