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

export type ProviderKind = "openai" | "kimi" | "deepseek" | "minimax" | "custom";

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
};

export type StickyEscapeConfig = {
  enabled: boolean;
  ttftMs: number;
  errorRate: number;
};

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

export type AppSnapshot = {
  proxy: ProxyStatus;
  binding: CodexBindingStatus;
  providers: ProviderSummary[];
  publishedModels: number;
  healthyAccounts: number;
  attentionItems: string[];
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
