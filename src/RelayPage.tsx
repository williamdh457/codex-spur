import { useCallback, useEffect, useMemo, useState } from "react";
import {
  createRelayApiKey,
  deleteRelayApiKey,
  getApiRelayStatus,
  listModelRoutes,
  listRelayApiKeys,
  revealRelayApiKey,
  setApiRelaySettings,
  startApiRelay,
  stopApiRelay,
  updateRelayApiKey,
} from "./api";
import type {
  ApiRelayStatus,
  ModelRouteSummary,
  RelayApiKeySummary,
  StatusTone,
} from "./types";

function StatusDot({ tone }: { tone: StatusTone }) {
  return <span className={`status-dot status-dot--${tone}`} aria-hidden="true" />;
}

function EmptyState({
  title,
  body,
  action,
}: {
  title: string;
  body: string;
  action: string;
}) {
  return (
    <div className="empty-state">
      <strong>{title}</strong>
      <p>{body}</p>
      <span className="caption">{action}</span>
    </div>
  );
}

/** Machine id preview: 模型.供应商 (API / catalog slug). */
function dottedModelId(route: ModelRouteSummary): string {
  const model = (route.upstreamModel || route.displayName || route.id)
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  const provider = (route.providerName || route.providerId)
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return `${model || "model"}.${provider || "provider"}`;
}

/** Human label: 供应商 · 模型 (middle-dot separator). */
function humanModelLabel(route: ModelRouteSummary): string {
  const effective = (route.effectiveDisplayName || "").trim();
  if (effective) return effective;
  const fallback = (route.defaultDisplayName || "").trim();
  if (fallback) return fallback;
  const model = (route.upstreamModel || route.displayName || route.id).trim();
  const provider = (route.providerName || route.providerId).trim();
  if (provider && model) return `${provider} · ${model}`;
  return model || provider || route.id;
}

type BaseUrlEntry = {
  label: string;
  url: string;
};

type RelayTab = "basics" | "keys";

function baseUrlVariants(
  baseV1: string | null | undefined,
  lanV1: string | null | undefined,
): BaseUrlEntry[] {
  const out: BaseUrlEntry[] = [];
  if (baseV1) {
    out.push({ label: "本机 · OpenAI /v1", url: baseV1 });
    out.push({ label: "本机 · 根路径", url: baseV1.replace(/\/v1\/?$/, "") });
  }
  if (lanV1) {
    out.push({ label: "局域网 · OpenAI /v1", url: lanV1 });
    out.push({ label: "局域网 · 根路径", url: lanV1.replace(/\/v1\/?$/, "") });
  }
  return out;
}

export function RelayPage() {
  const [relayTab, setRelayTab] = useState<RelayTab>("basics");
  const [status, setStatus] = useState<ApiRelayStatus | null>(null);
  const [keys, setKeys] = useState<RelayApiKeySummary[]>([]);
  const [routes, setRoutes] = useState<ModelRouteSummary[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [messageTone, setMessageTone] = useState<"ok" | "error">("ok");
  /** key id → plaintext secret (local cache; always shown when available). */
  const [secretsById, setSecretsById] = useState<Record<string, string>>({});
  const [portDraft, setPortDraft] = useState("17862");
  const [newLabel, setNewLabel] = useState("");
  const [docsOpen, setDocsOpen] = useState(false);

  const relayRoutes = useMemo(
    () => routes.filter((route) => route.relayEnabled),
    [routes],
  );

  const urlEntries = useMemo(
    () => baseUrlVariants(status?.baseUrl, status?.lanBaseUrl),
    [status?.baseUrl, status?.lanBaseUrl],
  );

  const loadSecrets = useCallback(async (nextKeys: RelayApiKeySummary[]) => {
    const entries = await Promise.all(
      nextKeys.map(async (key) => {
        const secret = await revealRelayApiKey(key.id);
        return secret ? ([key.id, secret] as const) : null;
      }),
    );
    const map: Record<string, string> = {};
    for (const entry of entries) {
      if (entry) map[entry[0]] = entry[1];
    }
    setSecretsById(map);
  }, []);

  const reload = useCallback(async () => {
    const [nextStatus, nextKeys, nextRoutes] = await Promise.all([
      getApiRelayStatus(),
      listRelayApiKeys(),
      listModelRoutes(),
    ]);
    setStatus(nextStatus);
    setKeys(nextKeys);
    setRoutes(nextRoutes);
    setPortDraft(String(nextStatus.port));
    await loadSecrets(nextKeys);
  }, [loadSecrets]);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const [nextStatus, nextKeys, nextRoutes] = await Promise.all([
          getApiRelayStatus(),
          listRelayApiKeys(),
          listModelRoutes(),
        ]);
        if (!active) return;
        setStatus(nextStatus);
        setKeys(nextKeys);
        setRoutes(nextRoutes);
        setPortDraft(String(nextStatus.port));
        await loadSecrets(nextKeys);
      } catch (caught) {
        if (active) {
          setMessageTone("error");
          setMessage(caught instanceof Error ? caught.message : String(caught));
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [loadSecrets]);

  const copyText = async (label: string, value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setMessageTone("ok");
      setMessage(`已复制 ${label}`);
    } catch {
      setMessageTone("error");
      setMessage(`无法复制 ${label}，请手动选择。`);
    }
  };

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setMessage(null);
    try {
      await action();
      await reload();
    } catch (caught) {
      setMessageTone("error");
      setMessage(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  const baseV1 = status?.baseUrl ?? null;
  const exampleModel =
    relayRoutes[0] != null ? dottedModelId(relayRoutes[0]) : "deepseek-chat.deepseek";
  const keyCount = status?.keyCount ?? keys.length;
  const modelCount = status?.relayModelCount ?? relayRoutes.length;

  return (
    <div className="page-stack relay-page">
      <div className="model-scope" role="group" aria-label="反代设置">
        <button
          type="button"
          id="relay-scope-basics"
          aria-pressed={relayTab === "basics"}
          className={`model-scope__btn${relayTab === "basics" ? " model-scope__btn--active" : ""}`}
          onClick={() => setRelayTab("basics")}
        >
          <strong>基础信息</strong>
          <span>启停 · Base URL · 模型</span>
          <span className="model-scope__count">
            {status?.running ? "运行中" : "已停止"} · {modelCount} 模型
          </span>
        </button>
        <button
          type="button"
          id="relay-scope-keys"
          aria-pressed={relayTab === "keys"}
          className={`model-scope__btn${relayTab === "keys" ? " model-scope__btn--active" : ""}`}
          onClick={() => setRelayTab("keys")}
        >
          <strong>API Key</strong>
          <span>Client Keys 创建与复制</span>
          <span className="model-scope__count">{keyCount} 把</span>
        </button>
      </div>
      <p className="caption model-scope__hint">
        默认启动反代。模型是否外放在「模型」页勾选；这里管理服务与 Key。
        Review & Apply 会把反代模型 soft-sync 到 Z Code（SPUR）。
      </p>

      {message ? (
        <div
          className={messageTone === "error" ? "inline-warning" : "inline-success"}
          role="status"
        >
          {message}
        </div>
      ) : null}

      {relayTab === "basics" ? (
        <>
          <section className="panel relay-status" aria-labelledby="relay-scope-basics">
            <div className="panel__header">
              <div>
                <h2>反代中转站</h2>
                <p>
                  把已开「反代」的模型以 Base URL + API Key 外放。一把 Key 同时支持{" "}
                  <code>/v1/responses</code> 与 <code>/v1/chat/completions</code>
                  ；模型 id 为 <code>模型.供应商</code>。
                </p>
              </div>
              <StatusDot tone={status?.running ? "healthy" : "muted"} />
            </div>

            <dl className="diagnostic-grid">
              <div>
                <dt>状态</dt>
                <dd>{status?.running ? "运行中" : "已停止"}</dd>
              </div>
              <div>
                <dt>反代模型</dt>
                <dd>{modelCount}</dd>
              </div>
            </dl>

            {urlEntries.length > 0 ? (
              <div className="relay-base-urls">
                <div className="relay-base-urls__title">Base URL</div>
                <ul className="relay-base-urls__list">
                  {urlEntries.map((entry) => (
                    <li key={entry.label + entry.url} className="relay-base-urls__item">
                      <span className="relay-base-urls__label">{entry.label}</span>
                      <code className="mono-copy">{entry.url}</code>
                      <button
                        type="button"
                        className="button button--ghost"
                        disabled={busy}
                        onClick={() => void copyText(entry.label, entry.url)}
                      >
                        复制
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
            ) : (
              <p className="caption" style={{ margin: "0 14px 12px" }}>
                启动反代后显示 Base URL。
              </p>
            )}

            <div className="form-actions form-actions--wrap relay-status__actions">
              {status?.running ? (
                <button
                  type="button"
                  className="button button--secondary"
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      await stopApiRelay();
                      setMessageTone("ok");
                      setMessage("API 反代已停止（下次启动将保持关闭）");
                    })
                  }
                >
                  停止反代
                </button>
              ) : (
                <button
                  type="button"
                  className="button button--primary"
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      await startApiRelay();
                      setMessageTone("ok");
                      setMessage("API 反代已启动");
                    })
                  }
                >
                  启动反代
                </button>
              )}
              <label className="inline-field">
                端口
                <input
                  className="input input--narrow"
                  inputMode="numeric"
                  value={portDraft}
                  disabled={busy}
                  onChange={(event) => setPortDraft(event.target.value)}
                  onBlur={() => {
                    const port = Number(portDraft);
                    if (!Number.isFinite(port) || port < 1 || port > 65535) return;
                    if (status && port === status.port) return;
                    void run(async () => {
                      await setApiRelaySettings({ port });
                      setMessageTone("ok");
                      setMessage(`端口已设为 ${port}`);
                    });
                  }}
                />
              </label>
              <label className="inline-field">
                <input
                  type="checkbox"
                  checked={Boolean(status?.bindLan)}
                  disabled={busy}
                  onChange={(event) => {
                    const bindLan = event.target.checked;
                    void run(async () => {
                      await setApiRelaySettings({ bindLan });
                      setMessageTone("ok");
                      setMessage(
                        bindLan
                          ? "已允许局域网访问（持 Key 的设备可调用，请谨慎）"
                          : "已仅绑定本机 127.0.0.1",
                      );
                    });
                  }}
                />
                允许局域网
              </label>
            </div>
            {status?.bindLan ? (
              <div className="inline-warning" role="status">
                局域网模式下，同一网段内持有 API Key 的设备均可调用。仅在可信网络开启。
              </div>
            ) : null}
            {status?.lastError ? (
              <div className="inline-warning" role="status">
                {status.lastError}
              </div>
            ) : null}
          </section>

          <section className="panel">
            <div className="panel__header">
              <div>
                <h2>可反代模型</h2>
                <p>
                  在「模型」页打开反代开关后出现在此。展示名 供应商 · 模型；对外 id 为 模型.供应商。
                </p>
              </div>
            </div>
            {relayRoutes.length === 0 ? (
              <EmptyState
                title="还没有开启反代的模型"
                body="到「模型」页打开某一行的「反代」开关，再回到这里复制 model id。"
                action="前往模型页"
              />
            ) : (
              <div className="relay-model-table" role="table">
                <div className="relay-model-table__head" role="row">
                  <span role="columnheader">对外 id</span>
                  <span role="columnheader">显示名</span>
                  <span role="columnheader">上游协议</span>
                  <span role="columnheader">供应商</span>
                </div>
                {relayRoutes.map((route) => (
                  <div className="relay-model-table__row" role="row" key={route.id}>
                    <code role="cell">{dottedModelId(route)}</code>
                    <span role="cell">{humanModelLabel(route)}</span>
                    <span role="cell">{route.protocol}</span>
                    <span role="cell">{route.providerName || route.providerId}</span>
                  </div>
                ))}
              </div>
            )}
          </section>

          <section className="panel">
            <div className="panel__header">
              <div>
                <h2>接入说明</h2>
                <p>同一 Key 可用于 Responses 与 Chat Completions。</p>
              </div>
              <button
                type="button"
                className="button button--ghost"
                aria-expanded={docsOpen}
                onClick={() => setDocsOpen((open) => !open)}
              >
                {docsOpen ? "收起" : "展开示例"}
              </button>
            </div>
            {docsOpen ? (
              <div className="relay-docs-inline">
                <p className="caption">
                  模型 id：<code>模型.供应商</code>
                </p>
                <pre className="relay-code">{`# Responses
curl -sS ${baseV1 ?? "http://127.0.0.1:17862/v1"}/responses \\
  -H "Authorization: Bearer sk-spur-…" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"${exampleModel}","input":"hi","stream":true}'

# Chat Completions
curl -sS ${baseV1 ?? "http://127.0.0.1:17862/v1"}/chat/completions \\
  -H "Authorization: Bearer sk-spur-…" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"${exampleModel}","messages":[{"role":"user","content":"hi"}]}'
`}</pre>
              </div>
            ) : null}
          </section>
        </>
      ) : (
        <section className="panel" aria-labelledby="relay-scope-keys">
          <div className="panel__header">
            <div>
              <h2>Client Keys</h2>
              <p>
                一把 Key 双入口。明文常显（本机缓存）；模型是否外放请在「模型」页勾选反代开关。
              </p>
            </div>
          </div>

          <div className="relay-create">
            <input
              className="input"
              placeholder="名称（可选）"
              value={newLabel}
              disabled={busy}
              onChange={(event) => setNewLabel(event.target.value)}
            />
            <button
              type="button"
              className="button button--secondary"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  const created = await createRelayApiKey({
                    label: newLabel.trim() || undefined,
                    nameStyle: "dotted",
                  });
                  setSecretsById((prev) => ({ ...prev, [created.key.id]: created.secret }));
                  setNewLabel("");
                  setMessageTone("ok");
                  setMessage(`已创建 Key「${created.key.label}」`);
                })
              }
            >
              新建 Key
            </button>
          </div>

          <div className="relay-key-list">
            {keys.length === 0 ? (
              <EmptyState
                title="还没有 Client Key"
                body="创建一把 Key，再把 Base URL 给第三方客户端。"
                action="等待创建"
              />
            ) : (
              keys.map((key) => {
                const secret = secretsById[key.id];
                return (
                  <div key={key.id} className="relay-key-row">
                    <button
                      type="button"
                      className={`switch${key.enabled ? " switch--on" : ""}`}
                      role="switch"
                      aria-checked={key.enabled}
                      aria-label={`${key.enabled ? "关闭" : "开启"} ${key.label}`}
                      disabled={busy}
                      onClick={() => {
                        void run(async () => {
                          await updateRelayApiKey({ id: key.id, enabled: !key.enabled });
                        });
                      }}
                    >
                      <span className="switch__track" aria-hidden="true" />
                    </button>
                    <span className="relay-key-row__main">
                      <strong>{key.label}</strong>
                      <span className="relay-key-secret">
                        {secret ? (
                          <>
                            <code className="mono-copy">{secret}</code>
                            <button
                              type="button"
                              className="button button--ghost"
                              disabled={busy}
                              onClick={() => void copyText("API Key", secret)}
                            >
                              复制 Key
                            </button>
                          </>
                        ) : (
                          <small className="caption">
                            明文不可恢复（仅有 <code>{key.keyPrefix}</code>
                            ）。删除后新建即可常显。
                          </small>
                        )}
                      </span>
                    </span>
                    <button
                      type="button"
                      className="button button--ghost relay-key-row__delete"
                      disabled={busy}
                      aria-label={`删除 ${key.label}`}
                      onClick={() =>
                        void run(async () => {
                          await deleteRelayApiKey(key.id);
                          setSecretsById((prev) => {
                            const next = { ...prev };
                            delete next[key.id];
                            return next;
                          });
                          setMessageTone("ok");
                          setMessage(`已删除「${key.label}」`);
                        })
                      }
                    >
                      删除
                    </button>
                  </div>
                );
              })
            )}
          </div>
        </section>
      )}
    </div>
  );
}
