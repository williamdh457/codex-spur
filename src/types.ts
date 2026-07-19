export type NavigationSection =
  | "overview"
  | "providers"
  | "models"
  | "accounts"
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

export type ProviderSummary = {
  id: string;
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
};
