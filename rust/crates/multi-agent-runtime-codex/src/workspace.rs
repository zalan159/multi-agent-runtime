use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use multi_agent_protocol::{
    instantiate_workspace, DispatchStatus, RoleSpec, RoleTaskRequest, TaskDispatch, WorkspaceEvent,
    WorkspaceInstanceParams, WorkspaceProfile, WorkspaceSpec, WorkspaceTemplate,
};
use multi_agent_runtime_core::{RuntimeError, WorkspaceRuntime};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl CodexSandboxMode {
    fn as_cli_value(self) -> &'static str {
        match self {
            CodexSandboxMode::ReadOnly => "read-only",
            CodexSandboxMode::WorkspaceWrite => "workspace-write",
            CodexSandboxMode::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexApprovalPolicy {
    Untrusted,
    OnFailure,
    OnRequest,
    Never,
}

impl CodexApprovalPolicy {
    fn as_cli_value(self) -> &'static str {
        match self {
            CodexApprovalPolicy::Untrusted => "untrusted",
            CodexApprovalPolicy::OnFailure => "on-failure",
            CodexApprovalPolicy::OnRequest => "on-request",
            CodexApprovalPolicy::Never => "never",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexWorkspaceOptions {
    pub codex_path: PathBuf,
    pub sandbox_mode: CodexSandboxMode,
    pub approval_policy: CodexApprovalPolicy,
    pub working_directory: Option<PathBuf>,
    pub additional_directories: Vec<PathBuf>,
    pub temp_directory_name: String,
    pub skip_git_repo_check: bool,
    pub turn_timeout: Duration,
}

impl Default for CodexWorkspaceOptions {
    fn default() -> Self {
        Self {
            codex_path: PathBuf::from("codex"),
            sandbox_mode: CodexSandboxMode::WorkspaceWrite,
            approval_policy: CodexApprovalPolicy::Never,
            working_directory: None,
            additional_directories: Vec::new(),
            temp_directory_name: ".codex-tmp".to_string(),
            skip_git_repo_check: true,
            turn_timeout: Duration::from_secs(240),
        }
    }
}

#[derive(Debug)]
pub struct CodexRoleTaskRun {
    pub dispatch: TaskDispatch,
    pub events: Vec<WorkspaceEvent>,
}

#[derive(Debug, Error)]
pub enum CodexAdapterError {
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("unknown role: {0}")]
    UnknownRole(String),
    #[error("codex process missing stdin")]
    MissingStdin,
    #[error("codex process missing stdout")]
    MissingStdout,
    #[error("codex process missing stderr")]
    MissingStderr,
    #[error("codex io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("codex task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("codex stderr: {0}")]
    Process(String),
    #[error("codex failed turn: {0}")]
    TurnFailed(String),
    #[error("codex timed out after {timeout:?}\n{debug}")]
    TimedOut { timeout: Duration, debug: String },
}

pub struct CodexWorkspace {
    runtime: WorkspaceRuntime,
    options: CodexWorkspaceOptions,
    started: bool,
    role_thread_ids: BTreeMap<String, String>,
}

impl CodexWorkspace {
    pub fn new(spec: WorkspaceSpec, options: CodexWorkspaceOptions) -> Self {
        Self {
            runtime: WorkspaceRuntime::new(spec),
            options,
            started: false,
            role_thread_ids: BTreeMap::new(),
        }
    }

    pub fn from_template(
        template: &WorkspaceTemplate,
        instance: &WorkspaceInstanceParams,
        profile: &WorkspaceProfile,
        options: CodexWorkspaceOptions,
    ) -> Self {
        Self::new(instantiate_workspace(template, instance, profile), options)
    }

    pub fn runtime(&self) -> &WorkspaceRuntime {
        &self.runtime
    }

    pub fn start(&mut self) -> Vec<WorkspaceEvent> {
        if self.started {
            return Vec::new();
        }

        self.started = true;
        let mut emitted = Vec::new();
        emitted.extend(self.runtime.start().emitted);
        emitted.extend(
            self.runtime
                .initialize(
                    None,
                    self.runtime
                        .spec()
                        .roles
                        .iter()
                        .map(|role| role.id.clone())
                        .collect(),
                    self.runtime
                        .spec()
                        .allowed_tools
                        .clone()
                        .unwrap_or_default(),
                    Some(vec!["exec".to_string(), "resume".to_string()]),
                )
                .emitted,
        );
        emitted
    }

    pub async fn run_role_task(
        &mut self,
        request: RoleTaskRequest,
    ) -> Result<CodexRoleTaskRun, CodexAdapterError> {
        let role = self
            .runtime
            .spec()
            .roles
            .iter()
            .find(|role| role.id == request.role_id)
            .cloned()
            .ok_or_else(|| CodexAdapterError::UnknownRole(request.role_id.clone()))?;

        let (dispatch, queued_tick) = self.runtime.queue_dispatch(request)?;
        let mut emitted = queued_tick.emitted;

        let provider_result = self.execute_provider_turn(&role, &dispatch).await?;
        emitted.extend(provider_result.events);

        let snapshot = self.runtime.snapshot();
        let final_dispatch = snapshot
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after provider turn");

        Ok(CodexRoleTaskRun {
            dispatch: final_dispatch,
            events: emitted,
        })
    }

    async fn execute_provider_turn(
        &mut self,
        role: &RoleSpec,
        dispatch: &TaskDispatch,
    ) -> Result<CodexRoleTaskRun, CodexAdapterError> {
        let role_thread_id = self.role_thread_ids.get(&role.id).cloned();
        let effective_working_directory = self
            .options
            .working_directory
            .clone()
            .or_else(|| self.runtime.spec().cwd.as_ref().map(PathBuf::from));
        let temp_directory = effective_working_directory
            .as_ref()
            .map(|working_directory| working_directory.join(&self.options.temp_directory_name));
        let stdout_tail = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_tail = Arc::new(Mutex::new(Vec::<String>::new()));

        if let Some(temp_directory) = temp_directory.as_ref() {
            fs::create_dir_all(temp_directory)?;
        }

        let mut command = Command::new(&self.options.codex_path);
        command
            .arg("exec")
            .arg("--experimental-json")
            .arg("--model")
            .arg(&self.runtime.spec().model)
            .arg("--sandbox")
            .arg(self.options.sandbox_mode.as_cli_value())
            .arg("--config")
            .arg(format!(
                "approval_policy=\"{}\"",
                self.options.approval_policy.as_cli_value()
            ));
        if let Some(working_directory) = effective_working_directory.as_ref() {
            command.arg("--cd").arg(working_directory);
        }

        for additional_directory in &self.options.additional_directories {
            command.arg("--add-dir").arg(additional_directory);
        }

        if self.options.skip_git_repo_check {
            command.arg("--skip-git-repo-check");
        }

        if let Some(thread_id) = role_thread_id.as_deref() {
            command.arg("resume").arg(thread_id);
        } else {
        }

        if let Some(temp_directory) = temp_directory.as_ref() {
            command.env("TMPDIR", temp_directory);
        }
        command.env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "multi_agent_runtime_rust");
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().ok_or(CodexAdapterError::MissingStdin)?;
        let stdout = child.stdout.take().ok_or(CodexAdapterError::MissingStdout)?;
        let stderr = child.stderr.take().ok_or(CodexAdapterError::MissingStderr)?;
        let command_line = render_command_line(&self.options.codex_path, &command);

        let prompt = format!(
            "{}\n",
            build_dispatch_prompt(self.runtime.spec(), role, dispatch)
        );
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await?;
        drop(stdin);

        let stderr_tail_for_task = Arc::clone(&stderr_tail);
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut buffer = Vec::new();
            while let Some(line) = lines.next_line().await? {
                push_tail(&stderr_tail_for_task, line.clone());
                buffer.push(line);
            }
            Ok::<Vec<String>, std::io::Error>(buffer)
        });

        let stdout_tail_for_processing = Arc::clone(&stdout_tail);
        let processing = async {
            let mut stdout_lines = BufReader::new(stdout).lines();
            let mut emitted = Vec::new();
            let mut final_result_text: Option<String> = None;
            let mut saw_turn_completion = false;
            let mut saw_turn_failure = None::<String>;

            while let Some(line) = stdout_lines.next_line().await? {
                push_tail(&stdout_tail_for_processing, line.clone());
                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    continue;
                }

                let event: CodexJsonEvent = match serde_json::from_str(trimmed) {
                    Ok(event) => event,
                    Err(_) => continue,
                };

                match event {
                    CodexJsonEvent::ThreadStarted { thread_id } => {
                        self.role_thread_ids.insert(role.id.clone(), thread_id.clone());
                        emitted.extend(
                            self.runtime
                                .start_next_dispatch(
                                    thread_id.clone(),
                                    dispatch
                                        .summary
                                        .clone()
                                        .unwrap_or_else(|| dispatch.instruction.clone()),
                                    Some(format!("codex-thread:{thread_id}")),
                                )?
                                .1
                                .emitted,
                        );
                    }
                    CodexJsonEvent::ItemStarted { item } => {
                        if let CodexItem::CommandExecution { id, command, .. } = item {
                            let _ = id;
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        command,
                                        Some("Codex is executing a shell command.".to_string()),
                                        Some("Bash".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                    }
                    CodexJsonEvent::ItemUpdated { item } => {
                        if let CodexItem::TodoList { items, .. } = item {
                            let remaining = items.iter().filter(|item| !item.completed).count();
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        "todo_list",
                                        Some(format!(
                                            "Codex is tracking {remaining} remaining todo item(s)."
                                        )),
                                        Some("TodoList".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                    }
                    CodexJsonEvent::ItemCompleted { item } => match item {
                        CodexItem::CommandExecution {
                            command,
                            exit_code,
                            ..
                        } => {
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        command,
                                        Some(match exit_code {
                                            Some(0) => {
                                                "Codex completed a shell command.".to_string()
                                            }
                                            Some(code) => {
                                                format!("Codex command exited with code {code}.")
                                            }
                                            None => {
                                                "Codex completed a shell command.".to_string()
                                            }
                                        }),
                                        Some("Bash".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::AgentMessage { text, .. } => {
                            final_result_text = Some(text.clone());
                            emitted.push(WorkspaceEvent::Message {
                                timestamp: Utc::now().to_rfc3339(),
                                workspace_id: self.runtime.spec().id.clone(),
                                role: "assistant".to_string(),
                                text,
                                visibility: Some(multi_agent_protocol::WorkspaceVisibility::Public),
                                member_id: Some(role.id.clone()),
                                session_id: self.role_thread_ids.get(&role.id).cloned(),
                                parent_tool_use_id: None,
                            });
                        }
                        CodexItem::FileChange { changes, .. } => {
                            let description = changes
                                .iter()
                                .map(|change| change.path.clone())
                                .collect::<Vec<_>>()
                                .join(", ");
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        description,
                                        Some("Codex applied file changes.".to_string()),
                                        Some("ApplyPatch".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::McpToolCall {
                            server,
                            tool,
                            status,
                            error,
                            ..
                        } => {
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        format!("{server}.{tool}"),
                                        Some(match status.as_deref() {
                                            Some("failed") => error
                                                .and_then(|error| error.message)
                                                .unwrap_or_else(|| {
                                                    "Codex MCP call failed.".to_string()
                                                }),
                                            _ => "Codex completed an MCP tool call.".to_string(),
                                        }),
                                        Some(tool),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::WebSearch { query, .. } => {
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        query,
                                        Some("Codex completed a web search.".to_string()),
                                        Some("WebSearch".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::Error { message, .. } => {
                            emitted.push(WorkspaceEvent::Error {
                                timestamp: Utc::now().to_rfc3339(),
                                workspace_id: self.runtime.spec().id.clone(),
                                error: message,
                            });
                        }
                        CodexItem::Reasoning { text, .. } => {
                            emitted.extend(
                                self.runtime
                                    .progress_dispatch(
                                        &current_task_id(&self.role_thread_ids, &role.id, dispatch),
                                        "reasoning",
                                        Some(text),
                                        Some("Reasoning".to_string()),
                                    )?
                                    .emitted,
                            );
                        }
                        CodexItem::TodoList { .. } => {}
                    },
                    CodexJsonEvent::TurnCompleted { .. } => {
                        saw_turn_completion = true;
                    }
                    CodexJsonEvent::TurnFailed { error } => {
                        saw_turn_failure = Some(error.message);
                    }
                    CodexJsonEvent::Error { message } => {
                        emitted.push(WorkspaceEvent::Error {
                            timestamp: Utc::now().to_rfc3339(),
                            workspace_id: self.runtime.spec().id.clone(),
                            error: message,
                        });
                    }
                    CodexJsonEvent::TurnStarted => {}
                }
            }

            let status = child.wait().await?;

            Ok::<_, CodexAdapterError>((
                emitted,
                final_result_text,
                saw_turn_completion,
                saw_turn_failure,
                status.success(),
            ))
        };

        let (mut emitted, final_result_text, saw_turn_completion, saw_turn_failure, process_success) =
            match timeout(self.options.turn_timeout, processing).await {
                Ok(result) => result?,
                Err(_) => {
                    let _ = child.kill().await;
                    stderr_task.abort();
                    return Err(CodexAdapterError::TimedOut {
                        timeout: self.options.turn_timeout,
                        debug: render_debug_context(
                            &command_line,
                            &prompt,
                            effective_working_directory.as_ref(),
                            temp_directory.as_ref(),
                            &stdout_tail,
                            &stderr_tail,
                        ),
                    });
                }
            };

        let stderr_lines = stderr_task.await??;
        if !process_success {
            return Err(CodexAdapterError::Process(stderr_lines.join("\n")));
        }
        if let Some(message) = saw_turn_failure {
            return Err(CodexAdapterError::TurnFailed(message));
        }

        let provider_task_id = self
            .role_thread_ids
            .get(&role.id)
            .cloned()
            .unwrap_or_else(|| dispatch.dispatch_id.to_string());

        if saw_turn_completion {
            emitted.extend(
                self.runtime
                    .complete_dispatch(
                        &provider_task_id,
                        DispatchStatus::Completed,
                        None,
                        "Codex completed the turn.".to_string(),
                    )?
                    .emitted,
            );
            if let Some(result_text) = final_result_text {
                emitted.extend(
                    self.runtime
                        .attach_result_text(&provider_task_id, result_text)?
                        .emitted,
                );
            }
        }

        let snapshot = self.runtime.snapshot();
        let final_dispatch = snapshot
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after codex turn");

        Ok(CodexRoleTaskRun {
            dispatch: final_dispatch,
            events: emitted,
        })
    }
}

fn build_dispatch_prompt(spec: &WorkspaceSpec, role: &RoleSpec, dispatch: &TaskDispatch) -> String {
    let mut parts = vec![format!(
        "You are the {} role in the workspace \"{}\".",
        role.name, spec.name
    )];

    parts.push(
        "The current working directory is the workspace root for this task. Create or edit files using paths relative to the current directory, and avoid exploring unrelated directories.".to_string(),
    );
    parts.push(
        "If a shell command needs a temporary directory, use .codex-tmp/ inside the workspace rather than relying on the system temp directory.".to_string(),
    );

    if let Some(description) = role.description.as_ref() {
        parts.push(format!("Role description: {description}"));
    }
    parts.push(format!(
        "Follow this role-specific instruction set strictly:\n{}",
        role.agent.prompt
    ));
    if let Some(orchestrator_prompt) = spec.orchestrator_prompt.as_ref() {
        parts.push(format!(
            "Workspace orchestration context:\n{}",
            orchestrator_prompt
        ));
    }
    if let Some(output_root) = role.output_root.as_ref() {
        parts.push(format!("Preferred output root for this role: {output_root}"));
    }
    if let Some(summary) = dispatch.summary.as_ref() {
        parts.push(format!("Task summary: {summary}"));
    }
    parts.push(format!("Task instruction:\n{}", dispatch.instruction));
    parts.push(
        "Return a concise final answer after completing the task. If you create or edit files, mention the key output paths in the final answer."
            .to_string(),
    );

    parts.join("\n\n")
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum CodexJsonEvent {
    #[serde(rename = "thread.started")]
    ThreadStarted { thread_id: String },
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        #[serde(default)]
        usage: Option<Value>,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: CodexTurnError },
    #[serde(rename = "item.started")]
    ItemStarted { item: CodexItem },
    #[serde(rename = "item.updated")]
    ItemUpdated { item: CodexItem },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: CodexItem },
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Debug, Deserialize)]
struct CodexTurnError {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum CodexItem {
    #[serde(rename = "command_execution")]
    CommandExecution {
        id: String,
        command: String,
        #[serde(default)]
        aggregated_output: Option<String>,
        #[serde(default)]
        exit_code: Option<i32>,
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(rename = "agent_message")]
    AgentMessage { id: String, text: String },
    #[serde(rename = "reasoning")]
    Reasoning { id: String, text: String },
    #[serde(rename = "file_change")]
    FileChange {
        id: String,
        changes: Vec<CodexFileChange>,
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(rename = "mcp_tool_call")]
    McpToolCall {
        id: String,
        server: String,
        tool: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        error: Option<CodexItemError>,
    },
    #[serde(rename = "web_search")]
    WebSearch { id: String, query: String },
    #[serde(rename = "todo_list")]
    TodoList { id: String, items: Vec<CodexTodoItem> },
    #[serde(rename = "error")]
    Error { id: String, message: String },
}

#[derive(Debug, Deserialize)]
struct CodexFileChange {
    path: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct CodexItemError {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexTodoItem {
    text: String,
    completed: bool,
}

fn current_task_id<'a>(
    role_thread_ids: &BTreeMap<String, String>,
    role_id: &str,
    dispatch: &TaskDispatch,
) -> String {
    role_thread_ids
        .get(role_id)
        .cloned()
        .unwrap_or_else(|| dispatch.dispatch_id.to_string())
}

fn push_tail(buffer: &Arc<Mutex<Vec<String>>>, line: String) {
    let mut guard = buffer.lock().expect("tail buffer mutex should not be poisoned");
    guard.push(line);
    if guard.len() > 40 {
        let overflow = guard.len() - 40;
        guard.drain(0..overflow);
    }
}

fn render_command_line(program: &PathBuf, command: &Command) -> String {
    let mut parts = vec![program.display().to_string()];
    parts.extend(command.as_std().get_args().map(|arg| arg.to_string_lossy().to_string()));
    parts.join(" ")
}

fn render_debug_context(
    command_line: &str,
    prompt: &str,
    working_directory: Option<&PathBuf>,
    temp_directory: Option<&PathBuf>,
    stdout_tail: &Arc<Mutex<Vec<String>>>,
    stderr_tail: &Arc<Mutex<Vec<String>>>,
) -> String {
    let stdout_tail = stdout_tail
        .lock()
        .expect("stdout tail mutex should not be poisoned")
        .join("\n");
    let stderr_tail = stderr_tail
        .lock()
        .expect("stderr tail mutex should not be poisoned")
        .join("\n");
    let prompt_preview = prompt
        .lines()
        .take(24)
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "command: {command_line}\nworking_directory: {}\ntemp_directory: {}\nprompt_preview:\n{prompt_preview}\nstdout_tail:\n{stdout_tail}\nstderr_tail:\n{stderr_tail}",
        working_directory
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        temp_directory
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
    )
}
