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
        AccountPoolSummary, CredentialSummary, DeleteCredentialResult, ModelRouteSummary,
        OpenAiQuotaSnapshot, PoolMemberDetail, ProviderRouting, ProviderSummary, ProxyRequestEvent,
        UsageBreakdown, UsageDashboardSnapshot, UsageRange, UsageTrendPoint,
    },
    providers::RouteCatalogPayload,
    scheduler::{
        select_account, sticky_concurrency_full, BindingKind, CandidateAccount,
        PoolSchedulerConfig, RoutingMode, ScheduleState, SelectOutcome, SelectRequest,
        SelectionLayer,
    },
    vault::EncryptedSecret,
};

#[derive(Debug, Clone)]
pub struct StoredRoute {
    pub id: String,
    pub provider_id: String,
    /// User-facing provider instance name from `providers.name`.
    pub provider_name: String,
    pub kind: String,
    pub upstream_model: String,
    pub display_name: String,
    pub enabled: bool,
    pub catalog_json: String,
    pub protocol: String,
    pub base_url: String,
    /// Provider entry channel (`official` / `api` / `json`); used for xAI base resolution.
    pub entry_category: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredCredential {
    pub id: String,
    pub provider_id: String,
    pub account_id: Option<String>,
    /// Unix seconds; used for OAuth access-token refresh lead time.
    pub expires_at: Option<i64>,
    pub secret_envelope: EncryptedSecret,
}

#[derive(Debug, Clone)]
pub struct Lease {
    pub id: String,
    pub credential_id: String,
    pub layer: SelectionLayer,
    pub sticky_escaped: bool,
}

pub struct UsageDelta {
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_observations: i64,
    pub cache_hits: i64,
    pub failed_requests: i64,
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
        // foreign_keys(true) applies on every pool connection (PRAGMA is per-connection).
        let options = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        sqlx::query("PRAGMA journal_mode = WAL;")
            .execute(&pool)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let storage = Self { pool, path };
        storage.normalize_provider_kinds().await?;
        storage.normalize_credential_kinds().await?;
        storage.normalize_entry_categories().await?;
        let _ = storage.repair_agent_identity_refreshable_flags().await;
        storage.cleanup_empty_seed_providers().await?;
        Ok(storage)
    }

    /// Backfill kind for legacy rows; kinds are templates in code, not seeded list rows.
    async fn normalize_provider_kinds(&self) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE providers SET kind = id WHERE kind = '' OR kind IS NULL")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Fix serde snake_case mangling: `OAuth` → `o_auth`, `ChatGptWebSession` → `chat_gpt_web_session`.
    /// Counts for entry-category badges filter on `oauth` / `chatgpt_web_session`.
    async fn normalize_credential_kinds(&self) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE credentials SET kind = 'oauth' WHERE kind = 'o_auth'")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "UPDATE credentials SET kind = 'chatgpt_web_session' WHERE kind = 'chat_gpt_web_session'",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Repair entry_category stamps after credential-kind / import path bugs.
    /// Multi-account oauth rows stamped `official` are JSON file imports (browser login is single-account).
    async fn normalize_entry_categories(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE providers SET entry_category = 'json'
             WHERE entry_category IN ('pool', 'config')",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE providers SET entry_category = 'json'
             WHERE entry_category = 'official'
               AND (
                 SELECT COUNT(*) FROM credentials c
                 WHERE c.provider_id = providers.id
                   AND c.kind IN ('oauth', 'o_auth', 'chatgpt_web_session', 'chat_gpt_web_session', 'agent_identity')
               ) >= 2",
        )
        .execute(&self.pool)
        .await?;
        // Browser official login with a single oauth row and null stamp → official.
        sqlx::query(
            "UPDATE providers SET entry_category = 'official'
             WHERE (entry_category IS NULL OR entry_category = '')
               AND kind IN ('openai', 'xai')
               AND (
                 SELECT COUNT(*) FROM credentials c
                 WHERE c.provider_id = providers.id
                   AND c.kind IN ('oauth', 'o_auth', 'chatgpt_web_session', 'chat_gpt_web_session', 'agent_identity')
               ) = 1
               AND (
                 SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id
               ) = 1
               AND (
                 SELECT COUNT(*) FROM credentials c
                 WHERE c.provider_id = providers.id AND c.kind = 'api_key'
               ) = 0",
        )
        .execute(&self.pool)
        .await?;
        // Grok OAuth subscription must hit the CLI chat proxy, not api.x.ai.
        // Rewrite official/legacy official-host rows so live routes pick up the fix.
        self.migrate_xai_subscription_bases().await?;
        Ok(())
    }

    /// Move Grok official/OAuth instances from `api.x.ai` onto the CLI subscription base.
    /// Custom hosts are left alone. Returns how many providers were updated.
    pub async fn migrate_xai_subscription_bases(&self) -> Result<u64, sqlx::Error> {
        let cli = crate::providers::XAI_CLI_SUBSCRIPTION_BASE;
        let result = sqlx::query(
            "UPDATE providers SET base_url = ?
             WHERE kind = 'xai'
               AND (
                 entry_category = 'official'
                 OR entry_category = 'subscription'
                 OR entry_category = 'oauth'
                 OR (
                   (entry_category IS NULL OR entry_category = '')
                   AND EXISTS (
                     SELECT 1 FROM credentials c
                     WHERE c.provider_id = providers.id
                       AND c.kind IN ('oauth', 'o_auth')
                   )
                   AND NOT EXISTS (
                     SELECT 1 FROM credentials c
                     WHERE c.provider_id = providers.id AND c.kind = 'api_key'
                   )
                 )
               )
               AND (
                 base_url IS NULL
                 OR base_url = ''
                 OR base_url LIKE '%api.x.ai%'
               )",
        )
        .bind(cli)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Remove legacy empty seed rows (id == kind, never configured, no credentials/routes).
    async fn cleanup_empty_seed_providers(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM providers
             WHERE id = kind
               AND configured = 0
               AND id IN ('openai', 'kimi', 'deepseek', 'minimax', 'custom')
               AND NOT EXISTS (SELECT 1 FROM credentials c WHERE c.provider_id = providers.id)
               AND NOT EXISTS (SELECT 1 FROM model_routes m WHERE m.provider_id = providers.id)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    fn map_provider_row(row: &sqlx::sqlite::SqliteRow) -> ProviderSummary {
        let kind: String = row.get("kind");
        let kind = if kind.is_empty() {
            row.get::<String, _>("id")
        } else {
            kind
        };
        let base_url: Option<String> = row.get("base_url");
        let credential_count = row.get::<i64, _>("credential_count") as u32;
        let stored_category: Option<String> = row
            .try_get::<Option<String>, _>("entry_category")
            .unwrap_or(None)
            .filter(|value| !value.trim().is_empty())
            .map(|value| match value.as_str() {
                // Legacy stamps: account-pool import / provider-config import → JSON.
                "pool" | "config" => "json".to_string(),
                other => other.to_string(),
            });
        let api_key_count = row.try_get::<i64, _>("api_key_count").unwrap_or(0) as u32;
        let oauth_count = row.try_get::<i64, _>("oauth_count").unwrap_or(0) as u32;
        // Browser official login only ever writes one account. Multi-account rows
        // stamped "official" are almost always a prior JSON import mis-classified
        // when discover rewrote the channel after an empty base_url fetch.
        let stored_category = match stored_category.as_deref() {
            Some("official")
                if oauth_count >= 2
                    || (oauth_count >= 1 && credential_count >= 2 && api_key_count == 0) =>
            {
                Some("json".to_string())
            }
            _ => stored_category,
        };
        let entry_category = stored_category.or_else(|| {
            infer_entry_category(
                &kind,
                base_url.as_deref(),
                credential_count,
                api_key_count,
                oauth_count,
            )
        });
        ProviderSummary {
            id: row.get("id"),
            kind: kind.clone(),
            name: row.get("name"),
            region: row.get("region"),
            protocol: row.get("protocol"),
            configured: row.get::<i64, _>("configured") != 0,
            selected_models: row.get::<i64, _>("selected_models") as u32,
            discovered_models: row.get::<i64, _>("discovered_models") as u32,
            last_fetched_at: row.get("last_fetched_at"),
            base_url,
            default_base_url: crate::providers::default_base_url_for_kind(&kind),
            supports_official_account: kind == "openai" || kind == "xai",
            credential_count,
            healthy_credential_count: row.get::<i64, _>("healthy_credential_count") as u32,
            pool_count: row.get::<i64, _>("pool_count") as u32,
            active_pool_id: row.get("active_pool_id"),
            routing_mode: row
                .try_get::<String, _>("routing_mode")
                .unwrap_or_else(|_| "pool".into()),
            fixed_credential_id: row
                .try_get::<Option<String>, _>("fixed_credential_id")
                .unwrap_or(None),
            entry_category,
        }
    }

    pub async fn list_providers(&self) -> Result<Vec<ProviderSummary>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, kind, name, region, protocol, configured, selected_models, discovered_models, last_fetched_at, base_url, active_pool_id, routing_mode, fixed_credential_id, entry_category,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id) AS credential_count,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id AND c.healthy = 1) AS healthy_credential_count,
                (SELECT COUNT(*) FROM account_pools p WHERE p.provider_id = providers.id AND p.enabled = 1) AS pool_count,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id AND c.kind = 'api_key') AS api_key_count,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id AND c.kind IN ('oauth', 'o_auth', 'chatgpt_web_session', 'chat_gpt_web_session', 'agent_identity')) AS oauth_count
             FROM providers
             ORDER BY CASE kind WHEN 'openai' THEN 0 WHEN 'xai' THEN 1 WHEN 'kimi' THEN 2 WHEN 'deepseek' THEN 3 WHEN 'minimax' THEN 4 WHEN 'opencode-go' THEN 5 ELSE 6 END, name, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(Self::map_provider_row).collect())
    }

    pub async fn get_provider(
        &self,
        provider_id: &str,
    ) -> Result<Option<ProviderSummary>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, kind, name, region, protocol, configured, selected_models, discovered_models, last_fetched_at, base_url, active_pool_id, routing_mode, fixed_credential_id, entry_category,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id) AS credential_count,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id AND c.healthy = 1) AS healthy_credential_count,
                (SELECT COUNT(*) FROM account_pools p WHERE p.provider_id = providers.id AND p.enabled = 1) AS pool_count,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id AND c.kind = 'api_key') AS api_key_count,
                (SELECT COUNT(*) FROM credentials c WHERE c.provider_id = providers.id AND c.kind IN ('oauth', 'o_auth', 'chatgpt_web_session', 'chat_gpt_web_session', 'agent_identity')) AS oauth_count
             FROM providers WHERE id = ?",
        )
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(Self::map_provider_row))
    }

    pub async fn set_provider_entry_category(
        &self,
        provider_id: &str,
        category: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE providers SET entry_category = ? WHERE id = ?")
            .bind(category)
            .bind(provider_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_provider_instance(
        &self,
        kind: &str,
        name: Option<&str>,
    ) -> Result<String, sqlx::Error> {
        let (default_name, region, protocol, _) =
            crate::providers::kind_meta(kind).ok_or_else(|| {
                sqlx::Error::Configuration(format!("unknown provider kind: {kind}").into())
            })?;
        let id = uuid::Uuid::new_v4().to_string();
        let display = if let Some(custom) = name.map(str::trim).filter(|value| !value.is_empty()) {
            custom.to_owned()
        } else {
            let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM providers WHERE kind = ?")
                .bind(kind)
                .fetch_one(&self.pool)
                .await?;
            if existing <= 0 {
                default_name.to_string()
            } else {
                format!("{} {}", default_name, existing + 1)
            }
        };
        sqlx::query(
            "INSERT INTO providers (id, kind, name, region, protocol, configured) VALUES (?, ?, ?, ?, ?, 0)",
        )
        .bind(&id)
        .bind(kind)
        .bind(&display)
        .bind(region)
        .bind(protocol)
        .execute(&self.pool)
        .await?;
        let pool_id = self.ensure_default_pool(&id).await?;
        sqlx::query("UPDATE providers SET active_pool_id = ? WHERE id = ?")
            .bind(&pool_id)
            .bind(&id)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn delete_provider_instance(&self, provider_id: &str) -> Result<(), sqlx::Error> {
        let result = sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(provider_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::Protocol("供应商不存在".into()));
        }
        Ok(())
    }

    pub async fn rename_provider_instance(
        &self,
        provider_id: &str,
        name: &str,
    ) -> Result<(), sqlx::Error> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        sqlx::query("UPDATE providers SET name = ? WHERE id = ?")
            .bind(trimmed)
            .bind(provider_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_active_pool(
        &self,
        provider_id: &str,
        pool_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE providers SET active_pool_id = ? WHERE id = ? AND EXISTS (SELECT 1 FROM account_pools WHERE id = ? AND provider_id = ?)",
        )
        .bind(pool_id)
        .bind(provider_id)
        .bind(pool_id)
        .bind(provider_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn active_pool_id(&self, provider_id: &str) -> Result<Option<String>, sqlx::Error> {
        if let Some(active) = sqlx::query("SELECT active_pool_id FROM providers WHERE id = ?")
            .bind(provider_id)
            .fetch_optional(&self.pool)
            .await?
            .and_then(|row| row.get::<Option<String>, _>("active_pool_id"))
        {
            let exists = sqlx::query(
                "SELECT 1 AS ok FROM account_pools WHERE id = ? AND provider_id = ? AND enabled = 1",
            )
            .bind(&active)
            .bind(provider_id)
            .fetch_optional(&self.pool)
            .await?
            .is_some();
            if exists {
                return Ok(Some(active));
            }
        }
        self.default_pool_id(provider_id).await
    }

    #[allow(dead_code)]
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
            "SELECT mr.id, mr.provider_id, p.name AS provider_name, COALESCE(NULLIF(p.kind, ''), p.id) AS kind, mr.upstream_model, mr.display_name, mr.enabled, mr.catalog_json, p.protocol, COALESCE(p.base_url, '') AS base_url, p.entry_category FROM model_routes mr JOIN providers p ON p.id = mr.provider_id WHERE mr.enabled = 1 ORDER BY p.name, mr.display_name"
        } else {
            "SELECT mr.id, mr.provider_id, p.name AS provider_name, COALESCE(NULLIF(p.kind, ''), p.id) AS kind, mr.upstream_model, mr.display_name, mr.enabled, mr.catalog_json, p.protocol, COALESCE(p.base_url, '') AS base_url, p.entry_category FROM model_routes mr JOIN providers p ON p.id = mr.provider_id ORDER BY p.name, mr.display_name"
        };
        let rows = sqlx::query(query).fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredRoute {
                id: row.get("id"),
                provider_id: row.get("provider_id"),
                provider_name: row.get("provider_name"),
                kind: row.get("kind"),
                upstream_model: row.get("upstream_model"),
                display_name: row.get("display_name"),
                enabled: row.get::<i64, _>("enabled") != 0,
                catalog_json: row.get("catalog_json"),
                protocol: row.get("protocol"),
                base_url: row.get("base_url"),
                entry_category: row.try_get("entry_category").ok().flatten(),
            })
            .collect())
    }

    pub async fn set_provider_base_url(
        &self,
        provider_id: &str,
        base_url: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE providers SET base_url = ?, configured = CASE WHEN ? IS NOT NULL AND TRIM(?) != '' THEN 1 ELSE configured END WHERE id = ?")
            .bind(base_url)
            .bind(base_url)
            .bind(base_url)
            .bind(provider_id)
            .execute(&self.pool)
            .await?;
        Ok(())
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

    /// Persist a healed snake_case `catalog_json` for one route (startup scrub / re-fetch).
    pub async fn update_route_catalog_json(
        &self,
        route_id: &str,
        catalog_json: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE model_routes SET catalog_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(catalog_json)
        .bind(route_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Rewrite every stored route's catalog_json into the Codex-safe snake_case shape.
    /// Returns how many rows were updated. Failures on individual rows are skipped with a warn.
    pub async fn heal_all_route_catalogs(&self) -> Result<u32, sqlx::Error> {
        let routes = self.list_routes(false).await?;
        let mut updated = 0u32;
        for route in routes {
            match crate::catalog::heal_stored_catalog_json(&route) {
                Ok(healed) if healed != route.catalog_json => {
                    self.update_route_catalog_json(&route.id, &healed).await?;
                    updated += 1;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        route_id = %route.id,
                        error = %error,
                        "跳过无法 heal 的 model_routes.catalog_json"
                    );
                }
            }
        }
        Ok(updated)
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

    pub async fn record_apply_revision(
        &self,
        id: &str,
        catalog_path: &str,
        config_path: &str,
        config_hash_before: Option<&str>,
        config_hash_after: &str,
        state: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO apply_revisions (id, catalog_path, config_path, config_hash_before, config_hash_after, state) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(catalog_path)
        .bind(config_path)
        .bind(config_hash_before)
        .bind(config_hash_after)
        .bind(state)
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
        .bind(credential.kind.as_db_str())
        .bind(serde_json::to_string(&credential.state).unwrap_or_else(|_| "\"unknown\"".into()).trim_matches('"'))
        .bind(&credential.label)
        .bind(&credential.email)
        .bind(&credential.account_id)
        .bind(credential.expires_at)
        .bind(&credential.fingerprint)
        .bind(
            (credential.kind == crate::credentials::CredentialKind::AgentIdentity
                || (credential.refreshable && credential.secret.has_refresh_token()))
                as i64,
        )
        .bind(envelope_json)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Hard-delete a credential and clear dependent routing/quota state.
    ///
    /// FK CASCADE covers pool_members, leases, sticky bindings, usage_snapshots,
    /// and reset_credit_actions. This also clears fixed routing and cached quota.
    pub async fn delete_credential(
        &self,
        credential_id: &str,
    ) -> Result<DeleteCredentialResult, sqlx::Error> {
        let row = sqlx::query("SELECT provider_id FROM credentials WHERE id = ?")
            .bind(credential_id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Err(sqlx::Error::Protocol("账号不存在".into()));
        };
        let provider_id: String = row.get("provider_id");

        // Clear fixed routing before delete so the provider never points at a missing credential.
        sqlx::query(
            "UPDATE providers
             SET routing_mode = CASE WHEN fixed_credential_id = ? THEN 'pool' ELSE routing_mode END,
                 fixed_credential_id = CASE WHEN fixed_credential_id = ? THEN NULL ELSE fixed_credential_id END
             WHERE id = ?",
        )
        .bind(credential_id)
        .bind(credential_id)
        .bind(&provider_id)
        .execute(&self.pool)
        .await?;

        sqlx::query("DELETE FROM app_settings WHERE key = ?")
            .bind(format!("quota:{credential_id}"))
            .execute(&self.pool)
            .await?;

        let deleted = sqlx::query("DELETE FROM credentials WHERE id = ?")
            .bind(credential_id)
            .execute(&self.pool)
            .await?;
        if deleted.rows_affected() == 0 {
            return Err(sqlx::Error::Protocol("账号不存在".into()));
        }

        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM credentials WHERE provider_id = ?")
                .bind(&provider_id)
                .fetch_one(&self.pool)
                .await?;

        Ok(DeleteCredentialResult {
            provider_id,
            remaining_accounts: remaining as u32,
        })
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
        Ok(rows.into_iter().map(Self::map_credential_summary).collect())
    }

    /// Credentials whose parent provider instance has the given `providers.kind` (e.g. `"openai"`).
    pub async fn list_credentials_for_kind(
        &self,
        kind: &str,
    ) -> Result<Vec<CredentialSummary>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT c.id, c.provider_id, c.kind, c.state, c.label, c.email, c.account_id, c.expires_at, c.fingerprint, c.refreshable, c.healthy, c.last_error
             FROM credentials c
             INNER JOIN providers p ON p.id = c.provider_id
             WHERE p.kind = ?
             ORDER BY c.created_at DESC",
        )
        .bind(kind)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Self::map_credential_summary).collect())
    }

    fn map_credential_summary(row: sqlx::sqlite::SqliteRow) -> CredentialSummary {
        CredentialSummary {
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
        }
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
            "SELECT id, provider_id, account_id, expires_at, secret_envelope_json FROM credentials WHERE id = ?",
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
                expires_at: row.try_get("expires_at").unwrap_or(None),
                secret_envelope,
            })
        })
        .transpose()
    }

    /// Prefer a healthy oauth row; otherwise the newest refreshable oauth row
    /// (including `auth_invalid`) so callers can attempt token refresh recovery.
    pub async fn first_refreshable_oauth_credential(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredCredential>, sqlx::Error> {
        if let Some(healthy) = self.first_healthy_credential(provider_id).await? {
            return Ok(Some(healthy));
        }
        let row = sqlx::query(
            "SELECT id FROM credentials
             WHERE provider_id = ?
               AND refreshable = 1
               AND kind IN ('oauth', 'o_auth')
             ORDER BY updated_at DESC, created_at DESC
             LIMIT 1",
        )
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

    /// Replace encrypted secret payload and optional expiry after OAuth refresh.
    pub async fn update_credential_secret(
        &self,
        id: &str,
        envelope_json: &str,
        expires_at: Option<i64>,
        account_id: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE credentials SET secret_envelope_json = ?, expires_at = COALESCE(?, expires_at),
             account_id = COALESCE(?, account_id), updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(envelope_json)
        .bind(expires_at)
        .bind(account_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Find a credential in this provider instance by ChatGPT account_id or email.
    /// Prefers exact account_id match, then case-insensitive email.
    pub async fn find_credential_for_merge(
        &self,
        provider_id: &str,
        account_id: Option<&str>,
        email: Option<&str>,
    ) -> Result<Option<StoredCredential>, sqlx::Error> {
        if let Some(account_id) = account_id.map(str::trim).filter(|s| !s.is_empty()) {
            let row = sqlx::query(
                "SELECT id FROM credentials WHERE provider_id = ? AND account_id = ?
                 ORDER BY updated_at DESC, created_at DESC LIMIT 1",
            )
            .bind(provider_id)
            .bind(account_id)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(row) = row {
                return self
                    .get_credential(row.get::<String, _>("id").as_str())
                    .await;
            }
        }
        if let Some(email) = email.map(str::trim).filter(|s| !s.is_empty()) {
            let row = sqlx::query(
                "SELECT id FROM credentials WHERE provider_id = ? AND lower(email) = lower(?)
                 ORDER BY updated_at DESC, created_at DESC LIMIT 1",
            )
            .bind(provider_id)
            .bind(email)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(row) = row {
                return self
                    .get_credential(row.get::<String, _>("id").as_str())
                    .await;
            }
        }
        Ok(None)
    }

    pub async fn set_credential_refreshable(
        &self,
        id: &str,
        refreshable: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE credentials SET refreshable = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(refreshable as i64)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// After a successful secret re-import / agent upgrade: clear sticky auth_invalid
    /// so the scheduler can select the account again.
    pub async fn heal_credential_after_import(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE credentials SET schedule_state = 'ready', healthy = 1, last_error = NULL,
             cooldown_until = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Persist kind/state/refreshable when access-only is upgraded to Agent Identity
    /// (or when import intentionally leaves access-only for Codex).
    pub async fn update_credential_auth_meta(
        &self,
        id: &str,
        kind: &str,
        state: &str,
        refreshable: bool,
        email: Option<&str>,
        label: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE credentials SET kind = ?, state = ?, refreshable = ?,
             email = COALESCE(?, email),
             label = CASE WHEN (label IS NULL OR trim(label) = '') THEN COALESCE(?, label) ELSE label END,
             expires_at = COALESCE(?, expires_at),
             updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(kind)
        .bind(state)
        .bind(refreshable as i64)
        .bind(email)
        .bind(label)
        .bind(expires_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Best diagnostic string for proxy 401 when no healthy credential remains.
    pub async fn latest_credential_error_for_provider(
        &self,
        provider_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT last_error FROM credentials
             WHERE provider_id = ? AND last_error IS NOT NULL AND trim(last_error) != ''
             ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.get::<String, _>("last_error")))
    }

    /// Set label only when currently NULL/empty (import must not overwrite user renames).
    pub async fn fill_credential_label_if_empty(
        &self,
        id: &str,
        label: &str,
    ) -> Result<(), sqlx::Error> {
        let label = label.trim();
        if label.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE credentials SET label = ?, updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND (label IS NULL OR trim(label) = '')",
        )
        .bind(label)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Rename account display label. Empty string clears to NULL (fallback to email/id).
    pub async fn rename_credential(&self, id: &str, label: &str) -> Result<(), sqlx::Error> {
        let trimmed = label.trim();
        let owned: Option<String> = if trimmed.is_empty() {
            None
        } else {
            let mut out = String::new();
            for ch in trimmed.chars().take(64) {
                out.push(ch);
            }
            Some(out)
        };
        let result = sqlx::query(
            "UPDATE credentials SET label = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(owned.as_deref())
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::Protocol("账号不存在".into()));
        }
        Ok(())
    }

    /// One-shot repair: Agent Identity is durable without OAuth refresh_token.
    pub async fn repair_agent_identity_refreshable_flags(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE credentials SET refreshable = 1, updated_at = CURRENT_TIMESTAMP
             WHERE kind IN ('agent_identity', 'agentIdentity') AND refreshable = 0",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn ensure_default_pool(&self, provider_id: &str) -> Result<String, sqlx::Error> {
        let id = format!("default-{provider_id}");
        let defaults = PoolSchedulerConfig::default().to_json();
        sqlx::query(
            "INSERT INTO account_pools (id, name, provider_id, strategy, sticky_ttl_secs, scheduler_config_json)
             VALUES (?, ?, ?, 'load_aware_top_k', 3600, ?)
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(&id)
        .bind(format!("{} 默认账号池", provider_id))
        .bind(provider_id)
        .bind(&defaults)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn get_provider_routing(
        &self,
        provider_id: &str,
    ) -> Result<Option<ProviderRouting>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, routing_mode, fixed_credential_id, active_pool_id FROM providers WHERE id = ?",
        )
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| ProviderRouting {
            provider_id: row.get("id"),
            routing_mode: row
                .try_get::<String, _>("routing_mode")
                .unwrap_or_else(|_| "pool".into()),
            fixed_credential_id: row
                .try_get::<Option<String>, _>("fixed_credential_id")
                .unwrap_or(None),
            active_pool_id: row.get("active_pool_id"),
        }))
    }

    pub async fn set_provider_routing(
        &self,
        provider_id: &str,
        routing_mode: &str,
        fixed_credential_id: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let mode = RoutingMode::parse(routing_mode);
        let fixed = if mode == RoutingMode::Fixed {
            fixed_credential_id.map(str::to_string)
        } else {
            None
        };
        if mode == RoutingMode::Fixed {
            let Some(cred) = fixed.as_deref() else {
                return Err(sqlx::Error::Protocol(
                    "fixed routing requires fixed_credential_id".into(),
                ));
            };
            let ok =
                sqlx::query("SELECT 1 AS ok FROM credentials WHERE id = ? AND provider_id = ?")
                    .bind(cred)
                    .bind(provider_id)
                    .fetch_optional(&self.pool)
                    .await?
                    .is_some();
            if !ok {
                return Err(sqlx::Error::Protocol(
                    "fixed credential does not belong to provider".into(),
                ));
            }
        }
        sqlx::query("UPDATE providers SET routing_mode = ?, fixed_credential_id = ? WHERE id = ?")
            .bind(mode.as_str())
            .bind(fixed.as_deref())
            .bind(provider_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_pool_scheduler_config(
        &self,
        pool_id: &str,
    ) -> Result<PoolSchedulerConfig, sqlx::Error> {
        let row = sqlx::query(
            "SELECT scheduler_config_json, sticky_ttl_secs FROM account_pools WHERE id = ?",
        )
        .bind(pool_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(PoolSchedulerConfig::default());
        };
        let json: String = row
            .try_get("scheduler_config_json")
            .unwrap_or_else(|_| "{}".into());
        let mut config = PoolSchedulerConfig::from_json(&json);
        // Honor legacy sticky_ttl_secs when config still has defaults and json empty-ish.
        if json.trim() == "{}" {
            if let Ok(ttl) = row.try_get::<i64, _>("sticky_ttl_secs") {
                if ttl > 0 {
                    config.sticky_session_ttl_secs = ttl;
                    config.sticky_response_id_ttl_secs = ttl;
                }
            }
        }
        config.sanitize();
        Ok(config)
    }

    pub async fn update_pool_scheduler_config(
        &self,
        pool_id: &str,
        config: &PoolSchedulerConfig,
    ) -> Result<(), sqlx::Error> {
        let mut config = config.clone();
        config.sanitize();
        let json = config.to_json();
        sqlx::query(
            "UPDATE account_pools SET scheduler_config_json = ?, sticky_ttl_secs = ?, strategy = 'load_aware_top_k', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&json)
        .bind(config.sticky_session_ttl_secs)
        .bind(pool_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_pool_members_detailed(
        &self,
        pool_id: &str,
    ) -> Result<Vec<PoolMemberDetail>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT pm.pool_id, pm.credential_id, pm.weight, pm.priority, pm.enabled, pm.concurrency_limit,
                    COALESCE(pm.upstream_cost_rate, 1.0) AS upstream_cost_rate,
                    c.label, c.email, c.healthy, c.schedule_state, c.cooldown_until, c.last_error
             FROM pool_members pm
             JOIN credentials c ON c.id = pm.credential_id
             WHERE pm.pool_id = ?
             ORDER BY pm.priority DESC, pm.created_at",
        )
        .bind(pool_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| PoolMemberDetail {
                pool_id: row.get("pool_id"),
                credential_id: row.get("credential_id"),
                weight: row.get("weight"),
                priority: row.get("priority"),
                enabled: row.get::<i64, _>("enabled") != 0,
                concurrency_limit: row
                    .try_get::<i64, _>("concurrency_limit")
                    .unwrap_or(1)
                    .max(1),
                upstream_cost_rate: {
                    let rate: f64 = row.try_get("upstream_cost_rate").unwrap_or(1.0);
                    if rate.is_finite() && rate > 0.0 {
                        rate
                    } else {
                        1.0
                    }
                },
                label: row.get("label"),
                masked_email: row
                    .get::<Option<String>, _>("email")
                    .as_deref()
                    .map(mask_identity),
                healthy: row.get::<i64, _>("healthy") != 0,
                schedule_state: row
                    .try_get::<String, _>("schedule_state")
                    .unwrap_or_else(|_| "ready".into()),
                cooldown_until: row.try_get("cooldown_until").unwrap_or(None),
                last_error: row.get("last_error"),
            })
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_pool_member(
        &self,
        pool_id: &str,
        credential_id: &str,
        weight: i64,
        priority: i64,
        enabled: bool,
        concurrency_limit: i64,
        upstream_cost_rate: Option<f64>,
    ) -> Result<(), sqlx::Error> {
        let rate = upstream_cost_rate
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(1.0)
            .clamp(0.01, 100.0);
        sqlx::query(
            "UPDATE pool_members SET weight = ?, priority = ?, enabled = ?, concurrency_limit = ?,
                upstream_cost_rate = ?
             WHERE pool_id = ? AND credential_id = ?",
        )
        .bind(weight.max(1))
        .bind(priority)
        .bind(enabled as i64)
        .bind(concurrency_limit.max(1))
        .bind(rate)
        .bind(pool_id)
        .bind(credential_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    fn now_unix() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    fn hash_binding_key(raw: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"codex-select-sticky-v1\0");
        hasher.update(raw.as_bytes());
        hex::encode(hasher.finalize())
    }

    pub async fn get_sticky_binding(
        &self,
        pool_id: &str,
        kind: BindingKind,
        raw_key: &str,
        now_unix: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let key_hash = Self::hash_binding_key(raw_key);
        let row = sqlx::query(
            "SELECT credential_id, expires_at FROM sticky_bindings
             WHERE pool_id = ? AND binding_kind = ? AND binding_key_hash = ?",
        )
        .bind(pool_id)
        .bind(kind.as_str())
        .bind(&key_hash)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let expires_at: i64 = row.get("expires_at");
        if expires_at <= now_unix {
            sqlx::query(
                "DELETE FROM sticky_bindings WHERE pool_id = ? AND binding_kind = ? AND binding_key_hash = ?",
            )
            .bind(pool_id)
            .bind(kind.as_str())
            .bind(&key_hash)
            .execute(&self.pool)
            .await?;
            return Ok(None);
        }
        Ok(Some(row.get("credential_id")))
    }

    pub async fn put_sticky_binding(
        &self,
        pool_id: &str,
        kind: BindingKind,
        raw_key: &str,
        credential_id: &str,
        ttl_secs: i64,
        now_unix: i64,
    ) -> Result<(), sqlx::Error> {
        let key_hash = Self::hash_binding_key(raw_key);
        let expires_at = now_unix + ttl_secs.max(60);
        sqlx::query(
            "INSERT INTO sticky_bindings (pool_id, binding_kind, binding_key_hash, credential_id, expires_at, updated_at)
             VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
             ON CONFLICT(pool_id, binding_kind, binding_key_hash) DO UPDATE SET
               credential_id = excluded.credential_id,
               expires_at = excluded.expires_at,
               updated_at = CURRENT_TIMESTAMP",
        )
        .bind(pool_id)
        .bind(kind.as_str())
        .bind(&key_hash)
        .bind(credential_id)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_pool_candidates(
        &self,
        pool_id: &str,
    ) -> Result<Vec<CandidateAccount>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT pm.credential_id, pm.weight, pm.priority, pm.enabled, pm.concurrency_limit,
                    COALESCE(pm.upstream_cost_rate, 1.0) AS upstream_cost_rate,
                    c.healthy, c.schedule_state, c.cooldown_until, c.error_rate_ewma, c.ttft_ewma_ms,
                    c.last_used_at,
                    (SELECT COUNT(*) FROM account_leases l
                      WHERE l.credential_id = pm.credential_id
                        AND l.released_at IS NULL
                        AND (l.expires_at IS NULL OR l.expires_at > CURRENT_TIMESTAMP)
                    ) AS active_leases
             FROM pool_members pm
             JOIN credentials c ON c.id = pm.credential_id
             WHERE pm.pool_id = ?",
        )
        .bind(pool_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| CandidateAccount {
                credential_id: row.get("credential_id"),
                weight: row.get::<i64, _>("weight").max(1),
                priority: row.get("priority"),
                enabled: row.get::<i64, _>("enabled") != 0,
                healthy: row.get::<i64, _>("healthy") != 0,
                schedule_state: ScheduleState::parse(
                    &row.try_get::<String, _>("schedule_state")
                        .unwrap_or_else(|_| "ready".into()),
                ),
                cooldown_until: row.try_get("cooldown_until").unwrap_or(None),
                active_leases: row.get("active_leases"),
                concurrency_limit: row
                    .try_get::<i64, _>("concurrency_limit")
                    .unwrap_or(1)
                    .max(1),
                error_rate_ewma: row.try_get("error_rate_ewma").unwrap_or(0.0),
                ttft_ewma_ms: row.try_get("ttft_ewma_ms").unwrap_or(0.0),
                quota_remaining: None,
                session_reset_at: None,
                quota_fetched_at: None,
                upstream_cost_rate: {
                    let rate: f64 = row.try_get("upstream_cost_rate").unwrap_or(1.0);
                    if rate.is_finite() && rate > 0.0 {
                        rate
                    } else {
                        1.0
                    }
                },
                last_used_at: row.try_get("last_used_at").unwrap_or(None),
            })
            .collect())
    }

    async fn hydrate_quota_fields(
        &self,
        candidates: &mut [CandidateAccount],
    ) -> Result<(), sqlx::Error> {
        for candidate in candidates.iter_mut() {
            if let Some(snapshot) = self.cached_quota_snapshot(&candidate.credential_id).await? {
                candidate.quota_fetched_at = Some(snapshot.fetched_at);
                if let Some(five) = snapshot.five_hour.as_ref() {
                    candidate.quota_remaining =
                        Some((five.remaining_percent / 100.0).clamp(0.0, 1.0));
                    candidate.session_reset_at = five.reset_at.or(candidate.session_reset_at);
                } else if let Some(seven) = snapshot.seven_day.as_ref() {
                    candidate.quota_remaining =
                        Some((seven.remaining_percent / 100.0).clamp(0.0, 1.0));
                }
                if candidate.session_reset_at.is_none() {
                    candidate.session_reset_at = snapshot
                        .seven_day
                        .as_ref()
                        .and_then(|window| window.reset_at);
                }
                // Secondary 7d low-remain pressure: when 7d remaining is very low, shrink headroom.
                if let (Some(five_rem), Some(seven)) =
                    (candidate.quota_remaining, snapshot.seven_day.as_ref())
                {
                    let seven_rem = (seven.remaining_percent / 100.0).clamp(0.0, 1.0);
                    if seven_rem < 0.10 {
                        candidate.quota_remaining = Some(five_rem.min(seven_rem));
                    }
                }
            }
        }
        Ok(())
    }

    /// Select a credential for a provider request (Sub2API-like pipeline).
    pub async fn select_for_request(
        &self,
        provider_id: &str,
        previous_response_id: Option<&str>,
        session_key: Option<&str>,
        exclude_credential_ids: &[String],
    ) -> Result<Option<Lease>, sqlx::Error> {
        // Expire stale leases first.
        sqlx::query(
            "UPDATE account_leases SET released_at = CURRENT_TIMESTAMP
             WHERE released_at IS NULL AND expires_at IS NOT NULL AND expires_at <= CURRENT_TIMESTAMP",
        )
        .execute(&self.pool)
        .await?;

        let routing = self.get_provider_routing(provider_id).await?;
        let routing_mode = RoutingMode::parse(
            routing
                .as_ref()
                .map(|r| r.routing_mode.as_str())
                .unwrap_or("pool"),
        );
        let fixed_id = routing.as_ref().and_then(|r| r.fixed_credential_id.clone());

        let pool_id = match self.active_pool_id(provider_id).await? {
            Some(id) => id,
            None => {
                // No pool: fixed credential or first healthy.
                if routing_mode == RoutingMode::Fixed {
                    if let Some(id) = fixed_id {
                        return self
                            .acquire_direct_lease(provider_id, &id, SelectionLayer::Fixed)
                            .await;
                    }
                }
                return Ok(None);
            }
        };

        let config = self.get_pool_scheduler_config(&pool_id).await?;
        let now_unix = Self::now_unix();

        let previous_binding = if let Some(raw) = previous_response_id {
            self.get_sticky_binding(&pool_id, BindingKind::PreviousResponse, raw, now_unix)
                .await?
        } else {
            None
        };
        let session_binding = if let Some(raw) = session_key {
            self.get_sticky_binding(&pool_id, BindingKind::Session, raw, now_unix)
                .await?
        } else {
            None
        };

        let mut candidates = if routing_mode == RoutingMode::Fixed {
            // Fixed may use a credential that is not in pool_members; synthesize a candidate.
            let mut list = self.load_pool_candidates(&pool_id).await?;
            if let Some(ref fixed) = fixed_id {
                if !list.iter().any(|c| &c.credential_id == fixed) {
                    if let Some(row) = sqlx::query(
                        "SELECT id, healthy, schedule_state, cooldown_until, error_rate_ewma, ttft_ewma_ms FROM credentials WHERE id = ? AND provider_id = ?",
                    )
                    .bind(fixed)
                    .bind(provider_id)
                    .fetch_optional(&self.pool)
                    .await?
                    {
                        list.push(CandidateAccount {
                            credential_id: row.get("id"),
                            weight: 1,
                            priority: 0,
                            enabled: true,
                            healthy: row.get::<i64, _>("healthy") != 0,
                            schedule_state: ScheduleState::parse(
                                &row
                                    .try_get::<String, _>("schedule_state")
                                    .unwrap_or_else(|_| "ready".into()),
                            ),
                            cooldown_until: row.try_get("cooldown_until").unwrap_or(None),
                            active_leases: 0,
                            concurrency_limit: 1,
                            error_rate_ewma: row.try_get("error_rate_ewma").unwrap_or(0.0),
                            ttft_ewma_ms: row.try_get("ttft_ewma_ms").unwrap_or(0.0),
                            quota_remaining: None,
                            session_reset_at: None,
                            quota_fetched_at: None,
                            upstream_cost_rate: 1.0,
                            last_used_at: None,
                        });
                    }
                }
            }
            list
        } else {
            self.load_pool_candidates(&pool_id).await?
        };
        self.hydrate_quota_fields(&mut candidates).await?;

        if candidates.is_empty() && routing_mode == RoutingMode::Pool {
            // Fallback: any healthy credential on provider (single-account instances).
            if let Some(cred) = self.first_healthy_credential(provider_id).await? {
                return self
                    .acquire_direct_lease(provider_id, &cred.id, SelectionLayer::LoadBalance)
                    .await;
            }
            return Ok(None);
        }

        let mut request = SelectRequest {
            routing: routing_mode,
            fixed_credential_id: fixed_id,
            previous_response_id: previous_response_id.map(str::to_string),
            session_key: session_key.map(str::to_string),
            exclude_credential_ids: exclude_credential_ids.to_vec(),
            now_unix,
            previous_response_binding: previous_binding,
            session_binding,
        };

        let seed = uuid::Uuid::new_v4();
        let seed_u64 = u64::from_le_bytes(seed.as_bytes()[..8].try_into().unwrap_or([0; 8]));
        let Some(mut outcome) = select_account(&candidates, &config, &request, seed_u64) else {
            return Ok(None);
        };

        // Sticky concurrency wait: prefer waiting for the same account (cache hit) over switching.
        if matches!(
            outcome.layer,
            SelectionLayer::PreviousResponse | SelectionLayer::Session
        ) && config.sticky_wait_enabled
            && config.sticky_wait_timeout_secs > 0
            && !config.sticky_weighted_enabled
        {
            if let Some(candidate) = candidates
                .iter()
                .find(|c| c.credential_id == outcome.credential_id)
            {
                if sticky_concurrency_full(candidate) {
                    let can_wait = self
                        .try_begin_waiter(
                            &pool_id,
                            Some(&outcome.credential_id),
                            "sticky",
                            config.sticky_wait_max_waiting,
                            config.sticky_wait_timeout_secs,
                        )
                        .await?;
                    let waited = if can_wait {
                        let result = self
                            .wait_for_credential_slot(
                                &outcome.credential_id,
                                config.sticky_wait_timeout_secs,
                            )
                            .await;
                        let _ = self
                            .end_waiter(&pool_id, Some(&outcome.credential_id), "sticky")
                            .await;
                        result?
                    } else {
                        false
                    };
                    // Refresh lease counts after wait.
                    candidates = self.load_pool_candidates(&pool_id).await?;
                    self.hydrate_quota_fields(&mut candidates).await?;
                    request.now_unix = Self::now_unix();
                    if !waited {
                        // Timed out or queue full: exclude sticky account from load-balance reselect.
                        if !request
                            .exclude_credential_ids
                            .iter()
                            .any(|id| id == &outcome.credential_id)
                        {
                            request
                                .exclude_credential_ids
                                .push(outcome.credential_id.clone());
                        }
                        // Clear sticky bindings so we do not re-pick the full account.
                        request.previous_response_binding = None;
                        request.session_binding = None;
                        let reseed = uuid::Uuid::new_v4();
                        let reseed_u64 =
                            u64::from_le_bytes(reseed.as_bytes()[..8].try_into().unwrap_or([0; 8]));
                        let Some(next) = select_account(&candidates, &config, &request, reseed_u64)
                        else {
                            return Ok(None);
                        };
                        outcome = next;
                    } else if let Some(refreshed) = candidates
                        .iter()
                        .find(|c| c.credential_id == outcome.credential_id)
                    {
                        if sticky_concurrency_full(refreshed) {
                            // Slot still full (race); fall through to reselect without sticky.
                            request.previous_response_binding = None;
                            request.session_binding = None;
                            if !request
                                .exclude_credential_ids
                                .iter()
                                .any(|id| id == &outcome.credential_id)
                            {
                                request
                                    .exclude_credential_ids
                                    .push(outcome.credential_id.clone());
                            }
                            let reseed = uuid::Uuid::new_v4();
                            let reseed_u64 = u64::from_le_bytes(
                                reseed.as_bytes()[..8].try_into().unwrap_or([0; 8]),
                            );
                            let Some(next) =
                                select_account(&candidates, &config, &request, reseed_u64)
                            else {
                                return Ok(None);
                            };
                            outcome = next;
                        }
                    }
                }
            }
        }

        // Fallback wait: all healthy accounts concurrency-full — wait for any free slot.
        if outcome.layer == SelectionLayer::LoadBalance
            && config.fallback_wait_enabled
            && config.fallback_wait_timeout_secs > 0
        {
            if let Some(candidate) = candidates
                .iter()
                .find(|c| c.credential_id == outcome.credential_id)
            {
                if sticky_concurrency_full(candidate) {
                    let can_wait = self
                        .try_begin_waiter(
                            &pool_id,
                            None,
                            "fallback",
                            config.fallback_max_waiting,
                            config.fallback_wait_timeout_secs,
                        )
                        .await?;
                    if can_wait {
                        let waited = self
                            .wait_for_any_pool_slot(&pool_id, config.fallback_wait_timeout_secs)
                            .await;
                        let _ = self.end_waiter(&pool_id, None, "fallback").await;
                        candidates = self.load_pool_candidates(&pool_id).await?;
                        self.hydrate_quota_fields(&mut candidates).await?;
                        request.now_unix = Self::now_unix();
                        if waited? {
                            let reseed = uuid::Uuid::new_v4();
                            let reseed_u64 = u64::from_le_bytes(
                                reseed.as_bytes()[..8].try_into().unwrap_or([0; 8]),
                            );
                            // Prefer free slots on reselect.
                            if let Some(next) =
                                select_account(&candidates, &config, &request, reseed_u64)
                            {
                                outcome = next;
                            }
                        }
                    }
                }
            }
        }

        self.finalize_selection(
            &pool_id,
            &config,
            previous_response_id,
            session_key,
            &outcome,
            Self::now_unix(),
        )
        .await
    }

    /// Wait until `credential_id` has a free concurrency slot, or timeout.
    /// Returns true if a slot appeared before timeout.
    async fn wait_for_credential_slot(
        &self,
        credential_id: &str,
        timeout_secs: i64,
    ) -> Result<bool, sqlx::Error> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.max(0) as u64);
        loop {
            // Expire stale leases so crashes don't block sticky wait forever.
            sqlx::query(
                "UPDATE account_leases SET released_at = CURRENT_TIMESTAMP
                 WHERE released_at IS NULL AND expires_at IS NOT NULL AND expires_at <= CURRENT_TIMESTAMP",
            )
            .execute(&self.pool)
            .await?;
            let active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM account_leases
                 WHERE credential_id = ?
                   AND released_at IS NULL
                   AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)",
            )
            .bind(credential_id)
            .fetch_one(&self.pool)
            .await?;
            let limit: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(concurrency_limit), 1) FROM pool_members WHERE credential_id = ? AND enabled = 1",
            )
            .bind(credential_id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(1)
            .max(1);
            if active < limit {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Wait until any member of the pool has a free concurrency slot.
    async fn wait_for_any_pool_slot(
        &self,
        pool_id: &str,
        timeout_secs: i64,
    ) -> Result<bool, sqlx::Error> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.max(0) as u64);
        loop {
            sqlx::query(
                "UPDATE account_leases SET released_at = CURRENT_TIMESTAMP
                 WHERE released_at IS NULL AND expires_at IS NOT NULL AND expires_at <= CURRENT_TIMESTAMP",
            )
            .execute(&self.pool)
            .await?;
            let free: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pool_members pm
                 WHERE pm.pool_id = ? AND pm.enabled = 1
                   AND (
                     SELECT COUNT(*) FROM account_leases l
                     WHERE l.credential_id = pm.credential_id
                       AND l.released_at IS NULL
                       AND (l.expires_at IS NULL OR l.expires_at > CURRENT_TIMESTAMP)
                   ) < pm.concurrency_limit",
            )
            .bind(pool_id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);
            if free > 0 {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Register a waiter if under max; returns false when queue is full.
    async fn try_begin_waiter(
        &self,
        pool_id: &str,
        credential_id: Option<&str>,
        kind: &str,
        max_waiting: u32,
        timeout_secs: i64,
    ) -> Result<bool, sqlx::Error> {
        let now = Self::now_unix();
        // Drop expired waiters.
        sqlx::query("DELETE FROM schedule_waiters WHERE expires_at <= ?")
            .bind(now)
            .execute(&self.pool)
            .await?;
        let count: i64 = if let Some(cred) = credential_id {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM schedule_waiters
                 WHERE pool_id = ? AND kind = ? AND credential_id = ? AND expires_at > ?",
            )
            .bind(pool_id)
            .bind(kind)
            .bind(cred)
            .bind(now)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM schedule_waiters
                 WHERE pool_id = ? AND kind = ? AND credential_id IS NULL AND expires_at > ?",
            )
            .bind(pool_id)
            .bind(kind)
            .bind(now)
            .fetch_one(&self.pool)
            .await?
        };
        if count >= max_waiting.max(1) as i64 {
            return Ok(false);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let expires = now + timeout_secs.max(1);
        sqlx::query(
            "INSERT INTO schedule_waiters (id, pool_id, credential_id, kind, expires_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(pool_id)
        .bind(credential_id)
        .bind(kind)
        .bind(expires)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    async fn end_waiter(
        &self,
        pool_id: &str,
        credential_id: Option<&str>,
        kind: &str,
    ) -> Result<(), sqlx::Error> {
        // Delete one matching waiter (best-effort; concurrent ends are fine).
        if let Some(cred) = credential_id {
            sqlx::query(
                "DELETE FROM schedule_waiters WHERE id = (
                   SELECT id FROM schedule_waiters
                   WHERE pool_id = ? AND kind = ? AND credential_id = ?
                   ORDER BY created_at LIMIT 1
                 )",
            )
            .bind(pool_id)
            .bind(kind)
            .bind(cred)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                "DELETE FROM schedule_waiters WHERE id = (
                   SELECT id FROM schedule_waiters
                   WHERE pool_id = ? AND kind = ? AND credential_id IS NULL
                   ORDER BY created_at LIMIT 1
                 )",
            )
            .bind(pool_id)
            .bind(kind)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn acquire_direct_lease(
        &self,
        _provider_id: &str,
        credential_id: &str,
        layer: SelectionLayer,
    ) -> Result<Option<Lease>, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        let ttl = PoolSchedulerConfig::default().lease_ttl_secs;
        // Use a synthetic pool id slot if needed — lease table requires pool_id.
        // Attach to any pool of this credential's provider or skip.
        let pool_id: Option<String> = sqlx::query_scalar(
            "SELECT p.id FROM account_pools p
             JOIN credentials c ON c.provider_id = p.provider_id
             WHERE c.id = ? AND p.enabled = 1
             ORDER BY p.created_at LIMIT 1",
        )
        .bind(credential_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(pool_id) = pool_id else {
            return Ok(Some(Lease {
                id: id.clone(),
                credential_id: credential_id.to_string(),
                layer,
                sticky_escaped: false,
            }));
        };
        sqlx::query(
            "INSERT INTO account_leases (id, pool_id, credential_id, affinity_key, expires_at)
             VALUES (?, ?, ?, NULL, datetime(CURRENT_TIMESTAMP, '+' || ? || ' seconds'))",
        )
        .bind(&id)
        .bind(&pool_id)
        .bind(credential_id)
        .bind(ttl)
        .execute(&self.pool)
        .await?;
        Ok(Some(Lease {
            id,
            credential_id: credential_id.to_string(),
            layer,
            sticky_escaped: false,
        }))
    }

    async fn finalize_selection(
        &self,
        pool_id: &str,
        config: &PoolSchedulerConfig,
        previous_response_id: Option<&str>,
        session_key: Option<&str>,
        outcome: &SelectOutcome,
        now_unix: i64,
    ) -> Result<Option<Lease>, sqlx::Error> {
        if outcome.rebind_previous_response {
            if let Some(raw) = previous_response_id {
                self.put_sticky_binding(
                    pool_id,
                    BindingKind::PreviousResponse,
                    raw,
                    &outcome.credential_id,
                    config.sticky_response_id_ttl_secs,
                    now_unix,
                )
                .await?;
            }
        } else if outcome.layer == SelectionLayer::PreviousResponse {
            // Refresh TTL on hit.
            if let Some(raw) = previous_response_id {
                self.put_sticky_binding(
                    pool_id,
                    BindingKind::PreviousResponse,
                    raw,
                    &outcome.credential_id,
                    config.sticky_response_id_ttl_secs,
                    now_unix,
                )
                .await?;
            }
        }

        if outcome.rebind_session || outcome.layer == SelectionLayer::Session {
            if let Some(raw) = session_key {
                self.put_sticky_binding(
                    pool_id,
                    BindingKind::Session,
                    raw,
                    &outcome.credential_id,
                    config.sticky_session_ttl_secs,
                    now_unix,
                )
                .await?;
            }
        } else if outcome.layer == SelectionLayer::LoadBalance {
            if let Some(raw) = session_key {
                self.put_sticky_binding(
                    pool_id,
                    BindingKind::Session,
                    raw,
                    &outcome.credential_id,
                    config.sticky_session_ttl_secs,
                    now_unix,
                )
                .await?;
            }
            if let Some(raw) = previous_response_id {
                self.put_sticky_binding(
                    pool_id,
                    BindingKind::PreviousResponse,
                    raw,
                    &outcome.credential_id,
                    config.sticky_response_id_ttl_secs,
                    now_unix,
                )
                .await?;
            }
        }

        let lease_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO account_leases (id, pool_id, credential_id, affinity_key, expires_at)
             VALUES (?, ?, ?, ?, datetime(CURRENT_TIMESTAMP, '+' || ? || ' seconds'))",
        )
        .bind(&lease_id)
        .bind(pool_id)
        .bind(&outcome.credential_id)
        .bind(outcome.layer.as_str())
        .bind(config.lease_ttl_secs)
        .execute(&self.pool)
        .await?;

        // Touch last_used for fallback last_used selection.
        let _ = sqlx::query(
            "UPDATE credentials SET last_used_at = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(now_unix)
        .bind(&outcome.credential_id)
        .execute(&self.pool)
        .await;

        Ok(Some(Lease {
            id: lease_id,
            credential_id: outcome.credential_id.clone(),
            layer: outcome.layer,
            sticky_escaped: outcome.sticky_escaped,
        }))
    }

    pub async fn release_lease(&self, lease_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE account_leases SET released_at = CURRENT_TIMESTAMP
             WHERE id = ? AND released_at IS NULL",
        )
        .bind(lease_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn release_all_leases(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE account_leases SET released_at = CURRENT_TIMESTAMP WHERE released_at IS NULL",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn mark_schedule_state(
        &self,
        credential_id: &str,
        state: ScheduleState,
        healthy: bool,
        error: Option<&str>,
        cooldown_until: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE credentials SET schedule_state = ?, healthy = ?, last_error = ?,
                cooldown_until = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(state.as_str())
        .bind(healthy as i64)
        .bind(error)
        .bind(cooldown_until)
        .bind(credential_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn apply_rate_limit_cooldown(
        &self,
        credential_id: &str,
        cooldown_secs: i64,
        retry_after_secs: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let secs = retry_after_secs.unwrap_or(cooldown_secs).max(1);
        let until = Self::now_unix() + secs;
        self.apply_rate_limit_until(credential_id, until, "上游限流 (429)")
            .await
    }

    /// Persist an absolute cooldown deadline (unix seconds).
    pub async fn apply_rate_limit_until(
        &self,
        credential_id: &str,
        cooldown_until: i64,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        let until = cooldown_until.max(Self::now_unix() + 1);
        let summary = if reason.chars().count() > 120 {
            format!("{}…", reason.chars().take(119).collect::<String>())
        } else {
            reason.to_string()
        };
        self.mark_schedule_state(
            credential_id,
            ScheduleState::RateLimited,
            true,
            Some(&summary),
            Some(until),
        )
        .await
    }

    pub async fn max_failover_switches(&self, provider_id: &str) -> Result<u32, sqlx::Error> {
        if let Some(pool_id) = self.active_pool_id(provider_id).await? {
            let config = self.get_pool_scheduler_config(&pool_id).await?;
            return Ok(config.max_failover_switches.max(1));
        }
        Ok(PoolSchedulerConfig::default().max_failover_switches.max(1))
    }

    pub async fn failover_on_400(&self, provider_id: &str) -> Result<bool, sqlx::Error> {
        if let Some(pool_id) = self.active_pool_id(provider_id).await? {
            let config = self.get_pool_scheduler_config(&pool_id).await?;
            return Ok(config.failover_on_400);
        }
        Ok(false)
    }

    pub async fn rate_limit_429_cooldown_enabled(
        &self,
        provider_id: &str,
    ) -> Result<bool, sqlx::Error> {
        if let Some(pool_id) = self.active_pool_id(provider_id).await? {
            let config = self.get_pool_scheduler_config(&pool_id).await?;
            return Ok(config.rate_limit_429_cooldown_enabled);
        }
        Ok(true)
    }

    pub async fn overload_529_cooldown_secs(&self, provider_id: &str) -> Result<i64, sqlx::Error> {
        if let Some(pool_id) = self.active_pool_id(provider_id).await? {
            let config = self.get_pool_scheduler_config(&pool_id).await?;
            return Ok(config.overload_529_cooldown_secs.max(1));
        }
        Ok(PoolSchedulerConfig::default()
            .overload_529_cooldown_secs
            .max(1))
    }

    pub async fn default_429_cooldown_secs(&self, provider_id: &str) -> Result<i64, sqlx::Error> {
        if let Some(pool_id) = self.active_pool_id(provider_id).await? {
            let config = self.get_pool_scheduler_config(&pool_id).await?;
            return Ok(config.default_429_cooldown_secs.max(1));
        }
        Ok(30)
    }

    pub async fn credential_fingerprint_prefix(
        &self,
        credential_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query("SELECT fingerprint FROM credentials WHERE id = ?")
            .bind(credential_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| {
            row.get::<String, _>("fingerprint")
                .chars()
                .take(12)
                .collect()
        }))
    }

    pub async fn record_proxy_request_event(
        &self,
        event: &ProxyRequestEvent,
        max_events: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO proxy_request_events (
                id, created_at, route_slug, display_name, provider_id, upstream_model, protocol,
                selection_layer, sticky_escaped, account_fingerprint, schedule_state,
                result_category, failover_attempt, latency_ms_total, first_token_ms,
                cooldown_applied, error_summary
             ) VALUES (?, COALESCE(?, CURRENT_TIMESTAMP), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event.id)
        .bind(if event.created_at.is_empty() {
            None::<String>
        } else {
            Some(event.created_at.clone())
        })
        .bind(&event.route_slug)
        .bind(&event.display_name)
        .bind(&event.provider_id)
        .bind(&event.upstream_model)
        .bind(&event.protocol)
        .bind(&event.selection_layer)
        .bind(event.sticky_escaped as i64)
        .bind(&event.account_fingerprint)
        .bind(&event.schedule_state)
        .bind(&event.result_category)
        .bind(event.failover_attempt as i64)
        .bind(event.latency_ms_total)
        .bind(event.first_token_ms)
        .bind(event.cooldown_applied as i64)
        .bind(&event.error_summary)
        .execute(&self.pool)
        .await?;

        let cap = max_events.clamp(50, 1000);
        sqlx::query(
            "DELETE FROM proxy_request_events WHERE id IN (
                SELECT id FROM proxy_request_events
                ORDER BY created_at DESC
                LIMIT -1 OFFSET ?
             )",
        )
        .bind(cap)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_proxy_request_events(
        &self,
        limit: i64,
    ) -> Result<Vec<ProxyRequestEvent>, sqlx::Error> {
        let limit = limit.clamp(1, 1000);
        let rows = sqlx::query(
            "SELECT id, created_at, route_slug, display_name, provider_id, upstream_model, protocol,
                    selection_layer, sticky_escaped, account_fingerprint, schedule_state,
                    result_category, failover_attempt, latency_ms_total, first_token_ms,
                    cooldown_applied, error_summary
             FROM proxy_request_events
             ORDER BY created_at DESC
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| ProxyRequestEvent {
                id: row.get("id"),
                created_at: row.get("created_at"),
                route_slug: row.get("route_slug"),
                display_name: row.get("display_name"),
                provider_id: row.get("provider_id"),
                upstream_model: row.get("upstream_model"),
                protocol: row.get("protocol"),
                selection_layer: row.get("selection_layer"),
                sticky_escaped: row.get::<i64, _>("sticky_escaped") != 0,
                account_fingerprint: row.get("account_fingerprint"),
                schedule_state: row.get("schedule_state"),
                result_category: row.get("result_category"),
                failover_attempt: row.get::<i64, _>("failover_attempt") as u32,
                latency_ms_total: row.get("latency_ms_total"),
                first_token_ms: row.get("first_token_ms"),
                cooldown_applied: row.get::<i64, _>("cooldown_applied") != 0,
                error_summary: row.get("error_summary"),
            })
            .collect())
    }

    pub async fn clear_proxy_request_events(&self) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM proxy_request_events")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn diagnostics_max_events(&self) -> Result<i64, sqlx::Error> {
        let row =
            sqlx::query("SELECT value_json FROM app_settings WHERE key = 'diagnostics.max_events'")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|row| {
                let json: String = row.get("value_json");
                serde_json::from_str::<i64>(&json).ok()
            })
            .unwrap_or(200)
            .clamp(50, 1000))
    }

    pub async fn set_diagnostics_max_events(&self, max_events: i64) -> Result<i64, sqlx::Error> {
        let value = max_events.clamp(50, 1000);
        sqlx::query(
            "INSERT INTO app_settings (key, value_json) VALUES ('diagnostics.max_events', ?)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(value.to_string())
        .execute(&self.pool)
        .await?;
        Ok(value)
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
        let defaults = PoolSchedulerConfig::default().to_json();
        sqlx::query(
            "INSERT INTO account_pools (id, name, provider_id, strategy, sticky_ttl_secs, scheduler_config_json)
             VALUES (?, ?, ?, 'load_aware_top_k', 3600, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(provider_id)
        .bind(&defaults)
        .execute(&self.pool)
        .await?;
        // If this provider has no active pool yet, activate the new one.
        sqlx::query(
            "UPDATE providers SET active_pool_id = ? WHERE id = ? AND (active_pool_id IS NULL OR active_pool_id = '')",
        )
        .bind(&id)
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

    pub async fn mark_credential_health(
        &self,
        id: &str,
        healthy: bool,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let state = if healthy {
            ScheduleState::Ready
        } else {
            ScheduleState::AuthInvalid
        };
        self.mark_schedule_state(id, state, healthy, error, None)
            .await
    }

    pub async fn record_usage(
        &self,
        provider_id: &str,
        model_id: &str,
        delta: &UsageDelta,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO usage_events (day, provider_id, model_id, request_count, input_tokens, output_tokens, cache_observations, cache_hits, failed_requests) VALUES (date('now','localtime'), ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(day, provider_id, model_id) DO UPDATE SET request_count = request_count + excluded.request_count, input_tokens = input_tokens + excluded.input_tokens, output_tokens = output_tokens + excluded.output_tokens, cache_observations = cache_observations + excluded.cache_observations, cache_hits = cache_hits + excluded.cache_hits, failed_requests = failed_requests + excluded.failed_requests, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(provider_id)
        .bind(model_id)
        .bind(delta.request_count)
        .bind(delta.input_tokens)
        .bind(delta.output_tokens)
        .bind(delta.cache_observations)
        .bind(delta.cache_hits)
        .bind(delta.failed_requests)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn usage_snapshot(&self) -> Result<crate::domain::UsageSnapshot, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(request_count), 0) AS request_count, COALESCE(SUM(input_tokens), 0) AS input_tokens, COALESCE(SUM(output_tokens), 0) AS output_tokens, COALESCE(SUM(cache_observations), 0) AS cache_observations, COALESCE(SUM(cache_hits), 0) AS cache_hits, COALESCE(SUM(failed_requests), 0) AS failed_requests, COALESCE(SUM(CASE WHEN day = date('now','localtime') THEN input_tokens + output_tokens ELSE 0 END), 0) AS today_tokens, COALESCE(SUM(CASE WHEN day >= date('now','localtime', '-6 day') THEN input_tokens + output_tokens ELSE 0 END), 0) AS seven_day_tokens FROM usage_events",
        )
        .fetch_one(&self.pool)
        .await?;
        let cache_observations = row.get::<i64, _>("cache_observations");
        let cache_hit_rate = (cache_observations > 0)
            .then(|| row.get::<i64, _>("cache_hits") as f64 / cache_observations as f64);
        Ok(crate::domain::UsageSnapshot {
            request_count: row.get::<i64, _>("request_count") as u64,
            input_tokens: row.get::<i64, _>("input_tokens") as u64,
            output_tokens: row.get::<i64, _>("output_tokens") as u64,
            total_tokens: (row.get::<i64, _>("input_tokens") + row.get::<i64, _>("output_tokens"))
                as u64,
            today_tokens: row.get::<i64, _>("today_tokens") as u64,
            seven_day_tokens: row.get::<i64, _>("seven_day_tokens") as u64,
            cache_hit_rate,
            failed_requests: row.get::<i64, _>("failed_requests") as u64,
            sampled_at: chrono_like_now(),
        })
    }

    pub async fn usage_dashboard(
        &self,
        range: UsageRange,
    ) -> Result<UsageDashboardSnapshot, sqlx::Error> {
        // Static SQL only: sqlx 0.9 rejects dynamic format! strings without AssertSqlSafe.
        // Range windows are fixed enum values, so each branch uses a literal query.
        let summary = match range {
            UsageRange::SevenDays => sqlx::query(
                "SELECT COALESCE(SUM(request_count),0) request_count, COALESCE(SUM(input_tokens),0) input_tokens, COALESCE(SUM(output_tokens),0) output_tokens, COALESCE(SUM(failed_requests),0) failed_requests, COALESCE(SUM(cache_observations),0) cache_observations, COALESCE(SUM(cache_hits),0) cache_hits FROM usage_events WHERE day >= date('now','localtime', '-6 day')",
            )
            .fetch_one(&self.pool)
            .await?,
            UsageRange::ThirtyDays => sqlx::query(
                "SELECT COALESCE(SUM(request_count),0) request_count, COALESCE(SUM(input_tokens),0) input_tokens, COALESCE(SUM(output_tokens),0) output_tokens, COALESCE(SUM(failed_requests),0) failed_requests, COALESCE(SUM(cache_observations),0) cache_observations, COALESCE(SUM(cache_hits),0) cache_hits FROM usage_events WHERE day >= date('now','localtime', '-29 day')",
            )
            .fetch_one(&self.pool)
            .await?,
            UsageRange::All => sqlx::query(
                "SELECT COALESCE(SUM(request_count),0) request_count, COALESCE(SUM(input_tokens),0) input_tokens, COALESCE(SUM(output_tokens),0) output_tokens, COALESCE(SUM(failed_requests),0) failed_requests, COALESCE(SUM(cache_observations),0) cache_observations, COALESCE(SUM(cache_hits),0) cache_hits FROM usage_events",
            )
            .fetch_one(&self.pool)
            .await?,
        };
        let request_count = summary.get::<i64, _>("request_count") as u64;
        let input_tokens = summary.get::<i64, _>("input_tokens") as u64;
        let output_tokens = summary.get::<i64, _>("output_tokens") as u64;
        let failed_requests = summary.get::<i64, _>("failed_requests") as u64;
        let cache_observations = summary.get::<i64, _>("cache_observations");
        let total_tokens = input_tokens + output_tokens;
        let cache_hit_rate = (cache_observations > 0)
            .then(|| summary.get::<i64, _>("cache_hits") as f64 / cache_observations as f64);

        let trend_rows = match range {
            UsageRange::SevenDays => sqlx::query(
                "SELECT day, SUM(request_count) request_count, SUM(input_tokens) input_tokens, SUM(output_tokens) output_tokens, SUM(failed_requests) failed_requests, SUM(cache_observations) cache_observations, SUM(cache_hits) cache_hits FROM usage_events WHERE day >= date('now','localtime', '-6 day') GROUP BY day ORDER BY day",
            )
            .fetch_all(&self.pool)
            .await?,
            UsageRange::ThirtyDays => sqlx::query(
                "SELECT day, SUM(request_count) request_count, SUM(input_tokens) input_tokens, SUM(output_tokens) output_tokens, SUM(failed_requests) failed_requests, SUM(cache_observations) cache_observations, SUM(cache_hits) cache_hits FROM usage_events WHERE day >= date('now','localtime', '-29 day') GROUP BY day ORDER BY day",
            )
            .fetch_all(&self.pool)
            .await?,
            UsageRange::All => sqlx::query(
                "SELECT day, SUM(request_count) request_count, SUM(input_tokens) input_tokens, SUM(output_tokens) output_tokens, SUM(failed_requests) failed_requests, SUM(cache_observations) cache_observations, SUM(cache_hits) cache_hits FROM usage_events GROUP BY day ORDER BY day",
            )
            .fetch_all(&self.pool)
            .await?,
        };
        let trend_data: std::collections::HashMap<String, UsageTrendPoint> = trend_rows
            .into_iter()
            .map(|row| {
                let input = row.get::<i64, _>("input_tokens") as u64;
                let output = row.get::<i64, _>("output_tokens") as u64;
                let observations = row.get::<i64, _>("cache_observations");
                let point = UsageTrendPoint {
                    day: row.get("day"),
                    request_count: row.get::<i64, _>("request_count") as u64,
                    input_tokens: input,
                    output_tokens: output,
                    total_tokens: input + output,
                    failed_requests: row.get::<i64, _>("failed_requests") as u64,
                    cache_hit_rate: (observations > 0)
                        .then(|| row.get::<i64, _>("cache_hits") as f64 / observations as f64),
                };
                (point.day.clone(), point)
            })
            .collect();
        let trend = if trend_data.is_empty() {
            Vec::new()
        } else {
            let days = match range {
                UsageRange::SevenDays => {
                    sqlx::query_scalar::<_, String>(
                        "WITH RECURSIVE dates(day) AS (SELECT date('now','localtime', '-6 day') UNION ALL SELECT date(day, '+1 day') FROM dates WHERE day < date('now','localtime')) SELECT day FROM dates ORDER BY day",
                    )
                    .fetch_all(&self.pool)
                    .await?
                }
                UsageRange::ThirtyDays => {
                    sqlx::query_scalar::<_, String>(
                        "WITH RECURSIVE dates(day) AS (SELECT date('now','localtime', '-29 day') UNION ALL SELECT date(day, '+1 day') FROM dates WHERE day < date('now','localtime')) SELECT day FROM dates ORDER BY day",
                    )
                    .fetch_all(&self.pool)
                    .await?
                }
                UsageRange::All => {
                    sqlx::query_scalar::<_, String>(
                        "WITH RECURSIVE dates(day) AS (SELECT (SELECT MIN(day) FROM usage_events) UNION ALL SELECT date(day, '+1 day') FROM dates WHERE day < date('now','localtime')) SELECT day FROM dates ORDER BY day",
                    )
                    .fetch_all(&self.pool)
                    .await?
                }
            };
            days.into_iter()
                .map(|day| {
                    trend_data.get(&day).cloned().unwrap_or(UsageTrendPoint {
                        day,
                        request_count: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        total_tokens: 0,
                        failed_requests: 0,
                        cache_hit_rate: None,
                    })
                })
                .collect()
        };

        let model_rows = match range {
            UsageRange::SevenDays => sqlx::query(
                "SELECT model_id name, SUM(request_count) request_count, SUM(input_tokens) input_tokens, SUM(output_tokens) output_tokens, SUM(failed_requests) failed_requests FROM usage_events WHERE day >= date('now','localtime', '-6 day') GROUP BY model_id ORDER BY (SUM(input_tokens) + SUM(output_tokens)) DESC, model_id",
            )
            .fetch_all(&self.pool)
            .await?,
            UsageRange::ThirtyDays => sqlx::query(
                "SELECT model_id name, SUM(request_count) request_count, SUM(input_tokens) input_tokens, SUM(output_tokens) output_tokens, SUM(failed_requests) failed_requests FROM usage_events WHERE day >= date('now','localtime', '-29 day') GROUP BY model_id ORDER BY (SUM(input_tokens) + SUM(output_tokens)) DESC, model_id",
            )
            .fetch_all(&self.pool)
            .await?,
            UsageRange::All => sqlx::query(
                "SELECT model_id name, SUM(request_count) request_count, SUM(input_tokens) input_tokens, SUM(output_tokens) output_tokens, SUM(failed_requests) failed_requests FROM usage_events GROUP BY model_id ORDER BY (SUM(input_tokens) + SUM(output_tokens)) DESC, model_id",
            )
            .fetch_all(&self.pool)
            .await?,
        };
        let provider_rows = match range {
            UsageRange::SevenDays => sqlx::query(
                "SELECT COALESCE(NULLIF(p.name, ''), '已删除供应商 · ' || ue.provider_id) name, SUM(ue.request_count) request_count, SUM(ue.input_tokens) input_tokens, SUM(ue.output_tokens) output_tokens, SUM(ue.failed_requests) failed_requests FROM usage_events ue LEFT JOIN providers p ON p.id = ue.provider_id WHERE ue.day >= date('now','localtime', '-6 day') GROUP BY ue.provider_id, p.name ORDER BY (SUM(ue.input_tokens) + SUM(ue.output_tokens)) DESC, name",
            )
            .fetch_all(&self.pool)
            .await?,
            UsageRange::ThirtyDays => sqlx::query(
                "SELECT COALESCE(NULLIF(p.name, ''), '已删除供应商 · ' || ue.provider_id) name, SUM(ue.request_count) request_count, SUM(ue.input_tokens) input_tokens, SUM(ue.output_tokens) output_tokens, SUM(ue.failed_requests) failed_requests FROM usage_events ue LEFT JOIN providers p ON p.id = ue.provider_id WHERE ue.day >= date('now','localtime', '-29 day') GROUP BY ue.provider_id, p.name ORDER BY (SUM(ue.input_tokens) + SUM(ue.output_tokens)) DESC, name",
            )
            .fetch_all(&self.pool)
            .await?,
            UsageRange::All => sqlx::query(
                "SELECT COALESCE(NULLIF(p.name, ''), '已删除供应商 · ' || ue.provider_id) name, SUM(ue.request_count) request_count, SUM(ue.input_tokens) input_tokens, SUM(ue.output_tokens) output_tokens, SUM(ue.failed_requests) failed_requests FROM usage_events ue LEFT JOIN providers p ON p.id = ue.provider_id GROUP BY ue.provider_id, p.name ORDER BY (SUM(ue.input_tokens) + SUM(ue.output_tokens)) DESC, name",
            )
            .fetch_all(&self.pool)
            .await?,
        };
        let make_breakdowns = |rows: Vec<sqlx::sqlite::SqliteRow>, denominator: u64| {
            rows.into_iter()
                .map(|row| {
                    let input = row.get::<i64, _>("input_tokens") as u64;
                    let output = row.get::<i64, _>("output_tokens") as u64;
                    let total = input + output;
                    UsageBreakdown {
                        name: row.get("name"),
                        request_count: row.get::<i64, _>("request_count") as u64,
                        input_tokens: input,
                        output_tokens: output,
                        total_tokens: total,
                        failed_requests: row.get::<i64, _>("failed_requests") as u64,
                        token_share: if denominator == 0 {
                            0.0
                        } else {
                            total as f64 / denominator as f64
                        },
                    }
                })
                .collect::<Vec<_>>()
        };
        let today_tokens = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(input_tokens + output_tokens),0) FROM usage_events WHERE day = date('now','localtime')",
        )
        .fetch_one(&self.pool)
        .await? as u64;
        Ok(UsageDashboardSnapshot {
            range,
            request_count,
            input_tokens,
            output_tokens,
            total_tokens,
            today_tokens,
            selected_range_tokens: total_tokens,
            failed_requests,
            failure_rate: (request_count > 0)
                .then(|| failed_requests as f64 / request_count as f64),
            cache_hit_rate,
            sampled_at: chrono_like_now(),
            models: make_breakdowns(model_rows, total_tokens),
            providers: make_breakdowns(provider_rows, total_tokens),
            trend,
        })
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
                    provider_name: route.provider_name,
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

fn chrono_like_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

/// Best-effort entry channel when `providers.entry_category` is unset (legacy rows).
/// Does not write back — only used for Overview badges.
///
/// Three user-facing categories:
/// - `official` — browser OAuth login
/// - `json` — imported credentials/config JSON file
/// - `api` — form-filled API key
pub(crate) fn infer_entry_category(
    kind: &str,
    base_url: Option<&str>,
    credential_count: u32,
    api_key_count: u32,
    oauth_count: u32,
) -> Option<String> {
    let base = base_url.unwrap_or("");
    let has_chatgpt = base.contains("chatgpt.com");
    let has_api_openai = base.contains("api.openai.com");
    let has_api_xai = base.contains("api.x.ai");
    let has_cli_xai = base.contains("cli-chat-proxy.grok.com");

    // Pure API-key credentials → API (form key path; cannot distinguish JSON config
    // without a stored stamp — writers set `json` for file imports).
    if credential_count > 0 && api_key_count > 0 && oauth_count == 0 {
        return Some("api".into());
    }
    if has_api_openai
        || (kind == "xai" && has_api_xai && !has_cli_xai && oauth_count == 0 && api_key_count > 0)
    {
        return Some("api".into());
    }
    // Multi-account oauth/session without stored stamp → almost always a JSON import.
    if oauth_count >= 2 || (oauth_count >= 1 && credential_count >= 2 && api_key_count == 0) {
        return Some("json".into());
    }
    // Single oauth + ChatGPT backend is the common browser-login footprint.
    // Single oauth without chatgpt base is treated as JSON (file import of tokens).
    if has_chatgpt && oauth_count == 1 && credential_count <= 1 && api_key_count == 0 {
        return Some("official".into());
    }
    if kind == "xai"
        && oauth_count == 1
        && credential_count <= 1
        && api_key_count == 0
        && (has_api_xai || has_cli_xai || base.is_empty())
    {
        return Some("official".into());
    }
    if oauth_count >= 1 && api_key_count == 0 {
        return Some("json".into());
    }
    // Configured API-style base without chatgpt.
    if !base.is_empty() && !has_chatgpt && (api_key_count > 0 || credential_count > 0) {
        return Some("api".into());
    }
    // Non-OpenAI/xAI with any credentials → API by default.
    if kind != "openai" && kind != "xai" && credential_count > 0 {
        return Some("api".into());
    }
    None
}

#[cfg(test)]
mod entry_category_tests {
    use super::infer_entry_category;

    #[test]
    fn kimi_api_key_is_api() {
        assert_eq!(
            infer_entry_category("kimi", Some("https://api.kimi.com"), 1, 1, 0).as_deref(),
            Some("api")
        );
    }

    #[test]
    fn api_key_only_is_api() {
        assert_eq!(
            infer_entry_category("openai", Some("https://api.openai.com/v1"), 1, 1, 0).as_deref(),
            Some("api")
        );
    }

    #[test]
    fn single_oauth_chatgpt_is_official() {
        assert_eq!(
            infer_entry_category(
                "openai",
                Some("https://chatgpt.com/backend-api/codex"),
                1,
                0,
                1
            )
            .as_deref(),
            Some("official")
        );
    }

    #[test]
    fn multi_oauth_is_json() {
        assert_eq!(
            infer_entry_category(
                "openai",
                Some("https://chatgpt.com/backend-api/codex"),
                3,
                0,
                3
            )
            .as_deref(),
            Some("json")
        );
    }

    #[test]
    fn xai_oauth_is_official() {
        assert_eq!(
            infer_entry_category("xai", Some("https://api.x.ai/v1"), 1, 0, 1).as_deref(),
            Some("official")
        );
        assert_eq!(
            infer_entry_category("xai", Some("https://cli-chat-proxy.grok.com/v1"), 1, 0, 1)
                .as_deref(),
            Some("official")
        );
    }

    #[test]
    fn empty_unconfigured_is_none() {
        assert_eq!(infer_entry_category("openai", None, 0, 0, 0), None);
    }
}

#[cfg(test)]
mod usage_dashboard_tests {
    use super::{Storage, UsageDelta};
    use crate::domain::UsageRange;
    use sqlx::Row;
    use std::time::{SystemTime, UNIX_EPOCH};

    async fn open_temp_storage() -> Storage {
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-usage-{}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        Storage::open(&dir).await.expect("open storage")
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_usage_day(
        storage: &Storage,
        day_offset: i64,
        provider_id: &str,
        model_id: &str,
        request_count: i64,
        input_tokens: i64,
        output_tokens: i64,
        failed_requests: i64,
        cache_observations: i64,
        cache_hits: i64,
    ) {
        let day_modifier = format!("{day_offset} day");
        sqlx::query(
            "INSERT INTO usage_events (
                day, provider_id, model_id, request_count, input_tokens, output_tokens,
                cache_observations, cache_hits, failed_requests
             ) VALUES (
                date('now','localtime', ?), ?, ?, ?, ?, ?, ?, ?, ?
             )
             ON CONFLICT(day, provider_id, model_id) DO UPDATE SET
                request_count = excluded.request_count,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_observations = excluded.cache_observations,
                cache_hits = excluded.cache_hits,
                failed_requests = excluded.failed_requests",
        )
        .bind(day_modifier)
        .bind(provider_id)
        .bind(model_id)
        .bind(request_count)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(cache_observations)
        .bind(cache_hits)
        .bind(failed_requests)
        .execute(&storage.pool)
        .await
        .expect("insert usage day");
    }

    async fn insert_provider(storage: &Storage, id: &str, name: &str) {
        sqlx::query(
            "INSERT INTO providers (id, name, region, protocol, base_url, configured, selected_models, discovered_models, kind)
             VALUES (?, ?, 'global', 'openai-compatible', 'https://example.test', 1, 0, 0, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(id)
        .execute(&storage.pool)
        .await
        .expect("insert provider");
    }

    #[tokio::test]
    async fn empty_database_returns_renderable_empty_dashboard() {
        let storage = open_temp_storage().await;
        let dash = storage
            .usage_dashboard(UsageRange::SevenDays)
            .await
            .expect("dashboard");
        assert_eq!(dash.request_count, 0);
        assert_eq!(dash.total_tokens, 0);
        assert!(dash.failure_rate.is_none());
        assert!(dash.cache_hit_rate.is_none());
        assert!(dash.trend.is_empty());
        assert!(dash.models.is_empty());
        assert!(dash.providers.is_empty());
    }

    #[tokio::test]
    async fn seven_and_thirty_day_ranges_filter_and_pad_days() {
        let storage = open_temp_storage().await;
        // Outside 30d window
        insert_usage_day(&storage, -40, "p1", "m1", 1, 100, 50, 0, 0, 0).await;
        // Inside 30d, outside 7d
        insert_usage_day(&storage, -10, "p1", "m1", 2, 200, 100, 1, 0, 0).await;
        // Inside 7d
        insert_usage_day(&storage, -2, "p1", "m1", 3, 300, 150, 0, 0, 0).await;
        insert_usage_day(&storage, 0, "p1", "m1", 4, 400, 200, 0, 0, 0).await;

        let seven = storage
            .usage_dashboard(UsageRange::SevenDays)
            .await
            .expect("7d");
        assert_eq!(seven.trend.len(), 7);
        assert_eq!(seven.request_count, 7); // 3 + 4
        assert_eq!(seven.input_tokens, 700);
        assert_eq!(seven.output_tokens, 350);
        assert_eq!(seven.total_tokens, 1050);
        assert_eq!(seven.selected_range_tokens, 1050);
        // Missing days padded with zeros
        assert!(seven.trend.iter().any(|p| p.request_count == 0));
        assert!(seven.trend.iter().all(|p| !p.day.is_empty()));

        let thirty = storage
            .usage_dashboard(UsageRange::ThirtyDays)
            .await
            .expect("30d");
        assert_eq!(thirty.trend.len(), 30);
        assert_eq!(thirty.request_count, 9); // 2+3+4
        assert_eq!(thirty.total_tokens, 1350);

        let all = storage.usage_dashboard(UsageRange::All).await.expect("all");
        assert_eq!(all.request_count, 10);
        assert_eq!(all.total_tokens, 1500);
        assert!(!all.trend.is_empty());
        // Continuous day series from earliest to today
        assert!(all.trend.len() >= 41);
    }

    #[tokio::test]
    async fn models_and_providers_sorted_by_total_tokens() {
        let storage = open_temp_storage().await;
        insert_provider(&storage, "alpha", "Alpha Provider").await;
        insert_provider(&storage, "beta", "Beta Provider").await;
        insert_usage_day(&storage, 0, "alpha", "gpt-small", 2, 100, 50, 0, 0, 0).await;
        insert_usage_day(&storage, 0, "alpha", "gpt-large", 1, 500, 500, 1, 0, 0).await;
        insert_usage_day(&storage, 0, "beta", "gpt-mid", 3, 200, 100, 0, 0, 0).await;

        let dash = storage
            .usage_dashboard(UsageRange::SevenDays)
            .await
            .expect("dash");
        assert_eq!(
            dash.models
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-large", "gpt-mid", "gpt-small"]
        );
        assert!(dash.models[0].total_tokens >= dash.models[1].total_tokens);
        assert!((dash.models.iter().map(|m| m.token_share).sum::<f64>() - 1.0).abs() < 1e-9);

        assert_eq!(dash.providers[0].name, "Alpha Provider");
        assert_eq!(dash.providers[1].name, "Beta Provider");
        assert_eq!(dash.providers[0].total_tokens, 1150);
        assert_eq!(dash.providers[1].total_tokens, 300);
    }

    #[tokio::test]
    async fn failure_and_cache_rates_handle_zero_and_samples() {
        let storage = open_temp_storage().await;
        // No requests path already covered; insert successful requests without cache samples
        insert_usage_day(&storage, 0, "p1", "m1", 4, 40, 20, 1, 0, 0).await;
        let dash = storage
            .usage_dashboard(UsageRange::SevenDays)
            .await
            .expect("dash");
        assert!((dash.failure_rate.unwrap() - 0.25).abs() < 1e-9);
        assert!(dash.cache_hit_rate.is_none());
        assert_eq!(dash.failed_requests, 1);

        insert_usage_day(&storage, 0, "p1", "m1", 4, 40, 20, 1, 10, 4).await;
        let with_cache = storage
            .usage_dashboard(UsageRange::SevenDays)
            .await
            .expect("cache");
        assert!((with_cache.cache_hit_rate.unwrap() - 0.4).abs() < 1e-9);
        assert!(with_cache.trend.iter().any(|p| p.cache_hit_rate.is_some()));
    }

    #[tokio::test]
    async fn deleted_provider_uses_stable_fallback_name() {
        let storage = open_temp_storage().await;
        // No row in providers table
        insert_usage_day(&storage, 0, "gone-id", "m1", 1, 10, 5, 0, 0, 0).await;
        let dash = storage
            .usage_dashboard(UsageRange::All)
            .await
            .expect("dash");
        assert_eq!(dash.providers.len(), 1);
        assert_eq!(dash.providers[0].name, "已删除供应商 · gone-id");
    }

    #[tokio::test]
    async fn record_usage_aggregates_into_snapshot_and_dashboard() {
        let storage = open_temp_storage().await;
        storage
            .record_usage(
                "p1",
                "m1",
                &UsageDelta {
                    request_count: 2,
                    input_tokens: 30,
                    output_tokens: 10,
                    cache_observations: 2,
                    cache_hits: 1,
                    failed_requests: 0,
                },
            )
            .await
            .expect("record");
        storage
            .record_usage(
                "p1",
                "m1",
                &UsageDelta {
                    request_count: 1,
                    input_tokens: 5,
                    output_tokens: 5,
                    cache_observations: 0,
                    cache_hits: 0,
                    failed_requests: 1,
                },
            )
            .await
            .expect("record2");

        let snap = storage.usage_snapshot().await.expect("snapshot");
        assert_eq!(snap.request_count, 3);
        assert_eq!(snap.input_tokens, 35);
        assert_eq!(snap.output_tokens, 15);
        assert_eq!(snap.total_tokens, 50);
        assert_eq!(snap.failed_requests, 1);
        assert!((snap.cache_hit_rate.unwrap() - 0.5).abs() < 1e-9);

        let dash = storage
            .usage_dashboard(UsageRange::SevenDays)
            .await
            .expect("dash");
        assert_eq!(dash.total_tokens, 50);
        assert_eq!(dash.today_tokens, 50);
        assert_eq!(dash.trend.len(), 7);
    }

    #[tokio::test]
    async fn local_date_helper_matches_sqlite() {
        // Sanity: SQLite local date expression used by range filters is queryable
        let storage = open_temp_storage().await;
        let day: String = sqlx::query_scalar("SELECT date('now','localtime')")
            .fetch_one(&storage.pool)
            .await
            .expect("day");
        assert_eq!(day.len(), 10);
        let row = sqlx::query("SELECT date('now','localtime', '-6 day') as start")
            .fetch_one(&storage.pool)
            .await
            .expect("start");
        let start: String = row.get("start");
        assert_eq!(start.len(), 10);
    }
}

#[cfg(test)]
mod delete_credential_tests {
    use super::Storage;
    use std::time::{SystemTime, UNIX_EPOCH};

    async fn open_temp_storage() -> Storage {
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-delete-cred-{}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        Storage::open(&dir).await.expect("open storage")
    }

    async fn insert_provider(storage: &Storage, id: &str, name: &str) {
        sqlx::query(
            "INSERT INTO providers (id, name, region, protocol, base_url, configured, selected_models, discovered_models, kind, routing_mode)
             VALUES (?, ?, 'global', 'openai-compatible', 'https://example.test', 1, 0, 0, ?, 'pool')",
        )
        .bind(id)
        .bind(name)
        .bind(id)
        .execute(&storage.pool)
        .await
        .expect("insert provider");
    }

    async fn insert_credential_row(
        storage: &Storage,
        id: &str,
        provider_id: &str,
        fingerprint: &str,
    ) {
        sqlx::query(
            "INSERT INTO credentials (
                id, provider_id, kind, state, label, email, account_id, expires_at,
                fingerprint, refreshable, secret_envelope_json
             ) VALUES (?, ?, 'api_key', 'ready', 'acct', NULL, NULL, NULL, ?, 0, '{}')",
        )
        .bind(id)
        .bind(provider_id)
        .bind(fingerprint)
        .execute(&storage.pool)
        .await
        .expect("insert credential");
    }

    #[tokio::test]
    async fn delete_last_credential_clears_fixed_routing_and_quota() {
        let storage = open_temp_storage().await;
        insert_provider(&storage, "p1", "Provider 1").await;
        insert_credential_row(&storage, "c1", "p1", "fp-c1").await;
        let pool_id = storage.ensure_default_pool("p1").await.expect("pool");
        storage
            .add_pool_member(&pool_id, "c1")
            .await
            .expect("member");
        storage
            .set_provider_routing("p1", "fixed", Some("c1"))
            .await
            .expect("fixed");
        sqlx::query(
            "INSERT INTO app_settings (key, value_json) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        )
        .bind("quota:c1")
        .bind("{\"credentialId\":\"c1\"}")
        .execute(&storage.pool)
        .await
        .expect("quota key");

        let result = storage.delete_credential("c1").await.expect("delete");
        assert_eq!(result.provider_id, "p1");
        assert_eq!(result.remaining_accounts, 0);

        let creds = storage.list_credentials(Some("p1")).await.expect("list");
        assert!(creds.is_empty());

        let members = storage
            .list_pool_member_ids(&pool_id)
            .await
            .expect("members");
        assert!(members.is_empty());

        let routing = storage
            .get_provider_routing("p1")
            .await
            .expect("routing")
            .expect("present");
        assert_eq!(routing.routing_mode, "pool");
        assert!(routing.fixed_credential_id.is_none());

        let quota_left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM app_settings WHERE key = 'quota:c1'")
                .fetch_one(&storage.pool)
                .await
                .expect("quota count");
        assert_eq!(quota_left, 0);
    }

    #[tokio::test]
    async fn delete_one_of_many_keeps_sibling_and_returns_remaining() {
        let storage = open_temp_storage().await;
        insert_provider(&storage, "p1", "Provider 1").await;
        insert_credential_row(&storage, "c1", "p1", "fp-c1").await;
        insert_credential_row(&storage, "c2", "p1", "fp-c2").await;
        let pool_id = storage.ensure_default_pool("p1").await.expect("pool");
        storage.add_pool_member(&pool_id, "c1").await.expect("m1");
        storage.add_pool_member(&pool_id, "c2").await.expect("m2");
        storage
            .set_provider_routing("p1", "fixed", Some("c1"))
            .await
            .expect("fixed");

        let result = storage.delete_credential("c1").await.expect("delete");
        assert_eq!(result.remaining_accounts, 1);

        let creds = storage.list_credentials(Some("p1")).await.expect("list");
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].id, "c2");

        let members = storage
            .list_pool_member_ids(&pool_id)
            .await
            .expect("members");
        assert_eq!(members, vec!["c2".to_string()]);

        let routing = storage
            .get_provider_routing("p1")
            .await
            .expect("routing")
            .expect("present");
        assert_eq!(routing.routing_mode, "pool");
        assert!(routing.fixed_credential_id.is_none());
    }

    #[tokio::test]
    async fn delete_missing_credential_errors() {
        let storage = open_temp_storage().await;
        let err = storage
            .delete_credential("missing")
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("账号不存在"));
    }

    #[tokio::test]
    async fn delete_provider_cascades_credentials_pools_and_routes() {
        let storage = open_temp_storage().await;
        insert_provider(&storage, "p1", "Provider 1").await;
        insert_credential_row(&storage, "c1", "p1", "fp-c1").await;
        let pool_id = storage.ensure_default_pool("p1").await.expect("pool");
        storage
            .add_pool_member(&pool_id, "c1")
            .await
            .expect("member");
        sqlx::query(
            "INSERT INTO model_routes (id, provider_id, upstream_model, display_name, enabled, catalog_json)
             VALUES ('r1', 'p1', 'm1', 'Model 1', 1, '{}')",
        )
        .execute(&storage.pool)
        .await
        .expect("route");

        storage
            .delete_provider_instance("p1")
            .await
            .expect("delete provider");

        let providers = storage.list_providers().await.expect("providers");
        assert!(!providers.iter().any(|p| p.id == "p1"));
        let creds = storage.list_credentials(Some("p1")).await.expect("creds");
        assert!(creds.is_empty());
        let pools: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM account_pools WHERE provider_id = 'p1'")
                .fetch_one(&storage.pool)
                .await
                .expect("pools");
        assert_eq!(pools, 0);
        let routes: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM model_routes WHERE provider_id = 'p1'")
                .fetch_one(&storage.pool)
                .await
                .expect("routes");
        assert_eq!(routes, 0);
    }

    #[tokio::test]
    async fn delete_missing_provider_errors() {
        let storage = open_temp_storage().await;
        let err = storage
            .delete_provider_instance("missing")
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("供应商不存在"));
    }
}
