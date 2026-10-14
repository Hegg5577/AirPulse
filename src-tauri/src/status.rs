use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_HOOK_BYTES: usize = 1024 * 1024;
const STALE_MS: u64 = 24 * 60 * 60 * 1000;
const ROOT: &str = "root";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    #[default]
    #[serde(alias = "off")]
    Idle,
    Running,
    #[serde(alias = "needsInput")]
    PermissionWaiting,
    ToolFailed,
    Completed,
}

#[derive(Debug, Deserialize)]
pub struct HookEvent {
    pub hook_event_name: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub agent_id: Option<String>,
    pub tool_name: Option<String>,
    pub source: Option<String>,
    pub tool_response: Option<Value>,
}

impl HookEvent {
    /// 工具结果只在内存中用于识别失败，不将提示词、参数或正文写入状态文件。
    pub fn parse(input: &[u8]) -> Result<Self> {
        ensure!(input.len() <= MAX_HOOK_BYTES, "Hook 输入超过 1 MiB");
        let event: Self = serde_json::from_slice(input)
            .map_err(|_| anyhow::anyhow!("Hook 输入不是有效的事件 JSON"))?;
        event.validate()?;
        Ok(event)
    }

    /// 聚合器也校验直接构造的事件，保证公开接口不会因缺字段而崩溃。
    pub fn validate(&self) -> Result<()> {
        validate_id(&self.session_id)?;
        for value in [&self.turn_id, &self.agent_id, &self.tool_name].into_iter().flatten() {
            validate_id(value)?;
        }
        match self.hook_event_name.as_str() {
            "SessionStart" | "SessionEnd" => {}
            "UserPromptSubmit" | "Stop" | "Interrupt" => {
                ensure!(self.turn_id.is_some(), "轮次事件缺少 turn_id");
            }
            "SubagentStart" | "SubagentStop" => {
                ensure!(self.turn_id.is_some() && self.agent_id.is_some(), "子代理事件缺少标识");
                ensure!(self.agent_id.as_deref() != Some(ROOT), "子代理标识与主任务冲突");
            }
            "PreToolUse" | "PostToolUse" | "PermissionRequest" => {
                ensure!(self.turn_id.is_some() && self.tool_name.is_some(), "工具事件缺少标识");
            }
            _ => bail!("不支持的 Hook 事件"),
        }
        Ok(())
    }
}

/// 标识不用于文件名，长度限制用于防止异常事件无限扩大状态文件。
fn validate_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control),
        "Hook 标识为空、过长或包含控制字符"
    );
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Task {
    pub phase: TaskStatus,
    pub pending_tools: BTreeSet<String>,
    #[serde(default)]
    pub failed: bool,
}

impl Task {
    /// 新任务从执行中开始，失败只在当前轮次内保持。
    fn running() -> Self {
        Self { phase: TaskStatus::Running, pending_tools: BTreeSet::new(), failed: false }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Session {
    pub turn_id: Option<String>,
    pub updated_ms: u64,
    pub tasks: BTreeMap<String, Task>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StatusCore {
    pub version: u32,
    pub sessions: BTreeMap<String, Session>,
}

impl Default for StatusCore {
    /// 显式记录格式版本，避免新旧程序悄悄使用不兼容的数据。
    fn default() -> Self {
        Self { version: 1, sessions: BTreeMap::new() }
    }
}

impl StatusCore {
    /// 从不可信的状态文件读取时，也执行数量和标识校验。
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "状态文件版本不受支持");
        ensure!(self.sessions.len() <= 256, "会话数量超过上限");
        for (id, session) in &self.sessions {
            validate_id(id)?;
            if let Some(turn) = &session.turn_id { validate_id(turn)?; }
            ensure!(session.tasks.len() <= 64, "单会话任务数量超过上限");
            for (agent, task) in &session.tasks {
                validate_id(agent)?;
                ensure!(task.pending_tools.len() <= 64, "等待工具数量超过上限");
                for tool in &task.pending_tools { validate_id(tool)?; }
            }
        }
        Ok(())
    }

    /// 只接受当前轮次的事件，防止迟到的结束或工具回包覆盖新任务。
    pub fn apply(&mut self, event: &HookEvent, now_ms: u64) -> Result<()> {
        event.validate()?;
        self.sessions.retain(|_, session| now_ms.saturating_sub(session.updated_ms) < STALE_MS);
        if event.hook_event_name == "SessionEnd" {
            self.sessions.remove(&event.session_id);
            return Ok(());
        }
        ensure!(self.sessions.contains_key(&event.session_id) || self.sessions.len() < 256,
            "会话数量超过上限");
        let session = self.sessions.entry(event.session_id.clone()).or_default();
        match event.hook_event_name.as_str() {
            "SessionStart" => {
                if event.source.as_deref() == Some("compact") { return Ok(()); }
                *session = Session::default();
            }
            "UserPromptSubmit" => {
                session.turn_id = event.turn_id.clone();
                session.tasks.clear();
                session.tasks.insert(ROOT.to_owned(), Task::running());
            }
            _ => {
                if session.turn_id.is_none() {
                    session.turn_id = event.turn_id.clone();
                    session.tasks.insert(ROOT.to_owned(), Task::running());
                }
                if session.turn_id != event.turn_id { return Ok(()); }
                if !session.tasks.contains_key(ROOT) { return Ok(()); }
                match event.hook_event_name.as_str() {
                    "Stop" => {
                        if let Some(root) = session.tasks.get_mut(ROOT) {
                            root.phase = TaskStatus::Completed;
                            root.pending_tools.clear();
                        }
                    }
                    "Interrupt" => session.tasks.clear(),
                    "SubagentStop" => {
                        let agent = event.agent_id.as_deref().expect("子代理标识已校验");
                        let failed = session.tasks.remove(agent).is_some_and(|task| task.failed);
                        if failed { session.tasks.get_mut(ROOT).expect("主任务存在").failed = true; }
                    }
                    "SubagentStart" => {
                        if session.tasks[ROOT].phase == TaskStatus::Completed { return Ok(()); }
                        let agent = event.agent_id.as_ref().expect("子代理标识已校验");
                        ensure!(session.tasks.contains_key(agent) || session.tasks.len() < 64,
                            "单会话任务数量超过上限");
                        session.tasks.entry(agent.clone()).or_insert_with(Task::running);
                    }
                    "PreToolUse" => {}
                    "PermissionRequest" => {
                        let agent = event.agent_id.as_deref().unwrap_or(ROOT);
                        let Some(task) = session.tasks.get_mut(agent) else { return Ok(()); };
                        if task.phase == TaskStatus::Completed { return Ok(()); }
                        let tool = event.tool_name.as_ref().expect("工具标识已校验");
                        // shortcut: 审批事件缺少调用 ID，同名并发按名称合并；官方提供 ID 后改为精确关联。
                        ensure!(task.pending_tools.contains(tool) || task.pending_tools.len() < 64,
                            "等待工具数量超过上限");
                        task.pending_tools.insert(tool.clone());
                    }
                    "PostToolUse" => {
                        let agent = event.agent_id.as_deref().unwrap_or(ROOT);
                        let Some(task) = session.tasks.get_mut(agent) else { return Ok(()); };
                        if task.phase == TaskStatus::Completed { return Ok(()); }
                        let tool = event.tool_name.as_ref().expect("工具标识已校验");
                        task.pending_tools.remove(tool);
                        task.phase = TaskStatus::Running;
                        task.failed |= event.tool_response.as_ref().is_some_and(is_tool_failure);
                    }
                    _ => bail!("不支持的 Hook 事件"),
                }
            }
        }
        session.updated_ms = now_ms;
        self.validate()
    }

    /// 按失败、权限、执行、全部完成的固定优先级聚合所有仍被跟踪的任务。
    pub fn aggregate_at(&self, now_ms: u64) -> TaskStatus {
        let tasks: Vec<&Task> = self.sessions.values()
            .filter(|session| now_ms.saturating_sub(session.updated_ms) < STALE_MS)
            .flat_map(|session| session.tasks.values())
            .collect();
        if tasks.is_empty() {
            TaskStatus::Idle
        } else if tasks.iter().any(|task| task.failed) {
            TaskStatus::ToolFailed
        } else if tasks.iter().any(|task| !task.pending_tools.is_empty()
            || task.phase == TaskStatus::PermissionWaiting) {
            TaskStatus::PermissionWaiting
        } else if tasks.iter().any(|task| task.phase == TaskStatus::Running) {
            TaskStatus::Running
        } else if tasks.iter().all(|task| task.phase == TaskStatus::Completed) {
            TaskStatus::Completed
        } else {
            TaskStatus::Idle
        }
    }
}

/// 只识别官方结构化工具结果中的明确失败标志，不扫描自由文本。
fn is_tool_failure(response: &Value) -> bool {
    let Some(object) = response.as_object() else { return false; };
    if object.get("isError").and_then(Value::as_bool) == Some(true)
        || object.get("is_error").and_then(Value::as_bool) == Some(true)
    {
        return true;
    }
    ["exit_code", "exitCode"]
        .into_iter()
        .filter_map(|key| object.get(key).and_then(Value::as_i64))
        .any(|code| code != 0)
}
