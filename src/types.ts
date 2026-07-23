export type NavigationSection =
  | "overview"
  | "models"
  | "usage"
  | "diagnostics"
  | "settings";

export type StatusTone = "healthy" | "warning" | "error" | "muted";
export type ReasoningEffort = "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";

export type ProxyStatus = {
  running: boolean;
  baseUrl: string | null;
  port: number | null;
  catalogRevision: string;
  lastError: string | null;
};

export type CodexBindingStatus = {
  state: "not_applied" | "applied" | "changed" | "invalid";
  codexHome: string;
  providerId: string;
  catalogPath: string;
};

export type ProviderKind = "openai" | "xai" | "kimi" | "deepseek" | "minimax" | "opencode-go" | "custom";

export type ProviderSummary = {
  id: string;
  kind: ProviderKind;
  name: string;
  region: string;
  protocol: string;
  configured: boolean;
  selectedModels: number;
  discoveredModels: number;
  lastFetchedAt: string | null;
  baseUrl: string | null;
  defaultBaseUrl: string | null;
  supportsOfficialAccount: boolean;
  credentialCount: number;
  healthyCredentialCount: number;
  poolCount: number;
  activePoolId: string | null;
  routingMode: string;
  fixedCredentialId: string | null;
  /** Entry channel badge: official (browser) | json (file import) | api (form key). Legacy pool/config normalize to json. */
  entryCategory: "official" | "json" | "api" | "pool" | "config" | null;
};

export type ReasoningMapping = {
  codexEffort: ReasoningEffort;
  upstreamEffort: string;
  explanation: string;
};

export type ReasoningProfile = {
  title: string;
  mappings: ReasoningMapping[];
};

export type ModelRouteSummary = {
  id: string;
  providerId: string;
  /** User-facing provider instance name. */
  providerName: string;
  upstreamModel: string;
  displayName: string;
  enabled: boolean;
  protocol: string;
  baseUrl: string;
  reasoningProfile: ReasoningProfile;
};

export type CredentialSummary = {
  id: string;
  providerId: string;
  kind: string;
  state: string;
  label: string | null;
  maskedEmail: string | null;
  maskedAccountId: string | null;
  expiresAt: number | null;
  fingerprintPrefix: string;
  refreshable: boolean;
  healthy: boolean;
  lastError: string | null;
};

export type DeleteCredentialResult = {
  providerId: string;
  remainingAccounts: number;
};

export type AccountPoolSummary = {
  id: string;
  name: string;
  providerId: string;
  strategy: string;
  stickyTtlSecs: number;
  enabled: boolean;
  accountCount: number;
  healthyCount: number;
};

export type ProviderRouting = {
  providerId: string;
  routingMode: string;
  fixedCredentialId: string | null;
  activePoolId: string | null;
};

export type PoolMemberDetail = {
  poolId: string;
  credentialId: string;
  weight: number;
  priority: number;
  enabled: boolean;
  concurrencyLimit: number;
  upstreamCostRate: number;
  label: string | null;
  maskedEmail: string | null;
  healthy: boolean;
  scheduleState: string;
  cooldownUntil: number | null;
  lastError: string | null;
};

export type ScoreWeights = {
  priority: number;
  load: number;
  queue: number;
  errorRate: number;
  ttft: number;
  reset: number;
  quotaHeadroom: number;
  upstreamCost: number;
  previousResponse: number;
  sessionSticky: number;
};

export type StickyEscapeConfig = {
  enabled: boolean;
  ttftMs: number;
  errorRate: number;
};

export type FallbackSelectionMode = "last_used" | "random";

export type PoolSchedulerConfig = {
  lbTopK: number;
  stickySessionTtlSecs: number;
  stickyResponseIdTtlSecs: number;
  scoreWeights: ScoreWeights;
  stickyEscape: StickyEscapeConfig;
  preferSoonestReset: boolean;
  default429CooldownSecs: number;
  maxFailoverSwitches: number;
  leaseTtlSecs: number;
  /** Hard-filter accounts whose fresh quota remaining is ~0. */
  excludeZeroQuota: boolean;
  /** Used-fraction threshold for 5h auto-pause (0 disables). */
  quotaAutoPause5h: number;
  /** Used-fraction threshold for 7d auto-pause (0 disables). */
  quotaAutoPause7d: number;
  /** Wait for sticky account concurrency instead of switching. */
  stickyWaitEnabled: boolean;
  stickyWaitTimeoutSecs: number;
  /** Max concurrent waiters for one sticky account (Sub2API=3). */
  stickyWaitMaxWaiting: number;
  /** When all accounts are full, wait for any free slot. */
  fallbackWaitEnabled: boolean;
  fallbackWaitTimeoutSecs: number;
  fallbackMaxWaiting: number;
  fallbackSelectionMode: FallbackSelectionMode;
  /** Soft sticky via score bonuses instead of hard affinity. */
  stickyWeightedEnabled: boolean;
  rateLimit429CooldownEnabled: boolean;
  /** Cooldown after 529 overloaded (seconds; Sub2API default 10 min). */
  overload529CooldownSecs: number;
  /** Allow failover on selected 400 errors. */
  failoverOn400: boolean;
};

export type ProxyRequestEvent = {
  id: string;
  createdAt: string;
  routeSlug: string | null;
  displayName: string | null;
  providerId: string | null;
  upstreamModel: string | null;
  protocol: string | null;
  selectionLayer: string;
  stickyEscaped: boolean;
  accountFingerprint: string | null;
  scheduleState: string | null;
  resultCategory: string;
  failoverAttempt: number;
  latencyMsTotal: number | null;
  firstTokenMs: number | null;
  cooldownApplied: boolean;
  errorSummary: string | null;
};

export type QuotaWindow = {
  usedPercent: number;
  remainingPercent: number;
  resetAt: number | null;
  windowSeconds: number;
};

export type ResetCreditsSummary = {
  availableCount: number | null;
  credits: Array<{ grantedAt: number | null; expiresAt: number | null }>;
};

export type OpenAiQuotaSnapshot = {
  credentialId: string;
  planType: string | null;
  fiveHour: QuotaWindow | null;
  sevenDay: QuotaWindow | null;
  resetCredits: ResetCreditsSummary | null;
  fetchedAt: number;
};

export type UsageSnapshot = {
  requestCount: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  todayTokens: number;
  sevenDayTokens: number;
  cacheHitRate: number | null;
  failedRequests: number;
  sampledAt: number;
};

export type UsageRange = "7d" | "30d" | "all";
export type UsageTrendPoint = {
  day: string;
  requestCount: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  failedRequests: number;
  cacheHitRate: number | null;
};
export type UsageBreakdown = {
  name: string;
  requestCount: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  failedRequests: number;
  tokenShare: number;
};
export type UsageDashboardSnapshot = {
  range: UsageRange;
  requestCount: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  todayTokens: number;
  selectedRangeTokens: number;
  failedRequests: number;
  failureRate: number | null;
  cacheHitRate: number | null;
  sampledAt: number;
  trend: UsageTrendPoint[];
  models: UsageBreakdown[];
  providers: UsageBreakdown[];
};

export type DesktopVisibilityCheck = {
  id: string;
  label: string;
  ok: boolean;
  detail: string;
};

/** Whether ChatGPT Desktop can show custom catalog rows (Kimi/DeepSeek). */
export type DesktopVisibility = {
  ready: boolean;
  /** 就绪 / 缺登录 / 待应用 / 异常 */
  statusLabel: string;
  codexHome: string;
  checks: DesktopVisibilityCheck[];
};

export type AppSnapshot = {
  proxy: ProxyStatus;
  binding: CodexBindingStatus;
  providers: ProviderSummary[];
  publishedModels: number;
  healthyAccounts: number;
  attentionItems: string[];
  desktopVisibility: DesktopVisibility;
};

export type ApplyPreview = {
  providerId: string;
  baseUrl: string;
  catalogPath: string;
  selectedModel: string | null;
  modelCount: number;
  tomlPreview: string;
  warnings: string[];
};

export type CodexApplyOutcome = {
  configPath: string;
  catalogPath: string;
  backupPath: string | null;
  beforeHash: string | null;
  afterHash: string;
  restartRequired: boolean;
  modelCount: number;
  selectedModel: string | null;
  modelLabels?: string[];
  warnings?: string[];
};

/** Experimental Kimi Desktop publisher status (config/cache only). */
export type KimiTargetStatus = {
  installed: boolean;
  appVersion: string | null;
  versionSupported: boolean;
  userDir: string;
  cachePath: string;
  configPath: string;
  runtimeTomlPath: string;
  controlUrl: string | null;
  controlReady: boolean;
  lastPublishAt: string | null;
  lastModelCount: number | null;
  /** Persisted: last 启用发布 succeeded. */
  publishActive: boolean;
  warnings: string[];
};

export type KimiPublishPreview = {
  experimental: boolean;
  kimiVersion: string | null;
  gatewayBaseUrl: string;
  modelCount: number;
  modelLabels: string[];
  cachePath: string;
  configPath: string;
  runtimeTomlPath: string;
  cachePreview: string;
  configDiffSummary: string;
  tomlDiffSummary: string;
  warnings: string[];
};

export type KimiPublishOutcome = {
  experimental: boolean;
  modelCount: number;
  modelLabels: string[];
  backupDir: string;
  cachePath: string;
  configPath: string;
  runtimeTomlPath: string;
  controlUpdated: boolean;
  restartRecommended: boolean;
  warnings: string[];
};

/** Local CONNECT proxy that blocks www.kimi.com for Work model-list fallback. */
export type KimiListShieldStatus = {
  running: boolean;
  port: number | null;
  listen: string | null;
  blockedHosts: string[];
  blockedConnects: number;
  tunneledConnects: number;
  note: string;
};

/** One-shot enable/disable result for the two-button Kimi publish UI. */
export type KimiPublishToggleResult = {
  enabled: boolean;
  modelCount: number;
  modelLabels: string[];
  shieldListen: string | null;
  proxyOk: boolean;
  message: string;
  warnings: string[];
};
