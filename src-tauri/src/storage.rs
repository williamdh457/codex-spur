use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    QueryBuilder, Row, Sqlite, SqlitePool,
};

use crate::{
    credentials::CanonicalCredential,
    domain::{
        AccountPoolSummary, CredentialSummary, ModelRouteSummary, OpenAiQuotaSnapshot,
        ProviderSummary,
    },
    providers::RouteCatalogPayload,
    vault::EncryptedSecret,
};

#[derive(Debug, Clone)]
pub struct StoredRoute {
    pub id: String,
    pub provider_id: String,
    pub upstream_model: String,
    pub display_name: String,
    pub enabled: bool,
    pub catalog_json: String,
    pub protocol: String,
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct StoredCredential {
    pub id: String,
    pub provider_id: String,
    pub account_id: Option<String>,
    pub secret_envelope: EncryptedSecret,
}

#[derive(Debug, Clone)]
pub struct Lease {
    pub credential_id: String,
}

#[allow(dead_code)]
pub struct Storage {
    pub pool: SqlitePool,
    pub path: PathBuf,
}

impl Storage {
    pub async fn open(data_dir: &Path) -> Result<Self, sqlx::Error> {
        tokio::fs::create_dir_all(data_dir)
            .await
            .map_err(sqlx::Error::Io)?;
        let path = data_dir.join("codex-select.sqlite3");
        let url = format!("sqlite://{}", path.display());
        let options = SqliteConnectOptions::from_str(&url)?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        sqlx::query("PRAGMA journal_mode = WAL;")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA foreign_keys = ON;")
            .execute(&pool)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let storage = Self { pool, path };
        storage.ensure_provider_presets().await?;
        Ok(storage)
    }

    async fn ensure_provider_presets(&self) -> Result<(), sqlx::Error> {
        for (id, name, region, protocol) in [
            ("kimi", "Kimi", "中国 / Global", "Chat Completions"),
            ("deepseek", "DeepSeek", "Global", "Chat Completions"),
            ("minimax", "MiniMax", "中国 / Global", "Responses preferred"),
            ("openai", "OpenAI", "Official", "Responses"),
            ("custom", "自定义供应商", "Custom", "OpenAI-compatible"),
        ] {
            sqlx::query(
                "INSERT INTO providers (id, name, region, protocol) VALUES (?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET name = excluded.name, region = excluded.region, protocol = excluded.protocol",
            )
            .bind(id)
            .bind(name)
            .bind(region)
            .bind(protocol)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn list_providers(&self) -> Result<Vec<ProviderSummary>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, name, region, protocol, configured, selected_models, discovered_models, last_fetched_at, base_url,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id) AS credential_count,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id AND c.healthy = 1) AS healthy_credential_count,
                (SELECT COUNT(*) FROM account_pools p WHERE p.provider_id = providers.id AND p.enabled = 1) AS pool_count
             FROM providers
             ORDER BY CASE id WHEN 'openai' THEN 0 WHEN 'kimi' THEN 1 WHEN 'deepseek' THEN 2 WHEN 'minimax' THEN 3 ELSE 4 END, name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| ProviderSummary {
                id: row.get("id"),
                name: row.get("name"),
                region: row.get("region"),
                protocol: row.get("protocol"),
                configured: row.get::<i64, _>("configured") != 0,
                selected_models: row.get::<i64, _>("selected_models") as u32,
                discovered_models: row.get::<i64, _>("discovered_models") as u32,
                last_fetched_at: row.get("last_fetched_at"),
                base_url: row.get("base_url"),
                default_base_url: match row.get::<String, _>("id").as_str() {
                    "openai" => Some("https://chatgpt.com/backend-api/codex".into()),
                    "kimi" => Some("https://api.kimi.com/coding/v1".into()),
                    "deepseek" => Some("https://api.deepseek.com/v1".into()),
                    "minimax" => Some("https://api.minimaxi.com/v1".into()),
                    _ => None,
                },
                supports_official_account: row.get::<String, _>("id") == "openai",
                credential_count: row.get::<i64, _>("credential_count") as u32,
                healthy_credential_count: row.get::<i64, _>("healthy_credential_count") as u32,
                pool_count: row.get::<i64, _>("pool_count") as u32,
            })
            .collect())
    }

    pub async fn provider_base_url(
        &self,
        provider_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query("SELECT base_url FROM providers WHERE id = ?")
            .bind(provider_id)
            .fetch_optional(&self.pool)
            .await
            .map(|row| row.and_then(|row| row.get("base_url")))
    }

    pub async fn list_routes(&self, enabled_only: bool) -> Result<Vec<StoredRoute>, sqlx::Error> {
        let query = if enabled_only {
            "SELECT mr.id, mr.provider_id, mr.upstream_model, mr.display_name, mr.enabled, mr.catalog_json, p.protocol, COALESCE(p.base_url, '') AS base_url FROM model_routes mr JOIN providers p ON p.id = mr.provider_id WHERE mr.enabled = 1 ORDER BY p.name, mr.display_name"
        } else {
            "SELECT mr.id, mr.provider_id, mr.upstream_model, mr.display_name, mr.enabled, mr.catalog_json, p.protocol, COALESCE(p.base_url, '') AS base_url FROM model_routes mr JOIN providers p ON p.id = mr.provider_id ORDER BY p.name, mr.display_name"
        };
        let rows = sqlx::query(query).fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredRoute {
                id: row.get("id"),
                provider_id: row.get("provider_id"),
                upstream_model: row.get("upstream_model"),
                display_name: row.get("display_name"),
                enabled: row.get::<i64, _>("enabled") != 0,
                catalog_json: row.get("catalog_json"),
                protocol: row.get("protocol"),
                base_url: row.get("base_url"),
            })
            .collect())
    }

    pub async fn replace_discovered_models(
        &self,
        provider_id: &str,
        base_url: &str,
        models: &[(String, String, String)],
    ) -> Result<Vec<StoredRoute>, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE providers SET base_url = ?, configured = 1 WHERE id = ?")
            .bind(base_url)
            .bind(provider_id)
            .execute(&mut *transaction)
            .await?;
        if !models.is_empty() {
            let mut query =
                QueryBuilder::<Sqlite>::new("DELETE FROM model_routes WHERE provider_id = ");
            query
                .push_bind(provider_id)
                .push(" AND upstream_model NOT IN (");
            let mut separated = query.separated(", ");
            for (id, _, _) in models {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            query.build().execute(&mut *transaction).await?;
        }
        for (id, display_name, catalog_json) in models {
            let route_id = route_id(provider_id, id);
            sqlx::query(
                "INSERT INTO model_routes (id, provider_id, upstream_model, display_name, enabled, catalog_json) VALUES (?, ?, ?, ?, COALESCE((SELECT enabled FROM model_routes WHERE id = ?), 0), ?) ON CONFLICT(id) DO UPDATE SET display_name = excluded.display_name, catalog_json = excluded.catalog_json, updated_at = CURRENT_TIMESTAMP",
            )
            .bind(&route_id)
            .bind(provider_id)
            .bind(id)
            .bind(display_name)
            .bind(&route_id)
            .bind(catalog_json)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query("UPDATE providers SET discovered_models = ?, selected_models = (SELECT COUNT(*) FROM model_routes WHERE provider_id = providers.id AND enabled = 1), last_fetched_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(models.len() as i64)
            .bind(provider_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.list_routes(false).await
    }

    pub async fn set_route_enabled(
        &self,
        route_id: &str,
        enabled: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE model_routes SET enabled = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(enabled as i64)
        .bind(route_id)
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE providers SET selected_models = (SELECT COUNT(*) FROM model_routes WHERE provider_id = providers.id AND enabled = 1) WHERE id = (SELECT provider_id FROM model_routes WHERE id = ?)")
            .bind(route_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_credential(
        &self,
        credential: &CanonicalCredential,
        id: &str,
        envelope_json: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "INSERT INTO credentials (id, provider_id, kind, state, label, email, account_id, expires_at, fingerprint, refreshable, secret_envelope_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(fingerprint) DO NOTHING",
        )
        .bind(id)
        .bind(&credential.provider_id)
        .bind(serde_json::to_string(&credential.kind).unwrap_or_else(|_| "\"api_key\"".into()).trim_matches('"'))
        .bind(serde_json::to_string(&credential.state).unwrap_or_else(|_| "\"unknown\"".into()).trim_matches('"'))
        .bind(&credential.label)
        .bind(&credential.email)
        .bind(&credential.account_id)
        .bind(credential.expires_at)
        .bind(&credential.fingerprint)
        .bind((credential.refreshable && credential.secret.has_refresh_token()) as i64)
        .bind(envelope_json)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_credentials(
        &self,
        provider_id: Option<&str>,
    ) -> Result<Vec<CredentialSummary>, sqlx::Error> {
        let (query, bind_provider) = match provider_id {
            Some(_) => ("SELECT id, provider_id, kind, state, label, email, account_id, expires_at, fingerprint, refreshable, healthy, last_error FROM credentials WHERE provider_id = ? ORDER BY created_at DESC", true),
            None => ("SELECT id, provider_id, kind, state, label, email, account_id, expires_at, fingerprint, refreshable, healthy, last_error FROM credentials ORDER BY created_at DESC", false),
        };
        let mut request = sqlx::query(query);
        if bind_provider {
            request = request.bind(provider_id.unwrap_or_default());
        }
        let rows = request.fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|row| CredentialSummary {
                id: row.get("id"),
                provider_id: row.get("provider_id"),
                kind: row.get("kind"),
                state: row.get("state"),
                label: row.get("label"),
                masked_email: row
                    .get::<Option<String>, _>("email")
                    .as_deref()
                    .map(mask_identity),
                masked_account_id: row
                    .get::<Option<String>, _>("account_id")
                    .as_deref()
                    .map(mask_identity),
                expires_at: row.get("expires_at"),
                fingerprint_prefix: row
                    .get::<String, _>("fingerprint")
                    .chars()
                    .take(12)
                    .collect(),
                refreshable: row.get::<i64, _>("refreshable") != 0,
                healthy: row.get::<i64, _>("healthy") != 0,
                last_error: row.get("last_error"),
            })
            .collect())
    }

    pub async fn first_healthy_credential(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredCredential>, sqlx::Error> {
        let row = sqlx::query("SELECT id FROM credentials WHERE provider_id = ? AND healthy = 1 ORDER BY updated_at DESC, created_at LIMIT 1")
            .bind(provider_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => {
                self.get_credential(row.get::<String, _>("id").as_str())
                    .await
            }
            None => Ok(None),
        }
    }

    pub async fn get_credential(&self, id: &str) -> Result<Option<StoredCredential>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, provider_id, account_id, secret_envelope_json FROM credentials WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let envelope_json: String = row.get("secret_envelope_json");
            let secret_envelope = serde_json::from_str(&envelope_json)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
            Ok(StoredCredential {
                id: row.get("id"),
                provider_id: row.get("provider_id"),
                account_id: row.get("account_id"),
                secret_envelope,
            })
        })
        .transpose()
    }

    pub async fn ensure_default_pool(&self, provider_id: &str) -> Result<String, sqlx::Error> {
        let id = format!("default-{provider_id}");
        sqlx::query("INSERT INTO account_pools (id, name, provider_id, strategy, sticky_ttl_secs) VALUES (?, ?, ?, 'least_active', 3600) ON CONFLICT(id) DO NOTHING")
            .bind(&id)
            .bind(format!("{} 默认账号池", provider_id))
            .bind(provider_id)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn default_pool_id(&self, provider_id: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query("SELECT id FROM account_pools WHERE provider_id = ? AND enabled = 1 ORDER BY CASE WHEN id = 'default-' || ? THEN 0 ELSE 1 END, created_at LIMIT 1")
            .bind(provider_id)
            .bind(provider_id)
            .fetch_optional(&self.pool)
            .await
            .map(|row| row.map(|row| row.get("id")))
    }

    pub async fn create_pool(&self, provider_id: &str, name: &str) -> Result<String, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO account_pools (id, name, provider_id) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(name)
            .bind(provider_id)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn list_pools(&self) -> Result<Vec<AccountPoolSummary>, sqlx::Error> {
        let rows = sqlx::query("SELECT p.id, p.name, p.provider_id, p.strategy, p.sticky_ttl_secs, p.enabled, COUNT(pm.credential_id) AS account_count, SUM(CASE WHEN c.healthy = 1 THEN 1 ELSE 0 END) AS healthy_count FROM account_pools p LEFT JOIN pool_members pm ON pm.pool_id = p.id AND pm.enabled = 1 LEFT JOIN credentials c ON c.id = pm.credential_id GROUP BY p.id ORDER BY p.created_at")
            .fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|row| AccountPoolSummary {
                id: row.get("id"),
                name: row.get("name"),
                provider_id: row.get("provider_id"),
                strategy: row.get("strategy"),
                sticky_ttl_secs: row.get("sticky_ttl_secs"),
                enabled: row.get::<i64, _>("enabled") != 0,
                account_count: row.get::<i64, _>("account_count") as u32,
                healthy_count: row.get::<Option<i64>, _>("healthy_count").unwrap_or(0) as u32,
            })
            .collect())
    }

    pub async fn add_pool_member(
        &self,
        pool_id: &str,
        credential_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO pool_members (pool_id, credential_id) VALUES (?, ?) ON CONFLICT(pool_id, credential_id) DO UPDATE SET enabled = 1")
            .bind(pool_id).bind(credential_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn remove_pool_member(
        &self,
        pool_id: &str,
        credential_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE pool_members SET enabled = 0 WHERE pool_id = ? AND credential_id = ?")
            .bind(pool_id)
            .bind(credential_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_pool_member_ids(&self, pool_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query("SELECT credential_id FROM pool_members WHERE pool_id = ? AND enabled = 1 ORDER BY created_at")
            .bind(pool_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.get("credential_id"))
            .collect())
    }

    pub async fn acquire_lease(
        &self,
        pool_id: &str,
        affinity_key: Option<&str>,
        ttl_secs: i64,
    ) -> Result<Option<Lease>, sqlx::Error> {
        sqlx::query("UPDATE account_leases SET released_at = CURRENT_TIMESTAMP WHERE released_at IS NULL AND expires_at IS NOT NULL AND expires_at <= CURRENT_TIMESTAMP")
            .execute(&self.pool)
            .await?;
        if let Some(key) = affinity_key {
            if let Some(row) = sqlx::query("SELECT l.id, l.pool_id, l.credential_id, l.affinity_key FROM account_leases l JOIN credentials c ON c.id = l.credential_id WHERE l.pool_id = ? AND l.affinity_key = ? AND l.released_at IS NULL AND (l.expires_at IS NULL OR l.expires_at > CURRENT_TIMESTAMP) AND c.healthy = 1 ORDER BY l.acquired_at DESC LIMIT 1")
                .bind(pool_id).bind(key).fetch_optional(&self.pool).await? {
                return Ok(Some(Lease { credential_id: row.get("credential_id") }));
            }
        }
        let row = sqlx::query("SELECT pm.credential_id FROM pool_members pm JOIN credentials c ON c.id = pm.credential_id WHERE pm.pool_id = ? AND pm.enabled = 1 AND c.healthy = 1 ORDER BY (SELECT COUNT(*) FROM account_leases l WHERE l.credential_id = pm.credential_id AND l.released_at IS NULL AND (l.expires_at IS NULL OR l.expires_at > CURRENT_TIMESTAMP)), pm.priority DESC, pm.created_at LIMIT 1")
            .bind(pool_id).fetch_optional(&self.pool).await?;
        let Some(row) = row else { return Ok(None) };
        let credential_id: String = row.get("credential_id");
        let id = uuid::Uuid::new_v4().to_string();
        let affinity = affinity_key.map(ToOwned::to_owned);
        sqlx::query("INSERT INTO account_leases (id, pool_id, credential_id, affinity_key, expires_at) VALUES (?, ?, ?, ?, datetime(CURRENT_TIMESTAMP, '+' || ? || ' seconds'))")
            .bind(&id).bind(pool_id).bind(&credential_id).bind(&affinity).bind(ttl_secs).execute(&self.pool).await?;
        Ok(Some(Lease { credential_id }))
    }

    pub async fn mark_credential_health(
        &self,
        id: &str,
        healthy: bool,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE credentials SET healthy = ?, last_error = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(healthy as i64).bind(error).bind(id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn save_quota_snapshot(
        &self,
        snapshot: &OpenAiQuotaSnapshot,
    ) -> Result<(), sqlx::Error> {
        for (window_name, window) in [
            ("5h", snapshot.five_hour.as_ref()),
            ("7d", snapshot.seven_day.as_ref()),
        ] {
            if let Some(window) = window {
                sqlx::query("INSERT INTO usage_snapshots (id, credential_id, window, used_units, limit_units, reset_at, fetched_at) VALUES (?, ?, ?, ?, 10000, ?, datetime(?, 'unixepoch')) ON CONFLICT(credential_id, window) DO UPDATE SET used_units = excluded.used_units, limit_units = excluded.limit_units, reset_at = excluded.reset_at, fetched_at = excluded.fetched_at")
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(&snapshot.credential_id)
                    .bind(window_name)
                    .bind((window.used_percent * 100.0).round() as i64)
                    .bind(window.reset_at.map(|value| value.to_string()))
                    .bind(snapshot.fetched_at)
                    .execute(&self.pool)
                    .await?;
            }
        }
        sqlx::query("INSERT INTO app_settings (key, value_json) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = CURRENT_TIMESTAMP")
            .bind(format!("quota:{}", snapshot.credential_id))
            .bind(serde_json::to_string(snapshot).map_err(|error| sqlx::Error::Encode(Box::new(error)))?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn cached_quota_snapshot(
        &self,
        credential_id: &str,
    ) -> Result<Option<OpenAiQuotaSnapshot>, sqlx::Error> {
        let row = sqlx::query("SELECT value_json FROM app_settings WHERE key = ?")
            .bind(format!("quota:{credential_id}"))
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            let json: String = row.get("value_json");
            serde_json::from_str(&json).map_err(|error| sqlx::Error::Decode(Box::new(error)))
        })
        .transpose()
    }

    pub async fn reserve_reset_credit_action(
        &self,
        credential_id: &str,
        idempotency_key: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("INSERT INTO reset_credit_actions (idempotency_key, credential_id, status) VALUES (?, ?, 'pending') ON CONFLICT(idempotency_key) DO NOTHING")
            .bind(idempotency_key)
            .bind(credential_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn finish_reset_credit_action(
        &self,
        idempotency_key: &str,
        status: &str,
        result_json: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE reset_credit_actions SET status = ?, result_json = ?, completed_at = CURRENT_TIMESTAMP WHERE idempotency_key = ?")
            .bind(status)
            .bind(result_json)
            .bind(idempotency_key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn route_summaries(&self) -> Result<Vec<ModelRouteSummary>, sqlx::Error> {
        let routes = self.list_routes(false).await?;
        routes
            .into_iter()
            .map(|route| {
                let profile = serde_json::from_str::<RouteCatalogPayload>(&route.catalog_json)
                    .map(|payload| payload.reasoning_profile)
                    .unwrap_or_else(|_| crate::domain::ReasoningProfile {
                        title: route.display_name.clone(),
                        mappings: Vec::new(),
                    });
                Ok(ModelRouteSummary {
                    id: route.id,
                    provider_id: route.provider_id,
                    upstream_model: route.upstream_model,
                    display_name: route.display_name,
                    enabled: route.enabled,
                    protocol: route.protocol,
                    base_url: route.base_url,
                    reasoning_profile: profile,
                })
            })
            .collect()
    }
}

pub fn route_id(provider_id: &str, upstream_model: &str) -> String {
    format!("{provider_id}/{}", slugify(upstream_model))
}

fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn mask_identity(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 4 {
        return "••••".into();
    }
    let prefix: String = chars.iter().take(2).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}••••{suffix}")
}
