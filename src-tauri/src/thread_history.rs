//! Read-only Codex rollout health inspection.
//!
//! This module intentionally opens only Codex Desktop's thread index and only
//! selects `threads.rollout_path`. It never accesses Spur's credential store or
//! returns message text, tool arguments, command lines, search queries, or tool
//! outputs.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
};

use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row,
};

use crate::domain::{ThreadHistoryHealthReport, ThreadHistoryTimelineEntry};

const TIMELINE_LIMIT: usize = 500;

pub async fn inspect(thread_id: &str) -> Result<ThreadHistoryHealthReport, String> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err("请输入 Codex task ID。".into());
    }

    let index_path = crate::codex_config::publish_codex_home().join("state_5.sqlite");
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", index_path.display()))
        .map_err(|error| format!("无法打开 Codex 线程索引：{error}"))?
        .read_only(true)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|error| format!("无法只读连接 Codex 线程索引：{error}"))?;
    let row = sqlx::query("SELECT rollout_path FROM threads WHERE id = ?")
        .bind(thread_id)
        .fetch_optional(&pool)
        .await
        .map_err(|error| format!("无法查询 Codex task：{error}"))?;
    pool.close().await;
    let Some(row) = row else {
        return Err("未找到这个 Codex task。".into());
    };
    let rollout_path = PathBuf::from(row.get::<String, _>("rollout_path"));
    let bytes = tokio::fs::read(&rollout_path)
        .await
        .map_err(|error| format!("无法读取 rollout（只读）：{error}"))?;
    inspect_rollout_bytes(thread_id, &rollout_path, &bytes)
}

fn inspect_rollout_bytes(
    thread_id: &str,
    rollout_path: &Path,
    bytes: &[u8],
) -> Result<ThreadHistoryHealthReport, String> {
    let mut report = ThreadHistoryHealthReport {
        thread_id: thread_id.to_string(),
        rollout_path: rollout_path.display().to_string(),
        rollout_bytes: bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(bytes)),
        parsed_rows: 0,
        invalid_complete_lines: 0,
        trailing_partial_line: false,
        compaction_events: 0,
        turn_starts: 0,
        turn_finishes: 0,
        active_turns: 0,
        tool_calls: 0,
        tool_outputs: 0,
        missing_outputs: 0,
        orphan_outputs: 0,
        duplicate_call_ids: 0,
        duplicate_output_call_ids: 0,
        duplicate_response_item_ids: 0,
        missing_call_ids: 0,
        timeline: Vec::new(),
        timeline_truncated: false,
    };
    let mut calls = HashSet::new();
    let mut outputs = HashSet::new();
    let mut response_item_ids = HashSet::new();
    let mut call_counts = HashMap::<String, u64>::new();
    let mut output_counts = HashMap::<String, u64>::new();

    let lines: Vec<&[u8]> = bytes.split_inclusive(|byte| *byte == b'\n').collect();
    for (index, raw_line) in lines.iter().enumerate() {
        let is_last = index + 1 == lines.len();
        let line = raw_line.strip_suffix(b"\n").unwrap_or(raw_line);
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_slice(line) {
            Ok(entry) => entry,
            Err(_) if is_last && !bytes.ends_with(b"\n") => {
                report.trailing_partial_line = true;
                continue;
            }
            Err(_) => {
                report.invalid_complete_lines += 1;
                continue;
            }
        };
        report.parsed_rows += 1;
        let timestamp = entry
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let entry_type = entry
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let payload = entry.get("payload").unwrap_or(&serde_json::Value::Null);
        match entry_type {
            "event_msg" => match payload.get("type").and_then(serde_json::Value::as_str) {
                Some("task_started") | Some("turn_started") => report.turn_starts += 1,
                Some("task_complete") | Some("turn_complete") | Some("turn_aborted") => {
                    report.turn_finishes += 1
                }
                _ => {}
            },
            "response_item" => inspect_response_item(
                &mut report,
                payload,
                &timestamp,
                &mut calls,
                &mut outputs,
                &mut response_item_ids,
                &mut call_counts,
                &mut output_counts,
            ),
            _ => {}
        }
    }
    report.active_turns = report.turn_starts.saturating_sub(report.turn_finishes);
    report.missing_outputs = calls.difference(&outputs).count() as u64;
    report.orphan_outputs = outputs.difference(&calls).count() as u64;
    report.duplicate_call_ids = call_counts.values().filter(|count| **count > 1).count() as u64;
    report.duplicate_output_call_ids =
        output_counts.values().filter(|count| **count > 1).count() as u64;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn inspect_response_item(
    report: &mut ThreadHistoryHealthReport,
    payload: &serde_json::Value,
    timestamp: &str,
    calls: &mut HashSet<String>,
    outputs: &mut HashSet<String>,
    response_item_ids: &mut HashSet<String>,
    call_counts: &mut HashMap<String, u64>,
    output_counts: &mut HashMap<String, u64>,
) {
    let kind = payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if let Some(id) = payload
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
    {
        if !response_item_ids.insert(id.to_string()) {
            report.duplicate_response_item_ids += 1;
        }
    }
    if kind == "compaction" {
        report.compaction_events += 1;
        return;
    }
    if kind == "web_search_call" {
        push_timeline(
            report,
            timestamp,
            "web_search",
            "web_search",
            payload.get("status").and_then(serde_json::Value::as_str),
        );
        return;
    }
    let is_call = matches!(kind, "function_call" | "custom_tool_call");
    let is_output = matches!(kind, "function_call_output" | "custom_tool_call_output");
    if !is_call && !is_output {
        return;
    }
    let Some(call_id) = payload
        .get("call_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        report.missing_call_ids += 1;
        return;
    };
    if is_call {
        report.tool_calls += 1;
        calls.insert(call_id.to_string());
        *call_counts.entry(call_id.to_string()).or_default() += 1;
        let name = payload
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown_tool");
        let category = if name == "apply_patch" {
            "file_change"
        } else if name.contains("exec") || name.contains("command") {
            "command"
        } else {
            "tool"
        };
        push_timeline(
            report,
            timestamp,
            category,
            name,
            payload.get("status").and_then(serde_json::Value::as_str),
        );
    } else {
        report.tool_outputs += 1;
        outputs.insert(call_id.to_string());
        *output_counts.entry(call_id.to_string()).or_default() += 1;
    }
}

fn push_timeline(
    report: &mut ThreadHistoryHealthReport,
    timestamp: &str,
    category: &str,
    tool_name: &str,
    status: Option<&str>,
) {
    if report.timeline.len() >= TIMELINE_LIMIT {
        report.timeline_truncated = true;
        return;
    }
    report.timeline.push(ThreadHistoryTimelineEntry {
        timestamp: timestamp.to_string(),
        category: category.to_string(),
        tool_name: tool_name.to_string(),
        status: status.map(ToOwned::to_owned),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_rollout_reports_paired_calls_and_compaction_without_deletion() {
        let data = br#"{"timestamp":"a","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"b","type":"response_item","payload":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"exec_command"}}
{"timestamp":"c","type":"response_item","payload":{"type":"function_call_output","id":"fco_1","call_id":"call_1"}}
{"timestamp":"d","type":"response_item","payload":{"type":"custom_tool_call","id":"ctc_1","call_id":"call_2","name":"apply_patch"}}
{"timestamp":"e","type":"response_item","payload":{"type":"custom_tool_call_output","id":"ctco_1","call_id":"call_2"}}
{"timestamp":"f","type":"response_item","payload":{"type":"web_search_call","id":"ws_1","status":"completed"}}
{"timestamp":"g","type":"response_item","payload":{"type":"compaction","id":"cmp_1"}}
"#;
        let report =
            inspect_rollout_bytes("thread", Path::new("/tmp/rollout.jsonl"), data).unwrap();
        assert_eq!(report.tool_calls, 2);
        assert_eq!(report.tool_outputs, 2);
        assert_eq!(report.missing_outputs, 0);
        assert_eq!(report.orphan_outputs, 0);
        assert_eq!(report.compaction_events, 1);
        assert_eq!(report.invalid_complete_lines, 0);
        assert_eq!(report.timeline.len(), 3);
    }

    #[test]
    fn trailing_partial_line_is_active_write_not_corruption() {
        let data = b"{\"timestamp\":\"a\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n{\"unfinished\":";
        let report =
            inspect_rollout_bytes("thread", Path::new("/tmp/rollout.jsonl"), data).unwrap();
        assert_eq!(report.parsed_rows, 1);
        assert!(report.trailing_partial_line);
        assert_eq!(report.invalid_complete_lines, 0);
    }
}
