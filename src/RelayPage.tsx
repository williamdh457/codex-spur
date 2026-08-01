import { useCallback, useEffect, useMemo, useState } from "react";
import {
  createRelayApiKey,
  deleteRelayApiKey,
  getApiRelayStatus,
  listModelRoutes,
  listRelayApiKeys,
  regenerateRelayApiKey,
  revealDefaultRelayApiKey,
  setApiRelaySettings,
  startApiRelay,
  stopApiRelay,
  updateRelayApiKey,
} from "./api";
import type {
  ApiRelayStatus,
  ModelRouteSummary,
  RelayApiKeySummary,
  RelayWireType,
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

/** Published id preview: 模型.供应商 */
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

function wireLabel(wire: string | undefined): string {
  return wire === "completions" ? "Completion" : "Response";
}

export function RelayPage() {
  const [status, setStatus] = useState<ApiRelayStatus | null>(null);
  const [keys, setKeys] = useState<RelayApiKeySummary[]>([]);
  const [routes, setRoutes] = useState<ModelRouteSummary[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [messageTone, setMessageTone] = useState<"ok" | "error">("ok");
  const [revealedSecret, setRevealedSecret] = useState<string | null>(null);
  const [portDraft, setPortDraft] = useState("17862");

  // Create form
  const [newLabel, setNewLabel] = useState("");
  const [newWire, setNewWire] = useState<RelayWireType>("responses");
  const [newAllowed, setNewAllowed] = useState<string[]>([]);

  // Edit
  const [selectedKeyId, setSelectedKeyId] = useState<string | null>(null);

  const relayRoutes = useMemo(
    () => routes.filter((route) => route.relayEnabled),
    [routes],
  );

  const selectedKey = useMemo(
    () => keys.find((key) => key.id === selectedKeyId) ?? null,
    [keys, selectedKeyId],
  );

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
    setSelectedKeyId((current) => {
      if (current && nextKeys.some((key) => key.id === current)) return current;
      return nextKeys[0]?.id ?? null;
    });
  }, []);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const [nextStatus, nextKeys, nextRoutes, defaultSecret] = await Promise.all([
          getApiRelayStatus(),
          listRelayApiKeys(),
          listModelRoutes(),
          revealDefaultRelayApiKey(),
        ]);
        if (!active) return;
        setStatus(nextStatus);
        setKeys(nextKeys);
        setRoutes(nextRoutes);
        setPortDraft(String(nextStatus.port));
        setSelectedKeyId(nextKeys[0]?.id ?? null);
        if (defaultSecret) setRevealedSecret(defaultSecret);
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
  }, []);

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

  return (
    <div className="page-stack relay-page">
      {/* Status strip */}
      <section className="panel relay-status">
        <div className="panel__header">
          <div>
            <h2>反代中转站</h2>
            <p>
              纯转发：把已开「反代」的模型以 Base URL + API Key 外放。Response Key 支持
              Responses 原样与 Chat→Responses 双转换；Completion Key 仅 Chat。
              模型 id 统一为 <code>模型.供应商</code>。
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
            <dt>Local Base URL</dt>
            <dd>
              <code className="mono-copy">{status?.baseUrl ?? "—"}</code>
              {status?.baseUrl ? (
                <button
                  type="button"
                  className="button button--ghost"
                  disabled={busy}
                  onClick={() => void copyText("Base URL", status.baseUrl!)}
                >
                  复制
                </button>
              ) : null}
            </dd>
          </div>
          {status?.lanBaseUrl ? (
            <div>
              <dt>LAN Base URL</dt>
              <dd>
                <code className="mono-copy">{status.lanBaseUrl}</code>
                <button
                  type="button"
                  className="button button--ghost"
                  disabled={busy}
                  onClick={() => void copyText("LAN Base URL", status.lanBaseUrl!)}
                >
                  复制
                </button>
              </dd>
            </div>
          ) : null}
          <div>
            <dt>反代模型</dt>
            <dd>{status?.relayModelCount ?? relayRoutes.length}</dd>
          </div>
          <div>
            <dt>Client Keys</dt>
            <dd>{status?.keyCount ?? keys.length}</dd>
          </div>
        </dl>

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
                  setMessage("API 反代已停止");
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

      {revealedSecret ? (
        <div className="inline-success relay-secret" role="status">
          <div>API Key 明文（请立即复制；重新生成后失效）</div>
          <code className="mono-copy">{revealedSecret}</code>
          <button
            type="button"
            className="button button--ghost"
            onClick={() => void copyText("API Key", revealedSecret)}
          >
            复制 Key
          </button>
        </div>
      ) : null}

      {/* Two-column: keys + docs */}
      <div className="relay-split">
        <section className="panel">
          <div className="panel__header">
            <div>
              <h2>Client Keys</h2>
              <p>每把 Key 绑定一种协议类型。模型 id 固定为 模型.供应商。</p>
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
            <div className="segmented" role="group" aria-label="协议类型">
              <button
                type="button"
                className={`segmented__item${newWire === "responses" ? " segmented__item--active" : ""}`}
                disabled={busy}
                onClick={() => setNewWire("responses")}
              >
                Response
              </button>
              <button
                type="button"
                className={`segmented__item${newWire === "completions" ? " segmented__item--active" : ""}`}
                disabled={busy}
                onClick={() => setNewWire("completions")}
              >
                Completion
              </button>
            </div>
            <button
              type="button"
              className="button button--secondary"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  const created = await createRelayApiKey({
                    label: newLabel.trim() || undefined,
                    wireType: newWire,
                    nameStyle: "dotted",
                    allowedModels: newAllowed.length > 0 ? newAllowed : undefined,
                  });
                  setRevealedSecret(created.secret);
                  setNewLabel("");
                  setNewAllowed([]);
                  setSelectedKeyId(created.key.id);
                  setMessageTone("ok");
                  setMessage(`已创建 ${wireLabel(created.key.wireType)} Key「${created.key.label}」`);
                })
              }
            >
              新建 Key
            </button>
          </div>
          <p className="caption relay-create__hint">
            {newWire === "responses"
              ? "Response：/v1/responses 原样；/v1/chat/completions 转 Responses 再转回 Chat。"
              : "Completion：仅 /v1/chat/completions，仅 Completions 上游，不转换。"}
          </p>

          <div className="relay-key-list">
            {keys.length === 0 ? (
              <EmptyState
                title="还没有 Client Key"
                body="创建一把 Response 或 Completion Key，再把 Base URL 给第三方客户端。"
                action="等待创建"
              />
            ) : (
              keys.map((key) => {
                const active = key.id === selectedKeyId;
                return (
                  <button
                    key={key.id}
                    type="button"
                    className={`relay-key-row${active ? " relay-key-row--active" : ""}`}
                    onClick={() => setSelectedKeyId(key.id)}
                  >
                    <span
                      className={`switch${key.enabled ? " switch--on" : ""}`}
                      role="switch"
                      aria-checked={key.enabled}
                      onClick={(event) => {
                        event.stopPropagation();
                        void run(async () => {
                          await updateRelayApiKey({ id: key.id, enabled: !key.enabled });
                        });
                      }}
                    >
                      <span className="switch__track" aria-hidden="true" />
                    </span>
                    <span className="relay-key-row__main">
                      <strong>{key.label}</strong>
                      <small>
                        <span className="badge">{wireLabel(key.wireType)}</span>
                        <span className="badge badge--muted">加点</span>
                        <code>{key.keyPrefix}</code>
                        {key.allowedModels.length === 0
                          ? " · 全部反代模型"
                          : ` · 白名单 ${key.allowedModels.length}`}
                      </small>
                    </span>
                  </button>
                );
              })
            )}
          </div>

          {selectedKey ? (
            <div className="relay-key-detail">
              <div className="relay-key-detail__header">
                <h3>编辑 · {selectedKey.label}</h3>
                <div className="form-actions form-actions--wrap">
                  <button
                    type="button"
                    className="button button--ghost"
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        const created = await regenerateRelayApiKey(selectedKey.id);
                        setRevealedSecret(created.secret);
                        setMessageTone("ok");
                        setMessage(`已重新生成「${selectedKey.label}」`);
                      })
                    }
                  >
                    重新生成
                  </button>
                  <button
                    type="button"
                    className="button button--ghost"
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        await deleteRelayApiKey(selectedKey.id);
                        setMessageTone("ok");
                        setMessage(`已删除「${selectedKey.label}」`);
                      })
                    }
                  >
                    删除
                  </button>
                </div>
              </div>

              <label className="field">
                <span>协议类型</span>
                <div className="segmented" role="group" aria-label="协议类型">
                  <button
                    type="button"
                    className={`segmented__item${(selectedKey.wireType ?? "responses") === "responses" ? " segmented__item--active" : ""}`}
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        await updateRelayApiKey({
                          id: selectedKey.id,
                          wireType: "responses",
                        });
                      })
                    }
                  >
                    Response
                  </button>
                  <button
                    type="button"
                    className={`segmented__item${selectedKey.wireType === "completions" ? " segmented__item--active" : ""}`}
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        await updateRelayApiKey({
                          id: selectedKey.id,
                          wireType: "completions",
                        });
                      })
                    }
                  >
                    Completion
                  </button>
                </div>
              </label>

              <div className="relay-allowlist">
                <p className="caption">
                  模型白名单（空 = 全部已开反代的模型）。id 格式为 模型.供应商。
                </p>
                {relayRoutes.length === 0 ? (
                  <p className="caption">请先在「模型」页打开至少一个模型的反代开关。</p>
                ) : (
                  <div className="relay-allowlist__grid">
                    {relayRoutes.map((route) => {
                      const slug = dottedModelId(route);
                      const checked =
                        selectedKey.allowedModels.includes(route.id) ||
                        selectedKey.allowedModels.includes(slug) ||
                        selectedKey.allowedModels.includes(route.upstreamModel);
                      return (
                        <label key={route.id} className="relay-allowlist__item">
                          <input
                            type="checkbox"
                            checked={checked}
                            disabled={busy}
                            onChange={() => {
                              const current = selectedKey.allowedModels;
                              const next = checked
                                ? current.filter(
                                    (item) =>
                                      item !== route.id &&
                                      item !== slug &&
                                      item !== route.upstreamModel,
                                  )
                                : [...current, slug];
                              void run(async () => {
                                await updateRelayApiKey({
                                  id: selectedKey.id,
                                  allowedModels: next,
                                });
                              });
                            }}
                          />
                          <span>
                            {route.providerName} · {route.displayName}
                            <small>
                              <code>{slug}</code>
                            </small>
                          </span>
                        </label>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
          ) : null}
        </section>

        <section className="panel relay-docs">
          <div className="panel__header">
            <div>
              <h2>接入说明</h2>
              <p>随选中 Key 变化；复制后即可在第三方客户端使用。</p>
            </div>
          </div>
          {!selectedKey ? (
            <p className="caption">选中左侧一把 Key 查看示例。</p>
          ) : (
            <div className="relay-docs__body">
              <dl className="diagnostic-grid">
                <div>
                  <dt>协议</dt>
                  <dd>{wireLabel(selectedKey.wireType)}</dd>
                </div>
                <div>
                  <dt>命名</dt>
                  <dd>
                    <code>模型.供应商</code>
                  </dd>
                </div>
                <div>
                  <dt>Base URL</dt>
                  <dd>
                    <code className="mono-copy">{baseV1 ?? "—"}</code>
                  </dd>
                </div>
              </dl>
              {selectedKey.wireType === "completions" ? (
                <pre className="relay-code">{`# Completion Key — 仅 Chat Completions
curl -sS ${baseV1 ?? "http://127.0.0.1:17862/v1"}/chat/completions \\
  -H "Authorization: Bearer sk-spur-…" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"${exampleModel}","messages":[{"role":"user","content":"hi"}]}'
`}</pre>
              ) : (
                <pre className="relay-code">{`# Response Key — Responses 原样
curl -sS ${baseV1 ?? "http://127.0.0.1:17862/v1"}/responses \\
  -H "Authorization: Bearer sk-spur-…" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"${exampleModel}","input":"hi","stream":true}'

# Response Key — Chat 入口（转 Responses 上去，Chat 回来）
curl -sS ${baseV1 ?? "http://127.0.0.1:17862/v1"}/chat/completions \\
  -H "Authorization: Bearer sk-spur-…" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"${exampleModel}","messages":[{"role":"user","content":"hi"}]}'
`}</pre>
              )}
            </div>
          )}
        </section>
      </div>

      {/* Model inventory */}
      <section className="panel">
        <div className="panel__header">
          <div>
            <h2>可反代模型</h2>
            <p>
              在「模型」页打开反代开关后出现在此。对外 id 为 模型.供应商；与 Codex 发布 id 同一约定。
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
                <span role="cell">{route.displayName}</span>
                <span role="cell">{route.protocol}</span>
                <span role="cell">{route.providerName || route.providerId}</span>
              </div>
            ))}
          </div>
        )}
      </section>

      {message ? (
        <div
          className={messageTone === "ok" ? "inline-success" : "inline-warning"}
          role="status"
        >
          {message}
        </div>
      ) : null}
    </div>
  );
}
