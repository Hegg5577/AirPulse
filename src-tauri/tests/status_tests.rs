use airpulse_lib::status::{HookEvent, MAX_HOOK_BYTES, StatusCore, TaskStatus};
use serde_json::{Value, json};

/// 用官方字段构造事件，所有状态检查都经过接收器的输入边界。
fn event(
    name: &str,
    session: &str,
    turn: &str,
    tool: Option<&str>,
    agent: Option<&str>,
    response: Option<Value>,
) -> HookEvent {
    HookEvent::parse(&serde_json::to_vec(&json!({
        "hook_event_name": name,
        "session_id": session,
        "turn_id": turn,
        "tool_name": tool,
        "agent_id": agent,
        "tool_response": response,
    })).unwrap()).unwrap()
}

/// 多聊天聚合严格遵循失败、权限、执行、全部完成的优先级。
#[test]
fn aggregation_uses_global_priority_and_all_completed_rule() {
    let mut core = StatusCore::default();
    core.apply(&event("UserPromptSubmit", "a", "1", None, None, None), 100).unwrap();
    core.apply(&event("Stop", "a", "1", None, None, None), 200).unwrap();
    assert_eq!(core.aggregate_at(200), TaskStatus::Completed);

    core.apply(&event("UserPromptSubmit", "b", "2", None, None, None), 300).unwrap();
    assert_eq!(core.aggregate_at(300), TaskStatus::Running);
    core.apply(&event("PermissionRequest", "b", "2", Some("Bash"), None, None), 400).unwrap();
    assert_eq!(core.aggregate_at(400), TaskStatus::PermissionWaiting);
    core.apply(&event("PostToolUse", "b", "2", Some("Bash"), None,
        Some(json!({ "exit_code": 1 }))), 500).unwrap();
    assert_eq!(core.aggregate_at(500), TaskStatus::ToolFailed);

    core.apply(&event("UserPromptSubmit", "b", "3", None, None, None), 600).unwrap();
    assert_eq!(core.aggregate_at(600), TaskStatus::Running);
    core.apply(&event("Stop", "b", "3", None, None, None), 700).unwrap();
    assert_eq!(core.aggregate_at(700), TaskStatus::Completed);
    core.apply(&event("SessionEnd", "a", "1", None, None, None), 800).unwrap();
    core.apply(&event("SessionEnd", "b", "3", None, None, None), 900).unwrap();
    assert_eq!(core.aggregate_at(900), TaskStatus::Idle);
}

/// 只有结构化失败标志会触发工具失败，自由文本和零退出码不误判。
#[test]
fn tool_failure_requires_structured_failure_marker() {
    let mut core = StatusCore::default();
    core.apply(&event("UserPromptSubmit", "a", "1", None, None, None), 100).unwrap();
    for response in [json!("error in ordinary output"), json!({ "exit_code": 0 }),
        json!({ "status_code": 200 })] {
        core.apply(&event("PostToolUse", "a", "1", Some("Bash"), None, Some(response)), 200).unwrap();
        assert_eq!(core.aggregate_at(200), TaskStatus::Running);
    }
    core.apply(&event("PostToolUse", "a", "1", Some("mcp__server__tool"), None,
        Some(json!({ "isError": true }))), 300).unwrap();
    assert_eq!(core.aggregate_at(300), TaskStatus::ToolFailed);
}

/// 新轮次清除旧等待和失败，迟到的旧事件不能覆盖新状态。
#[test]
fn old_turn_events_cannot_override_new_prompt() {
    let mut core = StatusCore::default();
    core.apply(&event("UserPromptSubmit", "a", "old", None, None, None), 100).unwrap();
    core.apply(&event("PermissionRequest", "a", "old", Some("Bash"), None, None), 200).unwrap();
    assert_eq!(core.aggregate_at(200), TaskStatus::PermissionWaiting);
    core.apply(&event("UserPromptSubmit", "a", "new", None, None, None), 300).unwrap();
    for name in ["Stop", "Interrupt", "PermissionRequest", "PostToolUse"] {
        core.apply(&event(name, "a", "old", Some("Bash"), None,
            Some(json!({ "exit_code": 1 }))), 400).unwrap();
        assert_eq!(core.aggregate_at(400), TaskStatus::Running);
    }
    core.apply(&event("Interrupt", "a", "new", None, None, None), 500).unwrap();
    assert_eq!(core.aggregate_at(500), TaskStatus::Idle);
}

/// 子代理失败在停止时转移给主任务，不能因删除子任务而丢失。
#[test]
fn subagent_failure_survives_subagent_stop() {
    let mut core = StatusCore::default();
    core.apply(&event("UserPromptSubmit", "a", "1", None, None, None), 100).unwrap();
    core.apply(&event("SubagentStart", "a", "1", None, Some("child"), None), 200).unwrap();
    core.apply(&event("PostToolUse", "a", "1", Some("Bash"), Some("child"),
        Some(json!({ "exitCode": 2 }))), 300).unwrap();
    core.apply(&event("SubagentStop", "a", "1", None, Some("child"), None), 400).unwrap();
    assert_eq!(core.aggregate_at(400), TaskStatus::ToolFailed);
}

/// 主任务完成后仍跟踪子任务，只有所有任务停止才显示全部完成。
#[test]
fn root_stop_keeps_running_children_and_completion_has_no_ten_second_timeout() {
    let mut core = StatusCore::default();
    core.apply(&event("UserPromptSubmit", "a", "1", None, None, None), 100).unwrap();
    core.apply(&event("SubagentStart", "a", "1", None, Some("child"), None), 200).unwrap();
    core.apply(&event("Stop", "a", "1", None, None, None), 300).unwrap();
    assert_eq!(core.aggregate_at(300), TaskStatus::Running);
    core.apply(&event("PermissionRequest", "a", "1", Some("Bash"), Some("child"), None), 400).unwrap();
    assert_eq!(core.aggregate_at(400), TaskStatus::PermissionWaiting);
    core.apply(&event("SubagentStop", "a", "1", None, Some("child"), None), 500).unwrap();
    assert_eq!(core.aggregate_at(500 + 60_000), TaskStatus::Completed);
    core.apply(&event("PostToolUse", "a", "1", Some("Bash"), None,
        Some(json!({ "exit_code": 1 }))), 600).unwrap();
    assert_eq!(core.aggregate_at(600), TaskStatus::Completed);
}

/// 子任务在主任务完成后报告失败，仍优先展示失败并在子任务停止后保留。
#[test]
fn child_failure_is_retained_after_root_stop() {
    let mut core = StatusCore::default();
    core.apply(&event("UserPromptSubmit", "a", "1", None, None, None), 100).unwrap();
    core.apply(&event("SubagentStart", "a", "1", None, Some("child"), None), 200).unwrap();
    core.apply(&event("Stop", "a", "1", None, None, None), 300).unwrap();
    core.apply(&event("PostToolUse", "a", "1", Some("Bash"), Some("child"),
        Some(json!({ "exit_code": 1 }))), 400).unwrap();
    core.apply(&event("SubagentStop", "a", "1", None, Some("child"), None), 500).unwrap();
    assert_eq!(core.aggregate_at(500), TaskStatus::ToolFailed);
    core.apply(&event("Interrupt", "a", "1", None, None, None), 600).unwrap();
    assert_eq!(core.aggregate_at(600), TaskStatus::Idle);
}

/// 兼容旧状态文件的等待名称与缺失失败字段，工具回包后正常继续执行。
#[test]
fn legacy_waiting_state_recovers_on_tool_result() {
    let mut core: StatusCore = serde_json::from_value(json!({
        "version": 1,
        "sessions": {
            "a": {
                "turn_id": "1", "updated_ms": 100,
                "tasks": { "root": { "phase": "needsInput", "pending_tools": ["Bash"] } },
            },
        },
    })).unwrap();
    core.validate().unwrap();
    assert_eq!(core.aggregate_at(100), TaskStatus::PermissionWaiting);
    core.apply(&event("PostToolUse", "a", "1", Some("Bash"), None,
        Some(json!({ "exit_code": 0 }))), 200).unwrap();
    assert_eq!(core.aggregate_at(200), TaskStatus::Running);
}

/// 上下文压缩保留状态；缺失退出事件的任务一天后回到空闲。
#[test]
fn compact_preserves_state_and_stale_tasks_expire() {
    let compact = HookEvent::parse(
        br#"{"hook_event_name":"SessionStart","session_id":"a","source":"compact"}"#,
    ).unwrap();
    let mut core = StatusCore::default();
    core.apply(&event("UserPromptSubmit", "a", "1", None, None, None), 100).unwrap();
    core.apply(&event("PermissionRequest", "a", "1", Some("Bash"), None, None), 200).unwrap();
    core.apply(&compact, 300).unwrap();
    assert_eq!(core.aggregate_at(300), TaskStatus::PermissionWaiting);
    assert_eq!(core.aggregate_at(200 + 24 * 60 * 60 * 1000), TaskStatus::Idle);
}

/// 不完整事件和超大输入在边界处返回错误而非 panic。
#[test]
fn invalid_events_are_rejected() {
    for input in [
        br#"{"hook_event_name":"Unknown","session_id":"a"}"#.as_slice(),
        br#"{"hook_event_name":"PreToolUse","session_id":"a","turn_id":"1"}"#.as_slice(),
        br#"{"hook_event_name":"Stop","session_id":""}"#.as_slice(),
    ] {
        assert!(HookEvent::parse(input).is_err());
    }
    assert!(HookEvent::parse(&vec![b' '; MAX_HOOK_BYTES + 1]).is_err());
    let mut invalid = event("UserPromptSubmit", "a", "1", None, None, None);
    invalid.turn_id = None;
    assert!(StatusCore::default().apply(&invalid, 1).is_err());
}
