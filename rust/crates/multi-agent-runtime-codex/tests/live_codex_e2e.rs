use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use multi_agent_protocol::{
    create_coding_studio_template, create_codex_workspace_profile, RoleTaskRequest,
    WorkspaceInstanceParams,
};
use multi_agent_runtime_codex::{CodexSandboxMode, CodexWorkspace, CodexWorkspaceOptions};

#[tokio::test]
#[ignore = "requires local Codex CLI auth and a live model run"]
async fn codex_live_coding_template_writes_file_and_reuses_thread() {
    let cwd = create_repo_local_temp_dir("multi-agent-runtime-codex-live");
    fs::create_dir_all(cwd.join("10-prd")).expect("should create temp workspace");

    let mut template = create_coding_studio_template();
    template.orchestrator_prompt = None;
    if let Some(prd_role) = template.roles.iter_mut().find(|role| role.id == "prd") {
        prd_role.agent.prompt = "You are a PRD writer. Complete the requested markdown file directly with minimal tool usage and no extra exploration.".to_string();
        prd_role.agent.description = "Writes concise markdown PRDs.".to_string();
    }
    let profile = create_codex_workspace_profile(None);
    let instance = WorkspaceInstanceParams {
        id: format!("codex-live-{}", unique_suffix()),
        name: "Codex Live Coding".to_string(),
        cwd: Some(cwd.to_string_lossy().to_string()),
    };

    let mut workspace = CodexWorkspace::from_template(
        &template,
        &instance,
        &profile,
        CodexWorkspaceOptions {
            sandbox_mode: CodexSandboxMode::DangerFullAccess,
            working_directory: Some(cwd.clone()),
            turn_timeout: std::time::Duration::from_secs(150),
            ..Default::default()
        },
    );
    let startup_events = workspace.start();
    assert!(startup_events.len() >= 2);

    let first = workspace
        .run_role_task(RoleTaskRequest {
            role_id: "prd".to_string(),
            summary: Some("Create a PRD stub for group mentions".to_string()),
            instruction: "Create 10-prd/group-mentions.md in the current workspace. Keep it under 120 words. Include exactly these markdown headings in order: Goal, User Story, Scope, Non-Goals, Acceptance Criteria. Do not inspect unrelated directories.".to_string(),
            visibility: None,
            source_role_id: None,
        })
        .await
        .expect("first codex task should succeed");

    let output_file = cwd.join("10-prd/group-mentions.md");
    let file_text = fs::read_to_string(&output_file).expect("expected PRD output file");
    assert!(file_text.contains("Goal"));
    assert!(file_text.contains("User Story"));
    assert_eq!(first.dispatch.status, multi_agent_protocol::DispatchStatus::Completed);

    let first_thread_id = first
        .dispatch
        .provider_task_id
        .clone()
        .expect("expected provider task id after first turn");

    let second = workspace
        .run_role_task(RoleTaskRequest {
            role_id: "prd".to_string(),
            summary: Some("Recall the file path from the previous turn".to_string()),
            instruction:
                "What file path did you just write in the previous turn? Reply with the path only."
                    .to_string(),
            visibility: None,
            source_role_id: None,
        })
        .await
        .expect("second codex task should succeed");

    let second_thread_id = second
        .dispatch
        .provider_task_id
        .clone()
        .expect("expected provider task id after second turn");

    assert_eq!(first_thread_id, second_thread_id);
    assert_eq!(
        second.dispatch.result_text.as_deref(),
        Some("10-prd/group-mentions.md")
    );
}

fn create_repo_local_temp_dir(prefix: &str) -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..");
    repo_root.join(".tmp").join(format!("{prefix}-{}", unique_suffix()))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_millis()
}
