use std::{
    env, fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::domain::{ApplyPreview, ModelsResponse};

pub fn codex_home() -> PathBuf {
    if let Some(value) = env::var_os("CODEX_HOME") {
        return PathBuf::from(value);
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".codex"))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

pub fn preview(base_url: &str, model_count: u32) -> ApplyPreview {
    let home = codex_home();
    let catalog_path = home.join("codex-select").join("model-catalog.json");
    let config_path = home.join("config.toml");
    let toml_preview = format!(
        r#"model_provider = "codex_select"
model_catalog_json = "{}"

[model_providers.codex_select]
name = "Codex Spur"
base_url = "{}"
wire_api = "responses"
requires_openai_auth = false"#,
        catalog_path.display(),
        base_url,
    );
    let mut warnings = Vec::new();
    if model_count == 0 {
        warnings.push("当前没有已选择模型，应用会被阻止。".into());
    }
    if !config_path.exists() {
        warnings.push(format!("尚未找到 Codex 配置：{}", config_path.display()));
    }
    ApplyPreview {
        provider_id: "codex_select".into(),
        base_url: base_url.into(),
        catalog_path: catalog_path.display().to_string(),
        selected_model: None,
        model_count,
        toml_preview,
        warnings,
    }
}

#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub catalog_path: PathBuf,
    pub config_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub before_hash: Option<String>,
    pub after_hash: String,
}

pub fn apply(base_url: &str, bearer_token: &str, catalog: &ModelsResponse) -> Result<ApplyResult> {
    if catalog.models.is_empty() {
        anyhow::bail!("至少选择一个模型后才能应用到 Codex");
    }
    let home = codex_home();
    let select_dir = home.join("codex-select");
    let backup_dir = select_dir.join("backups");
    fs::create_dir_all(&backup_dir).context("无法创建 Codex Spur 配置目录")?;
    let catalog_path = select_dir.join("model-catalog.json");
    let config_path = home.join("config.toml");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let original_config = fs::read_to_string(&config_path).unwrap_or_default();
    let before_hash = if original_config.is_empty() {
        None
    } else {
        Some(hash_bytes(original_config.as_bytes()))
    };
    let backup_path = if config_path.exists() {
        let path = backup_dir.join(format!("config-{timestamp}.toml"));
        atomic_write(&path, original_config.as_bytes())?;
        Some(path)
    } else {
        None
    };

    let mut document = if original_config.trim().is_empty() {
        DocumentMut::new()
    } else {
        original_config
            .parse::<DocumentMut>()
            .context("Codex config.toml 不是有效 TOML，已停止应用")?
    };
    document["model_provider"] = value("codex_select");
    document["model_catalog_json"] = value(catalog_path.display().to_string());
    let providers = document["model_providers"]
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("model_providers 不是 TOML table")?;
    let mut select_provider = Table::new();
    select_provider["name"] = value("Codex Spur");
    select_provider["base_url"] = value(base_url);
    select_provider["wire_api"] = value("responses");
    select_provider["requires_openai_auth"] = value(false);
    select_provider["experimental_bearer_token"] = value(bearer_token);
    providers["codex_select"] = Item::Table(select_provider);

    let catalog_json = serde_json::to_vec_pretty(catalog)?;
    atomic_write(&catalog_path, &catalog_json)?;
    atomic_write(&config_path, document.to_string().as_bytes())?;
    let after_bytes = fs::read(&config_path).context("读取应用后的 Codex 配置失败")?;
    Ok(ApplyResult {
        catalog_path,
        config_path,
        backup_path,
        before_hash,
        after_hash: hash_bytes(&after_bytes),
    })
}

pub fn restore_latest() -> Result<Option<PathBuf>> {
    let backup_dir = codex_home().join("codex-select").join("backups");
    let mut backups = fs::read_dir(&backup_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .collect::<Vec<_>>();
    backups.sort();
    let Some(latest) = backups.pop() else {
        return Ok(None);
    };
    let config_path = codex_home().join("config.toml");
    let bytes = fs::read(&latest).context("读取 Codex 备份失败")?;
    atomic_write(&config_path, &bytes)?;
    Ok(Some(latest))
}

fn atomic_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp, bytes).with_context(|| format!("写入临时文件失败：{}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("原子替换文件失败：{}", path.display()))?;
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
