use std::collections::BTreeMap;
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
pub enum ClaudePermissionMode {
    Default,
    Auto,
    AcceptEdits,
    BypassPermissions,
    DontAsk,
    Plan,
}

impl ClaudePermissionMode {
    fn as_cli_value(self) -> &'static str {
        match self {
            ClaudePermissionMode::Default => "default",
            ClaudePermissionMode::Auto => "auto",
            ClaudePermissionMode::AcceptEdits => "acceptEdits",
            ClaudePermissionMode::BypassPermissions => "bypassPermissions",
            ClaudePermissionMode::DontAsk => "dontAsk",
            ClaudePermissionMode::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeWorkspaceOptions {
    pub claude_path: PathBuf,
    pub permission_mode: ClaudePermissionMode,
    pub working_directory: Option<PathBuf>,
    pub additional_directories: Vec<PathBuf>,
    pub turn_timeout: Duration,
}

impl Default for ClaudeWorkspaceOptions {
    fn default() -> Self {
        Self {
            claude_path: PathBuf::from("claude"),
            permission_mode: ClaudePermissionMode::BypassPermissions,
            working_directory: None,
            additional_directories: Vec::new(),
            turn_timeout: Duration::from_secs(240),
        }
    }
}

#[derive(Debug)]
pub struct ClaudeRoleTaskRun {
    pub dispatch: TaskDispatch,
    pub events: Vec<WorkspaceEvent>,
}

#[derive(Debug, Error)]
pub enum ClaudeAdapterError {
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("unknown role: {0}")]
    UnknownRole(String),
    #[error("claude process missing stdout")]
    MissingStdout,
    #[error("claude process missing stderr")]
    MissingStderr,
    #[error("claude io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("claude task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("claude stderr: {0}")]
    Process(String),
    #[error("claude failed turn: {0}")]
    TurnFailed(String),
    #[error("claude timed out after {timeout:?}\n{debug}")]
    TimedOut { timeout: Duration, debug: String },
}

pub struct ClaudeWorkspace {
    runtime: WorkspaceRuntime,
    options: ClaudeWorkspaceOptions,
    started: bool,
    role_session_ids: BTreeMap<String, String>,
}

impl ClaudeWorkspace {
    pub fn new(spec: WorkspaceSpec, options: ClaudeWorkspaceOptions) -> Self {
        Self {
            runtime: WorkspaceRuntime::new(spec),
            options,
            started: false,
            role_session_ids: BTreeMap::new(),
        }
    }

    pub fn from_template(
        template: &WorkspaceTemplate,
        instance: &WorkspaceInstanceParams,
        profile: &WorkspaceProfile,
        options: ClaudeWorkspaceOptions,
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
                    Some(vec!["print".to_string(), "resume".to_string()]),
                )
                .emitted,
        );
        emitted
    }

    pub async fn run_role_task(
        &mut self,
        request: RoleTaskRequest,
    ) -> Result<ClaudeRoleTaskRun, ClaudeAdapterError> {
        let role = self
            .runtime
            .spec()
            .roles
            .iter()
            .find(|role| role.id == request.role_id)
            .cloned()
            .ok_or_else(|| ClaudeAdapterError::UnknownRole(request.role_id.clone()))?;

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

        Ok(ClaudeRoleTaskRun {
            dispatch: final_dispatch,
            events: emitted,
        })
    }

    async fn execute_provider_turn(
        &mut self,
        role: &RoleSpec,
        dispatch: &TaskDispatch,
    ) -> Result<ClaudeRoleTaskRun, ClaudeAdapterError> {
        let role_session_id = self.role_session_ids.get(&role.id).cloned();
        let effective_working_directory = self
            .options
            .working_directory
            .clone()
            .or_else(|| self.runtime.spec().cwd.as_ref().map(PathBuf::from));
        let stdout_tail = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_tail = Arc::new(Mutex::new(Vec::<String>::new()));

        let mut command = Command::new(&self.options.claude_path);
        command
            .arg("-p")
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--permission-mode")
            .arg(self.options.permission_mode.as_cli_value())
            .arg("--model")
            .arg(&self.runtime.spec().model);

        if let Some(working_directory) = effective_working_directory.as_ref() {
            command.current_dir(working_directory);
        }

        for additional_directory in &self.options.additional_directories {
            command.arg("--add-dir").arg(additional_directory);
        }

        if let Some(session_id) = role_session_id.as_deref() {
            command.arg("--resume").arg(session_id);
        }

        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().expect("claude stdin should be piped");
        let stdout = child.stdout.take().ok_or(ClaudeAdapterError::MissingStdout)?;
        let stderr = child.stderr.take().ok_or(ClaudeAdapterError::MissingStderr)?;
        let command_line = render_command_line(&self.options.claude_path, &command);

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
            let mut turn_failed = None::<String>;

            while let Some(line) = stdout_lines.next_line().await? {
                push_tail(&stdout_tail_for_processing, line.clone());
                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    continue;
                }

                let event: ClaudeJsonEvent = match serde_json::from_str(trimmed) {
                    Ok(event) => event,
                    Err(_) => continue,
                };

                match event {
                    ClaudeJsonEvent::System {
                        subtype,
                        session_id,
                        tools,
                    } if subtype == "init" => {
                        if let Some(session_id) = session_id {
                            self.role_session_ids
                                .insert(role.id.clone(), session_id.clone());
                            emitted.extend(
                                self.runtime
                                    .start_next_dispatch(
                                        session_id.clone(),
                                        dispatch
                                            .summary
                                            .clone()
                                            .unwrap_or_else(|| dispatch.instruction.clone()),
                                        Some(format!("claude-session:{session_id}")),
                                    )?
                                    .1
                                    .emitted,
                            );
                            if let Some(tools) = tools {
                                emitted.extend(
                                    self.runtime
                                        .initialize(
                                            Some(session_id),
                                            self.runtime
                                                .spec()
                                                .roles
                                                .iter()
                                                .map(|role| role.id.clone())
                                                .collect(),
                                            tools,
                                            Some(vec!["print".to_string(), "resume".to_string()]),
                                        )
                                        .emitted,
                                );
                            }
                        }
                    }
                    ClaudeJsonEvent::Assistant { message, session_id } => {
                        for content in message.content {
                            match content {
                                ClaudeContent::ToolUse { id, name, input } => {
                                    let description = summarize_tool_input(&name, &input);
                                    emitted.extend(
                                        self.runtime
                                            .progress_dispatch(
                                                &current_task_id(
                                                    &self.role_session_ids,
                                                    &role.id,
                                                    dispatch,
                                                ),
                                                description,
                                                Some(format!(
                                                    "Claude is using the {name} tool."
                                                )),
                                                Some(name.clone()),
                                            )?
                                            .emitted,
                                    );

                                    emitted.push(WorkspaceEvent::Message {
                                        timestamp: Utc::now().to_rfc3339(),
                                        workspace_id: self.runtime.spec().id.clone(),
                                        role: role.id.clone(),
                                        text: format!("{name} tool started."),
                                        visibility: Some(
                                            multi_agent_protocol::WorkspaceVisibility::Private,
                                        ),
                                        member_id: Some(role.id.clone()),
                                        session_id: session_id.clone(),
                                        parent_tool_use_id: Some(id),
                                    });
                                }
                                ClaudeContent::Text { text } => {
                                    final_result_text = Some(match final_result_text.take() {
                                        Some(existing) => format!("{existing}\n{text}"),
                                        None => text.clone(),
                                    });
                                    emitted.push(WorkspaceEvent::Message {
                                        timestamp: Utc::now().to_rfc3339(),
                                        workspace_id: self.runtime.spec().id.clone(),
                                        role: "assistant".to_string(),
                                        text,
                                        visibility: Some(
                                            multi_agent_protocol::WorkspaceVisibility::Public,
                                        ),
                                        member_id: Some(role.id.clone()),
                                        session_id: session_id.clone(),
                                        parent_tool_use_id: None,
                                    });
                                }
                                ClaudeContent::Thinking { thinking } => {
                                    emitted.extend(
                                        self.runtime
                                            .progress_dispatch(
                                                &current_task_id(
                                                    &self.role_session_ids,
                                                    &role.id,
                                                    dispatch,
                                                ),
                                                "thinking",
                                                Some(thinking),
                                                Some("Thinking".to_string()),
                                            )?
                                            .emitted,
                                    );
                                }
                                ClaudeContent::Other => {}
                            }
                        }
                    }
                    ClaudeJsonEvent::Result {
                        subtype,
                        is_error,
                        result,
                        session_id,
                    } => {
                        if is_error || subtype != "success" {
                            turn_failed = Some(result);
                            continue;
                        }

                        if let Some(session_id) = session_id {
                            self.role_session_ids
                                .insert(role.id.clone(), session_id.clone());
                        }

                        let provider_task_id = self
                            .role_session_ids
                            .get(&role.id)
                            .cloned()
                            .unwrap_or_else(|| dispatch.dispatch_id.to_string());

                        emitted.extend(
                            self.runtime
                                .complete_dispatch(
                                    &provider_task_id,
                                    DispatchStatus::Completed,
                                    None,
                                    "Claude completed the turn.".to_string(),
                                )?
                                .emitted,
                        );

                        let result_text = final_result_text
                            .clone()
                            .filter(|text| !text.trim().is_empty())
                            .unwrap_or(result);
                        emitted.extend(
                            self.runtime
                                .attach_result_text(&provider_task_id, result_text)?
                                .emitted,
                        );
                    }
                    ClaudeJsonEvent::User => {}
                    ClaudeJsonEvent::RateLimitEvent => {}
                    ClaudeJsonEvent::System { .. } => {}
                }
            }

            Ok::<_, ClaudeAdapterError>((emitted, turn_failed))
        };

        let (emitted, turn_failed) =
            match timeout(self.options.turn_timeout, processing).await {
                Ok(result) => result?,
                Err(_) => {
                    let _ = child.kill().await;
                    stderr_task.abort();
                    return Err(ClaudeAdapterError::TimedOut {
                        timeout: self.options.turn_timeout,
                        debug: render_debug_context(
                            &command_line,
                            &prompt,
                            effective_working_directory.as_ref(),
                            &stdout_tail,
                            &stderr_tail,
                        ),
                    });
                }
            };

        let status = child.wait().await?;
        let stderr_lines = stderr_task.await??;
        if !status.success() {
            return Err(ClaudeAdapterError::Process(stderr_lines.join("\n")));
        }
        if let Some(message) = turn_failed {
            return Err(ClaudeAdapterError::TurnFailed(message));
        }

        let snapshot = self.runtime.snapshot();
        let final_dispatch = snapshot
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after claude turn");

        Ok(ClaudeRoleTaskRun {
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
enum ClaudeJsonEvent {
    #[serde(rename = "system")]
    System {
        subtype: String,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        tools: Option<Vec<String>>,
    },
    #[serde(rename = "assistant")]
    Assistant {
        message: ClaudeAssistantMessage,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(rename = "user")]
    User,
    #[serde(rename = "rate_limit_event")]
    RateLimitEvent,
    #[serde(rename = "result")]
    Result {
        subtype: String,
        is_error: bool,
        result: String,
        #[serde(default)]
        session_id: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct ClaudeAssistantMessage {
    content: Vec<ClaudeContent>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeContent {
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(other)]
    Other,
}

fn summarize_tool_input(name: &str, input: &Value) -> String {
    match name {
        "Bash" => input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("bash")
            .to_string(),
        "Write" | "Edit" | "Read" => input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string(),
        "WebSearch" => input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string(),
        "Task" | "TeamCreate" => input
            .get("prompt")
            .or_else(|| input.get("message"))
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string(),
        _ => name.to_string(),
    }
}

fn current_task_id(
    role_session_ids: &BTreeMap<String, String>,
    role_id: &str,
    dispatch: &TaskDispatch,
) -> String {
    role_session_ids
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
        "command: {command_line}\nworking_directory: {}\nprompt_preview:\n{prompt_preview}\nstdout_tail:\n{stdout_tail}\nstderr_tail:\n{stderr_tail}",
        working_directory
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
    )
}
