//! Official Desktop tool round-trip registry.
//!
//! # Contract
//!
//! - **Outbound** (Desktop → upstream): may adapt tools so third-party hosts accept them
//!   (e.g. freeform `apply_patch` / `exec` → Chat Completions `function`).
//! - **Inbound** (upstream → Desktop): **must restore official Desktop item types**.
//!   Never leave freeform tools as plain `function_call` or Desktop aborts them.
//!
//! Desktop call shapes observed in local rollouts (396 files):
//! - `custom_tool_call` + freeform `input`: `apply_patch`, `exec`
//! - `function_call` + JSON `arguments`: everything else (shell, plan, MCP, computer-use, …)
//!
//! Gold sample (native successful apply_patch):
//! ```json
//! {
//!   "type": "custom_tool_call",
//!   "id": "ctc_…",
//!   "status": "completed",
//!   "call_id": "call_…",
//!   "name": "apply_patch",
//!   "input": "*** Begin Patch\\n…"
//! }
//! ```

use serde_json::{json, Value};

pub const APPLY_PATCH_TOOL_NAME: &str = "apply_patch";
pub const EXEC_FREEFORM_TOOL_NAME: &str = "exec";

/// How Desktop records a tool invocation in Responses `output[]` / rollout items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopCallType {
    /// Standard Chat/Responses function call (`exec_command`, MCP, …).
    FunctionCall,
    /// Freeform / custom tool (`apply_patch`, legacy `exec`).
    CustomToolCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolProfile {
    pub name: &'static str,
    pub desktop_call: DesktopCallType,
    /// When true, Desktop freeform payload lives in `input` (not JSON `arguments`).
    pub freeform_input: bool,
}

macro_rules! fn_tool {
    ($name:expr) => {
        ToolProfile {
            name: $name,
            desktop_call: DesktopCallType::FunctionCall,
            freeform_input: false,
        }
    };
}

macro_rules! freeform_tool {
    ($name:expr) => {
        ToolProfile {
            name: $name,
            desktop_call: DesktopCallType::CustomToolCall,
            freeform_input: true,
        }
    };
}

/// Complete registry from local rollout inventory + Codex App dynamic_tools.
///
/// Freeform set must stay exactly `{apply_patch, exec}` until a new gold sample
/// proves another custom_tool_call name.
const PROFILES: &[ToolProfile] = &[
    // ── Batch 0/1: freeform custom_tool_call ───────────────────────────
    freeform_tool!(APPLY_PATCH_TOOL_NAME),
    freeform_tool!(EXEC_FREEFORM_TOOL_NAME),
    // ── Batch 2: core shell / interaction ──────────────────────────────
    fn_tool!("exec_command"),
    fn_tool!("write_stdin"),
    fn_tool!("wait"),
    fn_tool!("request_user_input"),
    fn_tool!("view_image"),
    // ── Batch 3: plan / goal / multi-agent ─────────────────────────────
    fn_tool!("update_plan"),
    fn_tool!("update_goal"),
    fn_tool!("get_goal"),
    fn_tool!("create_goal"),
    fn_tool!("spawn_agent"),
    fn_tool!("wait_agent"),
    fn_tool!("list_agents"),
    fn_tool!("close_agent"),
    fn_tool!("interrupt_agent"),
    fn_tool!("resume_agent"),
    fn_tool!("followup_task"),
    // ── Batch 4: Codex App / thread tools ──────────────────────────────
    fn_tool!("automation_update"),
    fn_tool!("open_in_codex"),
    fn_tool!("navigate_to_codex_page"),
    fn_tool!("read_thread_terminal"),
    fn_tool!("load_workspace_dependencies"),
    fn_tool!("fork_thread"),
    fn_tool!("handoff_thread"),
    fn_tool!("get_handoff_status"),
    fn_tool!("list_projects"),
    fn_tool!("create_thread"),
    fn_tool!("list_threads"),
    fn_tool!("read_thread"),
    fn_tool!("wait_threads"),
    fn_tool!("send_message_to_thread"),
    fn_tool!("set_thread_pinned"),
    fn_tool!("set_thread_archived"),
    fn_tool!("set_thread_title"),
    fn_tool!("uninstall_plugin"),
    // ── Batch 5: computer-use / UI ─────────────────────────────────────
    fn_tool!("js"),
    fn_tool!("js_reset"),
    fn_tool!("js_add_node_module_dir"),
    fn_tool!("click"),
    fn_tool!("type_text"),
    fn_tool!("set_value"),
    fn_tool!("press_key"),
    fn_tool!("scroll"),
    fn_tool!("send_input"),
    fn_tool!("get_app_state"),
    fn_tool!("list_apps"),
    fn_tool!("send_message"),
    fn_tool!("perform_secondary_action"),
    fn_tool!("imagegen"),
    // ── Batch 6: common MCP / plugin names ─────────────────────────────
    fn_tool!("_fetch_file"),
    fn_tool!("_search"),
    fn_tool!("_search_repositories"),
    fn_tool!("_get_repo"),
    fn_tool!("_search_issues"),
    fn_tool!("_get_user_login"),
    fn_tool!("_get_profile"),
    fn_tool!("_fetch_issue_comments"),
    fn_tool!("_fetch_commit"),
    fn_tool!("_update_file"),
    fn_tool!("list_mcp_resources"),
    fn_tool!("read_mcp_resource"),
    fn_tool!("apply_script_edit"),
];

/// Official freeform Desktop tools (must restore `custom_tool_call` + `input`).
#[allow(dead_code)] // used by tests + inventory API
pub fn freeform_tool_names() -> &'static [&'static str] {
    &[APPLY_PATCH_TOOL_NAME, EXEC_FREEFORM_TOOL_NAME]
}

#[allow(dead_code)] // public inventory API for diagnostics / future callers
pub fn all_profiles() -> &'static [ToolProfile] {
    PROFILES
}

pub fn profile_for(name: &str) -> ToolProfile {
    PROFILES
        .iter()
        .copied()
        .find(|p| p.name == name)
        .unwrap_or(ToolProfile {
            name: "",
            desktop_call: DesktopCallType::FunctionCall,
            freeform_input: false,
        })
}

pub fn is_freeform_desktop_tool(name: &str) -> bool {
    let p = profile_for(name);
    p.desktop_call == DesktopCallType::CustomToolCall && p.freeform_input
}

/// Extract freeform `input` text from a Chat Completions tool-call `arguments` string.
///
/// Upstream (Kimi) typically sends: `{"input":"*** Begin Patch\\n…"}` or `{"input":"shell…"}`.
/// Also accepts raw freeform bodies.
pub fn extract_freeform_input(arguments: &str) -> String {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
        if let Some(input) = map.get("input").and_then(Value::as_str) {
            return input.to_string();
        }
        for key in ["patch", "content", "text", "cmd", "command"] {
            if let Some(input) = map.get(key).and_then(Value::as_str) {
                return input.to_string();
            }
        }
    }
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        if let Ok(Value::String(s)) = serde_json::from_str::<Value>(trimmed) {
            return s;
        }
    }
    trimmed.to_string()
}

/// Extract freeform input and normalize apply_patch dialect for Desktop.
pub fn extract_freeform_input_for_tool(name: &str, arguments: &str) -> String {
    let raw = extract_freeform_input(arguments);
    if name == APPLY_PATCH_TOOL_NAME {
        normalize_apply_patch_input(&raw)
    } else {
        raw
    }
}

/// Normalize third-party / Grok apply_patch freeform bodies to Desktop's verifier.
///
/// Codex Desktop rejects patches whose first line is not exactly `*** Begin Patch`
/// (no trailing ` ***`). Grok/xAI and some Chat bridges commonly emit:
/// - `*** Begin Patch ***` / `***Begin Patch***`
/// - path glued to stars: `…/file.ts***`
/// - invented markers: `*** End of File ***`
///
/// Inspired by field failures on spur-route Grok sessions (44× verification failed).
/// CC Switch routes Grok via Responses too but does not rewrite patch text; Spur
/// must, because multi-vendor models reuse the same Desktop freeform executor.
pub fn normalize_apply_patch_input(raw: &str) -> String {
    let mut text = raw.replace("\r\n", "\n").replace('\r', "\n");
    // Unwrap accidental JSON string envelopes again (double-encoded).
    if text.trim_start().starts_with('{') {
        text = extract_freeform_input(text.trim());
        text = text.replace("\r\n", "\n").replace('\r', "\n");
    }
    if text.trim().is_empty() {
        return String::new();
    }

    let mut out_lines: Vec<String> = Vec::new();
    let mut saw_begin = false;
    let mut saw_end = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() && out_lines.is_empty() {
            // Drop leading blank lines so "first line" is the begin marker.
            continue;
        }

        // Invented marker — drop.
        if is_end_of_file_marker(trimmed) {
            continue;
        }

        // Begin markers (with/without spaces/trailing stars).
        if is_begin_patch_marker(trimmed) {
            if !saw_begin {
                out_lines.push("*** Begin Patch".into());
                saw_begin = true;
            }
            continue;
        }

        // End markers.
        if is_end_patch_marker(trimmed) {
            if saw_begin && !saw_end {
                out_lines.push("*** End Patch".into());
                saw_end = true;
            }
            continue;
        }

        // File action lines: "*** Update File: path***" / "***Update File:path"
        if let Some(normalized) = normalize_file_action_line(trimmed) {
            out_lines.push(normalized);
            continue;
        }

        // Path glued to stars without a proper action prefix — leave as-is but
        // strip a trailing *** that Grok often appends after the path.
        if let Some(stripped) = trimmed.strip_suffix("***") {
            let s = stripped.trim_end();
            if s.contains('/') || s.ends_with(".ts") || s.ends_with(".tsx") || s.ends_with(".rs")
            {
                // Only if it looks like a bare path line under a file action — rare.
                out_lines.push(line.to_string());
                continue;
            }
        }

        out_lines.push(line.to_string());
    }

    // If body clearly looks like a patch but begin was missing, prepend.
    let joined_probe = out_lines.join("\n");
    if !saw_begin
        && (joined_probe.contains("*** Update File:")
            || joined_probe.contains("*** Add File:")
            || joined_probe.contains("*** Delete File:"))
    {
        out_lines.insert(0, "*** Begin Patch".into());
        saw_begin = true;
    }
    if saw_begin && !saw_end {
        out_lines.push("*** End Patch".into());
    }

    let mut result = out_lines.join("\n");
    if !result.is_empty() && !result.ends_with('\n') {
        // Desktop samples usually end with a trailing newline after End Patch.
        if result.ends_with("*** End Patch") {
            result.push('\n');
        }
    }
    result
}

fn is_begin_patch_marker(line: &str) -> bool {
    let compact: String = line
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '*')
        .collect();
    compact.eq_ignore_ascii_case("BeginPatch")
}

fn is_end_patch_marker(line: &str) -> bool {
    let compact: String = line
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '*')
        .collect();
    compact.eq_ignore_ascii_case("EndPatch")
}

fn is_end_of_file_marker(line: &str) -> bool {
    let compact: String = line
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '*')
        .collect();
    compact.eq_ignore_ascii_case("EndofFile") || compact.eq_ignore_ascii_case("EndOfFile")
}

fn normalize_file_action_line(line: &str) -> Option<String> {
    // Accept: "*** Update File: path", "***Update File:path***", etc.
    let stripped = line.trim().trim_matches('*').trim();
    let lower = stripped.to_ascii_lowercase();
    let action_label = if lower.starts_with("update file:") {
        "Update File"
    } else if lower.starts_with("add file:") {
        "Add File"
    } else if lower.starts_with("delete file:") {
        "Delete File"
    } else {
        return None;
    };
    let path = stripped
        .split_once(':')
        .map(|(_, p)| p.trim().trim_end_matches('*').trim())
        .unwrap_or("")
        .to_string();
    if path.is_empty() {
        return None;
    }
    Some(format!("*** {action_label}: {path}"))
}

/// Portable Responses-style function definition for a freeform Desktop tool (outbound).
pub fn freeform_as_function_tool(name: &str) -> Value {
    // Schema-only portable shape. Prefer Desktop's original description when
    // rewriting (see proxy freeform_tool_as_function). Do not invent product
    // policy text for freeform `exec` / unknown freeform names — empty string
    // when Desktop omitted description.
    let description = match name {
        APPLY_PATCH_TOOL_NAME => {
            // Exact Desktop dialect (first line must be `*** Begin Patch` with NO trailing stars).
            // Grok often emits `*** Begin Patch ***` which Desktop rejects in a retry loop.
            "Apply a freeform patch. First line MUST be exactly `*** Begin Patch` (no trailing stars). Then `*** Update File: path` or `*** Add File: path`, hunks with @@, and end with `*** End Patch`."
        }
        _ => "",
    };
    json!({
        "type": "function",
        "name": name,
        "description": description,
        "parameters": {
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "Freeform tool input body."
                }
            },
            "required": ["input"]
        }
    })
}

/// Portable function-shaped `apply_patch` (outbound helper kept for call sites).
#[allow(dead_code)]
pub fn apply_patch_as_function_tool() -> Value {
    freeform_as_function_tool(APPLY_PATCH_TOOL_NAME)
}

/// Nested Chat Completions tools[] row for apply_patch.
pub fn apply_patch_as_chat_function_tool() -> Value {
    freeform_as_chat_function_tool(APPLY_PATCH_TOOL_NAME)
}

/// Nested Chat Completions tools[] row for any freeform tool name.
pub fn freeform_as_chat_function_tool(name: &str) -> Value {
    let portable = freeform_as_function_tool(name);
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": portable.get("description").cloned().unwrap_or(Value::String(String::new())),
            "parameters": portable.get("parameters").cloned().unwrap_or(json!({
                "type": "object",
                "properties": { "input": { "type": "string" } },
                "required": ["input"]
            }))
        }
    })
}

/// Build a Desktop-facing Responses output item for one upstream tool call.
///
/// Freeform tools become `custom_tool_call` with freeform `input`.
/// Everything else stays `function_call` with JSON `arguments`.
pub fn desktop_tool_call_item(
    response_id: &str,
    index: usize,
    name: &str,
    call_id: &str,
    arguments: &str,
    status: &str,
) -> Value {
    let profile = profile_for(name);
    let id_stem = response_id.strip_prefix("resp_").unwrap_or(response_id);
    match profile.desktop_call {
        DesktopCallType::CustomToolCall if profile.freeform_input => {
            let input = extract_freeform_input_for_tool(name, arguments);
            let item_id = if call_id.starts_with("ctc_") {
                call_id.to_string()
            } else {
                format!("ctc_{id_stem}_{index}")
            };
            let desktop_call_id = if call_id.is_empty() {
                format!("call_{id_stem}_{index}")
            } else if call_id.starts_with("ctc_") {
                format!("call_{id_stem}_{index}")
            } else {
                call_id.to_string()
            };
            json!({
                "id": item_id,
                "type": "custom_tool_call",
                "status": status,
                "call_id": desktop_call_id,
                "name": name,
                "input": input
            })
        }
        _ => {
            let item_id = format!("fc_{id_stem}_{index}");
            let desktop_call_id = if call_id.is_empty() {
                format!("call_{id_stem}_{index}")
            } else {
                call_id.to_string()
            };
            json!({
                "id": item_id,
                "type": "function_call",
                "status": status,
                "call_id": desktop_call_id,
                "name": name,
                "arguments": if arguments.is_empty() { "{}" } else { arguments }
            })
        }
    }
}

/// Whether inbound SSE should use freeform custom lifecycle (not function_call_arguments.*).
pub fn uses_custom_tool_sse(name: &str) -> bool {
    is_freeform_desktop_tool(name)
}

/// Rewrite one Responses `output[]` item so freeform tools match official Desktop.
///
/// Third-party hosts (and portable outbound ads) emit freeform tools as
/// `function_call` + JSON `arguments`. Desktop freeform executors require
/// `custom_tool_call` + freeform `input` or they **abort**. Idempotent for
/// items already in the official shape.
pub fn restore_freeform_output_item(item: &Value) -> Value {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");

    // Already Desktop-shaped: still normalize apply_patch body (Grok dialect).
    if item_type == "custom_tool_call" && name == APPLY_PATCH_TOOL_NAME {
        let input = item.get("input").and_then(Value::as_str).unwrap_or("");
        let normalized = normalize_apply_patch_input(input);
        if normalized == input {
            return item.clone();
        }
        let mut out = item.clone();
        if let Some(obj) = out.as_object_mut() {
            obj.insert("input".into(), Value::String(normalized));
        }
        return out;
    }

    if item_type != "function_call" {
        return item.clone();
    }
    if !is_freeform_desktop_tool(name) {
        return item.clone();
    }
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("");
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    let input = extract_freeform_input_for_tool(name, arguments);
    // Keep upstream item id when present so SSE item_id continuity is preserved;
    // prefer ctc_ prefix for gold-sample readability when we must invent an id.
    let item_id = match item.get("id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => {
            if id.starts_with("ctc_") {
                id.to_string()
            } else if let Some(rest) = id.strip_prefix("fc_") {
                format!("ctc_{rest}")
            } else {
                format!("ctc_{id}")
            }
        }
        _ if !call_id.is_empty() => format!("ctc_{call_id}"),
        _ => format!("ctc_{name}"),
    };
    let desktop_call_id = if call_id.is_empty() {
        format!("call_{name}")
    } else if call_id.starts_with("ctc_") {
        format!("call_{name}")
    } else {
        call_id
    };
    json!({
        "id": item_id,
        "type": "custom_tool_call",
        "status": status,
        "call_id": desktop_call_id,
        "name": name,
        "input": input
    })
}

/// Restore freeform tools inside a Responses JSON body (`output` / nested `response`).
pub fn restore_freeform_in_responses_body(body: &mut Value) {
    restore_freeform_in_output_array(body.get_mut("output"));
    if let Some(response) = body.get_mut("response") {
        restore_freeform_in_output_array(response.get_mut("output"));
    }
}

fn restore_freeform_in_output_array(output: Option<&mut Value>) {
    let Some(Value::Array(items)) = output else {
        return;
    };
    for item in items.iter_mut() {
        *item = restore_freeform_output_item(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn extract_unwraps_json_input_wrapper() {
        let args = r#"{"input":"*** Begin Patch\n*** Update File: a.py\n*** End Patch"}"#;
        let input = extract_freeform_input(args);
        assert!(input.starts_with("*** Begin Patch"));
        assert!(input.contains("Update File: a.py"));
        assert!(!input.starts_with('{'));
    }

    #[test]
    fn extract_keeps_raw_begin_patch() {
        let raw = "*** Begin Patch\n*** End Patch";
        assert_eq!(extract_freeform_input(raw), raw);
    }

    #[test]
    fn normalize_strips_trailing_stars_on_begin_patch_like_grok() {
        // Session 019fb8d7: Desktop requires first line exactly `*** Begin Patch`.
        let dirty = "*** Begin Patch ***\n*** Update File: /tmp/a.ts***\n@@\n export const x = 1;\n+export const y = 2;\n*** End Patch ***\n*** End of File ***\n";
        let clean = normalize_apply_patch_input(dirty);
        let first = clean.lines().next().unwrap_or("");
        assert_eq!(first, "*** Begin Patch");
        assert!(clean.contains("*** Update File: /tmp/a.ts\n"));
        assert!(!clean.contains("index.ts***") && !clean.contains(".ts***"));
        assert!(clean.contains("*** End Patch"));
        assert!(!clean.to_ascii_lowercase().contains("end of file"));
    }

    #[test]
    fn normalize_begin_without_spaces() {
        let dirty = "***Begin Patch***\n***Update File: pkg/a.ts***\n@@\n-a\n+b\n***End Patch***\n";
        let clean = normalize_apply_patch_input(dirty);
        assert_eq!(clean.lines().next().unwrap_or(""), "*** Begin Patch");
        assert!(clean.contains("*** Update File: pkg/a.ts\n"));
    }

    #[test]
    fn extract_for_apply_patch_normalizes() {
        let args = r#"{"input":"*** Begin Patch ***\n*** Update File: a.py\n+hi\n*** End Patch ***"}"#;
        let input = extract_freeform_input_for_tool(APPLY_PATCH_TOOL_NAME, args);
        assert_eq!(input.lines().next().unwrap_or(""), "*** Begin Patch");
    }

    #[test]
    fn restore_custom_tool_call_normalizes_apply_patch_body() {
        let item = json!({
            "type": "custom_tool_call",
            "name": "apply_patch",
            "call_id": "call_1",
            "id": "ctc_1",
            "status": "completed",
            "input": "*** Begin Patch ***\n*** Update File: a.ts\n*** End Patch ***"
        });
        let restored = restore_freeform_output_item(&item);
        assert_eq!(
            restored.get("input").and_then(Value::as_str).unwrap().lines().next().unwrap(),
            "*** Begin Patch"
        );
    }

    #[test]
    fn apply_patch_restores_to_custom_tool_call_gold_shape() {
        let args = r#"{"input":"*** Begin Patch\n*** Add File: work/x.js\n+hi\n*** End Patch"}"#;
        let item = desktop_tool_call_item(
            "resp_test",
            0,
            "apply_patch",
            "tool_abc123",
            args,
            "completed",
        );
        assert_eq!(item["type"], "custom_tool_call");
        assert_eq!(item["name"], "apply_patch");
        assert_eq!(item["status"], "completed");
        assert_eq!(item["call_id"], "tool_abc123");
        let input = item["input"].as_str().expect("input string");
        assert!(input.starts_with("*** Begin Patch"));
        assert!(input.contains("Add File: work/x.js"));
        assert!(!input.trim_start().starts_with('{'));
    }

    #[test]
    fn exec_freeform_restores_to_custom_tool_call() {
        let item = desktop_tool_call_item(
            "resp_test",
            0,
            "exec",
            "tool_exec_1",
            r#"{"input":"ls -la"}"#,
            "completed",
        );
        assert_eq!(item["type"], "custom_tool_call");
        assert_eq!(item["name"], "exec");
        assert_eq!(item["input"], "ls -la");
        assert!(item.get("arguments").is_none());
    }

    #[test]
    fn exec_command_stays_function_call() {
        let item = desktop_tool_call_item(
            "resp_test",
            1,
            "exec_command",
            "call_shell_1",
            r#"{"cmd":"ls"}"#,
            "completed",
        );
        assert_eq!(item["type"], "function_call");
        assert_eq!(item["name"], "exec_command");
        assert_eq!(item["arguments"], r#"{"cmd":"ls"}"#);
        assert!(item.get("input").is_none());
    }

    #[test]
    fn core_function_tools_stay_function_call() {
        for name in [
            "write_stdin",
            "wait",
            "request_user_input",
            "view_image",
            "update_plan",
            "spawn_agent",
            "automation_update",
            "js",
            "click",
            "_fetch_file",
            "apply_script_edit",
        ] {
            let item = desktop_tool_call_item(
                "resp_x",
                0,
                name,
                "call_1",
                r#"{"x":1}"#,
                "completed",
            );
            assert_eq!(
                item["type"], "function_call",
                "{name} must stay function_call"
            );
            assert_eq!(item["arguments"], r#"{"x":1}"#);
        }
    }

    #[test]
    fn unknown_tool_defaults_to_function_call() {
        let item = desktop_tool_call_item(
            "resp_x",
            0,
            "some_mcp_tool",
            "call_1",
            "{}",
            "completed",
        );
        assert_eq!(item["type"], "function_call");
    }

    #[test]
    fn freeform_set_is_exactly_apply_patch_and_exec() {
        let freeform: HashSet<&str> = freeform_tool_names().iter().copied().collect();
        assert_eq!(
            freeform,
            HashSet::from([APPLY_PATCH_TOOL_NAME, EXEC_FREEFORM_TOOL_NAME])
        );
        for p in PROFILES {
            let is_ff = p.desktop_call == DesktopCallType::CustomToolCall && p.freeform_input;
            assert_eq!(
                is_ff,
                freeform.contains(p.name),
                "profile {} freeform flag mismatch",
                p.name
            );
        }
    }

    #[test]
    fn all_registered_profiles_have_unique_names() {
        let mut seen = HashSet::new();
        for p in PROFILES {
            assert!(
                seen.insert(p.name),
                "duplicate tool profile name: {}",
                p.name
            );
            assert!(!p.name.is_empty());
        }
        assert!(
            seen.len() >= 50,
            "expected full inventory registry, got {}",
            seen.len()
        );
    }

    #[test]
    fn uses_custom_tool_sse_only_for_freeform() {
        assert!(uses_custom_tool_sse("apply_patch"));
        assert!(uses_custom_tool_sse("exec"));
        assert!(!uses_custom_tool_sse("exec_command"));
        assert!(!uses_custom_tool_sse("update_plan"));
    }

    #[test]
    fn restore_freeform_output_item_rewrites_function_call_apply_patch() {
        let upstream = json!({
            "id": "fc_abc",
            "type": "function_call",
            "status": "completed",
            "call_id": "call_patch_1",
            "name": "apply_patch",
            "arguments": "{\"input\":\"*** Begin Patch\\n*** End Patch\"}"
        });
        let restored = restore_freeform_output_item(&upstream);
        assert_eq!(restored["type"], "custom_tool_call");
        assert_eq!(restored["name"], "apply_patch");
        assert_eq!(restored["call_id"], "call_patch_1");
        assert!(restored["id"].as_str().unwrap().starts_with("ctc_"));
        let input = restored["input"].as_str().unwrap();
        assert!(input.starts_with("*** Begin Patch"));
        assert!(restored.get("arguments").is_none());
    }

    #[test]
    fn restore_freeform_output_item_leaves_exec_command() {
        let upstream = json!({
            "type": "function_call",
            "name": "exec_command",
            "call_id": "call_1",
            "arguments": "{\"cmd\":\"ls\"}"
        });
        let restored = restore_freeform_output_item(&upstream);
        assert_eq!(restored["type"], "function_call");
        assert_eq!(restored["name"], "exec_command");
    }

    #[test]
    fn restore_freeform_output_item_idempotent_for_custom_tool_call() {
        let gold = json!({
            "id": "ctc_1",
            "type": "custom_tool_call",
            "status": "completed",
            "call_id": "call_1",
            "name": "apply_patch",
            "input": "*** Begin Patch\n*** End Patch\n"
        });
        let restored = restore_freeform_output_item(&gold);
        assert_eq!(restored, gold);
    }

    #[test]
    fn restore_freeform_in_responses_body_walks_output_and_nested_response() {
        let mut body = json!({
            "output": [{
                "type": "function_call",
                "name": "apply_patch",
                "call_id": "c1",
                "arguments": "{\"input\":\"*** Begin Patch\\n*** End Patch\"}"
            }],
            "response": {
                "output": [{
                    "type": "function_call",
                    "name": "exec",
                    "call_id": "c2",
                    "arguments": "{\"input\":\"2+2\"}"
                }, {
                    "type": "function_call",
                    "name": "exec_command",
                    "call_id": "c3",
                    "arguments": "{\"cmd\":\"pwd\"}"
                }]
            }
        });
        restore_freeform_in_responses_body(&mut body);
        assert_eq!(body["output"][0]["type"], "custom_tool_call");
        assert_eq!(body["response"]["output"][0]["type"], "custom_tool_call");
        assert_eq!(body["response"]["output"][0]["input"], "2+2");
        assert_eq!(body["response"]["output"][1]["type"], "function_call");
    }
}
