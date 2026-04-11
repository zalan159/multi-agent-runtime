use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;
use multi_agent_protocol::{
    direct_workspace_turn_plan, instantiate_workspace, plan_workspace_turn, ClaimStatus,
    DispatchStatus, RoleSpec, RoleTaskRequest, TaskDispatch, WorkspaceEvent,
    WorkspaceInstanceParams, WorkspaceProfile, WorkspaceSpec, WorkspaceState, WorkspaceTemplate,
    WorkspaceTurnPlan, WorkspaceTurnRequest, WorkspaceVisibility,
};
use multi_agent_runtime_core::{RuntimeError, RuntimeTick, WorkspaceRuntime};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedRole {
    pub role_id: String,
    pub agent_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrappedWorkspace {
    pub workspace_persona_id: String,
    pub workspace_session_id: String,
    pub roles: Vec<ProvisionedRole>,
}

#[async_trait]
pub trait WorkspaceProvisioner: Send + Sync {
    async fn create_workspace_persona(&self, spec: &WorkspaceSpec) -> Result<(String, String), AdapterError>;
    async fn create_role_agent(&self, spec: &WorkspaceSpec, role: &RoleSpec) -> Result<String, AdapterError>;
    async fn spawn_role_session(
        &self,
        spec: &WorkspaceSpec,
        role: &RoleSpec,
        agent_id: &str,
        workspace_persona_id: &str,
    ) -> Result<String, AdapterError>;
}

#[async_trait]
pub trait SessionMessenger: Send + Sync {
    async fn send_to_session(&self, session_id: &str, message: &str) -> Result<(), AdapterError>;
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("provisioning error: {0}")]
    Provisioning(String),
    #[error("messaging error: {0}")]
    Messaging(String),
    #[error("missing provisioned role session for role {0}")]
    MissingRoleSession(String),
}

pub struct CtenoWorkspaceAdapter<P, M> {
    runtime: WorkspaceRuntime,
    provisioner: P,
    messenger: M,
    bootstrapped: Option<BootstrappedWorkspace>,
    role_sessions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceTurnResult {
    pub request: WorkspaceTurnRequest,
    pub plan: WorkspaceTurnPlan,
    pub role_id: Option<String>,
    pub session_id: String,
    pub dispatch: Option<TaskDispatch>,
    pub dispatches: Vec<TaskDispatch>,
    pub events: Vec<WorkspaceEvent>,
    pub state: WorkspaceState,
}

impl<P, M> CtenoWorkspaceAdapter<P, M>
where
    P: WorkspaceProvisioner,
    M: SessionMessenger,
{
    pub fn new(spec: WorkspaceSpec, provisioner: P, messenger: M) -> Self {
        Self {
            runtime: WorkspaceRuntime::new(spec),
            provisioner,
            messenger,
            bootstrapped: None,
            role_sessions: BTreeMap::new(),
        }
    }

    pub fn from_template(
        template: &WorkspaceTemplate,
        instance: &WorkspaceInstanceParams,
        profile: &WorkspaceProfile,
        provisioner: P,
        messenger: M,
    ) -> Self {
        Self::new(
            instantiate_workspace(template, instance, profile),
            provisioner,
            messenger,
        )
    }

    pub fn runtime(&self) -> &WorkspaceRuntime {
        &self.runtime
    }

    pub fn bootstrapped(&self) -> Option<&BootstrappedWorkspace> {
        self.bootstrapped.as_ref()
    }

    pub fn snapshot(&self) -> WorkspaceState {
        self.runtime.snapshot()
    }

    pub fn history(&self) -> &[WorkspaceEvent] {
        self.runtime.history()
    }

    pub fn has_role_session(&self, session_id: &str) -> bool {
        self.role_sessions.values().any(|value| value == session_id)
    }

    pub async fn bootstrap(&mut self) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let spec = self.runtime.spec().clone();
        let mut emitted = Vec::new();

        emitted.extend(self.runtime.start().emitted);

        let (workspace_persona_id, workspace_session_id) = self
            .provisioner
            .create_workspace_persona(&spec)
            .await?;

        let mut roles = Vec::new();
        for role in &spec.roles {
            let agent_id = self.provisioner.create_role_agent(&spec, role).await?;
            let session_id = self
                .provisioner
                .spawn_role_session(&spec, role, &agent_id, &workspace_persona_id)
                .await?;
            self.role_sessions.insert(role.id.clone(), session_id.clone());
            emitted.extend(
                self.runtime
                    .register_member_session(&role.id, session_id.clone())?
                    .emitted,
            );
            roles.push(ProvisionedRole {
                role_id: role.id.clone(),
                agent_id,
                session_id,
            });
        }

        self.bootstrapped = Some(BootstrappedWorkspace {
            workspace_persona_id,
            workspace_session_id: workspace_session_id.clone(),
            roles,
        });

        emitted.extend(
            self.runtime
                .initialize(
                    Some(workspace_session_id),
                    spec.roles.iter().map(|role| role.id.clone()).collect(),
                    spec.allowed_tools.clone().unwrap_or_default(),
                    None,
                )
                .emitted,
        );

        Ok(emitted)
    }

    pub fn restore_existing(
        &mut self,
        bootstrapped: BootstrappedWorkspace,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let spec = self.runtime.spec().clone();
        let mut emitted = self.runtime.start().emitted;

        for role in &bootstrapped.roles {
            self.role_sessions
                .insert(role.role_id.clone(), role.session_id.clone());
            emitted.extend(
                self.runtime
                    .register_member_session(&role.role_id, role.session_id.clone())?
                    .emitted,
            );
        }

        emitted.extend(
            self.runtime
                .initialize(
                    Some(bootstrapped.workspace_session_id.clone()),
                    spec.roles.iter().map(|role| role.id.clone()).collect(),
                    spec.allowed_tools.clone().unwrap_or_default(),
                    None,
                )
                .emitted,
        );

        self.bootstrapped = Some(bootstrapped);
        Ok(emitted)
    }

    pub async fn assign_role_task(
        &mut self,
        request: RoleTaskRequest,
    ) -> Result<(multi_agent_protocol::TaskDispatch, Vec<WorkspaceEvent>), AdapterError> {
        let role_id = request.role_id.clone();
        let role_session_id = self
            .role_sessions
            .get(&role_id)
            .cloned()
            .ok_or_else(|| AdapterError::MissingRoleSession(role_id.clone()))?;

        let (dispatch, queued_tick) = self.runtime.queue_dispatch(request)?;
        self.messenger
            .send_to_session(&role_session_id, &dispatch.instruction)
            .await?;

        Ok((dispatch, queued_tick.emitted))
    }

    pub async fn send_workspace_turn(
        &mut self,
        message: &str,
        role_id: Option<&str>,
    ) -> Result<WorkspaceTurnResult, AdapterError> {
        let request = WorkspaceTurnRequest {
            message: message.to_string(),
            visibility: Some(WorkspaceVisibility::Public),
            max_assignments: None,
            prefer_role_id: role_id.map(ToString::to_string),
        };
        let mut events = self.runtime.publish_user_message(message).emitted;
        let plan = if let Some(role_id) = role_id {
            direct_workspace_turn_plan(self.runtime.spec(), &request, role_id)
        } else {
            plan_workspace_turn(self.runtime.spec(), &request)
        };

        if !plan.response_text.trim().is_empty() {
            events.extend(
                self.runtime
                    .record_role_message(
                        &plan.coordinator_role_id,
                        plan.response_text.clone(),
                        WorkspaceVisibility::Public,
                        None,
                        None,
                    )?
                    .emitted,
            );
        }

        let Some(primary_assignment) = plan.assignments.first().cloned() else {
            let session_id = self
                .bootstrapped
                .as_ref()
                .map(|bootstrapped| bootstrapped.workspace_session_id.clone())
                .ok_or_else(|| {
                    AdapterError::Messaging(
                        "workspace has no coordinator role or workspace session".to_string(),
                    )
                })?;
            self.messenger.send_to_session(&session_id, message).await?;
            return Ok(WorkspaceTurnResult {
                request,
                plan,
                role_id: None,
                session_id,
                dispatch: None,
                dispatches: Vec::new(),
                events,
                state: self.runtime.snapshot(),
            });
        };

        let role_session_id = self
            .role_sessions
            .get(&primary_assignment.role_id)
            .cloned()
            .ok_or_else(|| AdapterError::MissingRoleSession(primary_assignment.role_id.clone()))?;

        let mut dispatches = Vec::new();
        for assignment in &plan.assignments {
            let role_session_id = self
                .role_sessions
                .get(&assignment.role_id)
                .cloned()
                .ok_or_else(|| AdapterError::MissingRoleSession(assignment.role_id.clone()))?;
            let (dispatch, queued_events) = self
                .assign_role_task(RoleTaskRequest {
                    role_id: assignment.role_id.clone(),
                    instruction: assignment.instruction.clone(),
                    summary: assignment
                        .summary
                        .clone()
                        .or_else(|| Some(summarize_workspace_message(&assignment.instruction))),
                    visibility: assignment.visibility.or(Some(WorkspaceVisibility::Public)),
                    source_role_id: Some(plan.coordinator_role_id.clone()),
                })
                .await?;
            events.extend(queued_events);
            let claim_note = if role_id.is_some() {
                Some("Directly addressed by user".to_string())
            } else {
                Some("Claimed by coordinator routing".to_string())
            };
            events.extend(
                self.runtime
                    .claim_dispatch(
                        dispatch.dispatch_id,
                        &assignment.role_id,
                        ClaimStatus::Claimed,
                        claim_note,
                    )?
                    .emitted,
            );

            let synthetic_task_id = format!("cteno:{}:{}", role_session_id, dispatch.dispatch_id);
            let description = dispatch
                .summary
                .clone()
                .unwrap_or_else(|| dispatch.instruction.clone());
            events.extend(self.start_provider_task(&synthetic_task_id, &description, None)?);

            let final_dispatch = self
                .runtime
                .snapshot()
                .dispatches
                .get(&dispatch.dispatch_id)
                .cloned()
                .unwrap_or(dispatch);
            dispatches.push(final_dispatch);
        }

        Ok(WorkspaceTurnResult {
            request,
            plan,
            role_id: Some(primary_assignment.role_id),
            session_id: role_session_id,
            dispatch: dispatches.first().cloned(),
            dispatches,
            events,
            state: self.runtime.snapshot(),
        })
    }

    pub fn start_provider_task(
        &mut self,
        provider_task_id: &str,
        description: &str,
        tool_use_id: Option<String>,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        Ok(self
            .runtime
            .start_next_dispatch(provider_task_id.to_string(), description.to_string(), tool_use_id)?
            .1
            .emitted)
    }

    pub fn progress_provider_task(
        &mut self,
        provider_task_id: &str,
        description: &str,
        summary: Option<String>,
        last_tool_name: Option<String>,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        Ok(self
            .runtime
            .progress_dispatch(provider_task_id, description.to_string(), summary, last_tool_name)?
            .emitted)
    }

    pub fn complete_provider_task(
        &mut self,
        provider_task_id: &str,
        status: DispatchStatus,
        output_file: Option<String>,
        summary: &str,
        result_text: Option<String>,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let mut emitted = self
            .runtime
            .complete_dispatch(provider_task_id, status, output_file, summary.to_string())?
            .emitted;

        if let Some(result_text) = result_text {
            emitted.extend(
                self.runtime
                    .attach_result_text(provider_task_id, result_text)?
                    .emitted,
            );
        }

        Ok(emitted)
    }

    pub fn record_message(&mut self, role: &str, text: &str) -> RuntimeTick {
        self.runtime
            .record_role_message(role, text, WorkspaceVisibility::Public, None, None)
            .unwrap_or_else(|_| RuntimeTick {
                state: self.runtime.snapshot(),
                emitted: vec![WorkspaceEvent::Error {
                    timestamp: Utc::now().to_rfc3339(),
                    workspace_id: self.runtime.spec().id.clone(),
                    error: format!("failed to record message for role {}", role),
                }],
            })
    }

    pub fn ingest_member_response(
        &mut self,
        session_id: &str,
        response_text: &str,
        success: bool,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let Some(role_id) = self
            .role_sessions
            .iter()
            .find_map(|(role_id, value)| (value == session_id).then_some(role_id.clone()))
        else {
            return Ok(Vec::new());
        };

        let active_dispatch = self
            .runtime
            .snapshot()
            .dispatches
            .values()
            .filter(|dispatch| {
                dispatch.role_id == role_id
                    && matches!(
                        dispatch.status,
                        DispatchStatus::Queued | DispatchStatus::Started | DispatchStatus::Running
                    )
            })
            .filter(|dispatch| {
                dispatch
                    .provider_task_id
                    .as_deref()
                    .map(|task_id| task_id.starts_with(&format!("cteno:{}:", session_id)))
                    .unwrap_or(false)
            })
            .max_by(|left, right| left.created_at.cmp(&right.created_at))
            .cloned();

        let summary = summarize_workspace_message(response_text);
        if let Some(dispatch) = active_dispatch {
            let provider_task_id = dispatch.provider_task_id.unwrap_or_else(|| {
                format!("cteno:{}:{}", session_id, dispatch.dispatch_id)
            });
            return self.complete_provider_task(
                &provider_task_id,
                if success {
                    DispatchStatus::Completed
                } else {
                    DispatchStatus::Failed
                },
                dispatch.output_file,
                &summary,
                Some(response_text.to_string()),
            );
        }

        Ok(self.record_message(&role_id, &summary).emitted)
    }
}

fn summarize_workspace_message(message: &str) -> String {
    const MAX_LEN: usize = 120;
    let trimmed = message.trim().replace('\n', " ");
    if trimmed.chars().count() <= MAX_LEN {
        return trimmed;
    }
    let mut out = String::with_capacity(MAX_LEN + 1);
    for ch in trimmed.chars().take(MAX_LEN.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use multi_agent_protocol::{
        create_claude_workspace_profile, create_coding_studio_template, MultiAgentProvider,
        RoleAgentSpec, RoleSpec, RoleTaskRequest, WorkspaceInstanceParams, WorkspaceSpec,
    };

    use super::*;

    #[derive(Clone, Default)]
    struct FakeProvisioner {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl WorkspaceProvisioner for FakeProvisioner {
        async fn create_workspace_persona(&self, spec: &WorkspaceSpec) -> Result<(String, String), AdapterError> {
            self.calls.lock().unwrap().push(format!("persona:{}", spec.id));
            Ok(("persona-1".to_string(), "session-main".to_string()))
        }

        async fn create_role_agent(&self, _spec: &WorkspaceSpec, role: &RoleSpec) -> Result<String, AdapterError> {
            self.calls.lock().unwrap().push(format!("agent:{}", role.id));
            Ok(format!("agent-{}", role.id))
        }

        async fn spawn_role_session(
            &self,
            _spec: &WorkspaceSpec,
            role: &RoleSpec,
            _agent_id: &str,
            _workspace_persona_id: &str,
        ) -> Result<String, AdapterError> {
            self.calls.lock().unwrap().push(format!("session:{}", role.id));
            Ok(format!("session-{}", role.id))
        }
    }

    #[derive(Clone, Default)]
    struct FakeMessenger {
        sent: Arc<Mutex<Vec<(String, String)>>>,
    }

    #[async_trait]
    impl SessionMessenger for FakeMessenger {
        async fn send_to_session(&self, session_id: &str, message: &str) -> Result<(), AdapterError> {
            self.sent
                .lock()
                .unwrap()
                .push((session_id.to_string(), message.to_string()));
            Ok(())
        }
    }

    fn sample_spec() -> WorkspaceSpec {
        WorkspaceSpec {
            id: "workspace-1".to_string(),
            name: "Cteno Workspace".to_string(),
            provider: MultiAgentProvider::Cteno,
            model: "claude-sonnet-4-5".to_string(),
            cwd: Some("/tmp/workspace".to_string()),
            orchestrator_prompt: None,
            allowed_tools: Some(vec!["Read".to_string(), "Edit".to_string()]),
            disallowed_tools: None,
            permission_mode: None,
            setting_sources: None,
            default_role_id: Some("coder".to_string()),
            coordinator_role_id: Some("coder".to_string()),
            claim_policy: None,
            activity_policy: None,
            roles: vec![RoleSpec {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: Some("Implements changes".to_string()),
                direct: Some(true),
                output_root: Some("40-code/".to_string()),
                agent: RoleAgentSpec {
                    description: "Writes code".to_string(),
                    prompt: "Implement the requested change".to_string(),
                    tools: Some(vec!["Read".to_string(), "Edit".to_string()]),
                    disallowed_tools: None,
                    model: None,
                    skills: Some(vec!["flow".to_string()]),
                    mcp_servers: None,
                    initial_prompt: None,
                    permission_mode: None,
                },
            }],
        }
    }

    #[tokio::test]
    async fn bootstrap_and_assign_role_task() {
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();
        let sent = messenger.sent.clone();

        let mut adapter = CtenoWorkspaceAdapter::new(sample_spec(), provisioner, messenger);
        let bootstrap_events = adapter.bootstrap().await.expect("bootstrap should succeed");
        assert!(bootstrap_events.len() >= 2);
        assert!(adapter.bootstrapped().is_some());

        let (dispatch, queued_events) = adapter
            .assign_role_task(RoleTaskRequest {
                role_id: "coder".to_string(),
                instruction: "Implement group mentions".to_string(),
                summary: Some("Mention MVP".to_string()),
                visibility: None,
                source_role_id: None,
            })
            .await
            .expect("assign role task should succeed");

        assert_eq!(dispatch.role_id, "coder");
        assert_eq!(queued_events.len(), 2);
        assert!(matches!(
            queued_events.first(),
            Some(WorkspaceEvent::DispatchQueued { .. })
        ));

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-coder");
        assert_eq!(sent[0].1, "Implement group mentions");
    }

    #[tokio::test]
    async fn bootstrap_from_template_constructor() {
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();

        let template = create_coding_studio_template();
        let instance = WorkspaceInstanceParams {
            id: "template-workspace".to_string(),
            name: "Template Workspace".to_string(),
            cwd: Some("/tmp/template".to_string()),
        };
        let profile = create_claude_workspace_profile(None);

        let mut adapter = CtenoWorkspaceAdapter::from_template(
            &template,
            &instance,
            &profile,
            provisioner,
            messenger,
        );
        let events = adapter.bootstrap().await.expect("bootstrap should succeed");

        assert!(events.len() >= 2);
        assert_eq!(adapter.runtime().spec().default_role_id.as_deref(), Some("pm"));
        assert_eq!(adapter.runtime().spec().provider, MultiAgentProvider::ClaudeAgentSdk);
        assert_eq!(adapter.bootstrapped().unwrap().roles.len(), 6);
    }

    #[tokio::test]
    async fn workspace_turn_publishes_user_message_and_routes_to_default_role() {
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();
        let sent = messenger.sent.clone();

        let mut adapter = CtenoWorkspaceAdapter::new(sample_spec(), provisioner, messenger);
        adapter.bootstrap().await.expect("bootstrap should succeed");

        let result = adapter
            .send_workspace_turn("Please implement group mentions", None)
            .await
            .expect("workspace turn should succeed");

        assert_eq!(result.request.message, "Please implement group mentions");
        assert_eq!(result.role_id.as_deref(), Some("coder"));
        assert_eq!(result.session_id, "session-coder");
        assert!(result.dispatch.is_some());
        assert_eq!(result.plan.assignments.len(), 1);
        assert_eq!(result.plan.assignments[0].role_id, "coder");
        assert!(result.events.iter().any(|event| matches!(
            event,
            WorkspaceEvent::ActivityPublished { activity, .. }
                if activity.kind == multi_agent_protocol::WorkspaceActivityKind::UserMessage
        )));
        assert!(result.events.iter().any(|event| matches!(
            event,
            WorkspaceEvent::ActivityPublished { activity, .. }
                if activity.kind == multi_agent_protocol::WorkspaceActivityKind::CoordinatorMessage
        )));
        assert!(result.events.iter().any(|event| matches!(
            event,
            WorkspaceEvent::DispatchClaimed { .. }
        )));
        assert!(result.events.iter().any(|event| matches!(
            event,
            WorkspaceEvent::DispatchStarted { .. }
        )));

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-coder");
        assert_eq!(sent[0].1, "Please implement group mentions");
    }

    #[tokio::test]
    async fn workspace_turn_honors_direct_role_targeting() {
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();
        let sent = messenger.sent.clone();

        let template = create_coding_studio_template();
        let instance = WorkspaceInstanceParams {
            id: "workspace-turn-template".to_string(),
            name: "Workspace Turn Template".to_string(),
            cwd: Some("/tmp/template".to_string()),
        };
        let profile = create_claude_workspace_profile(None);
        let mut adapter = CtenoWorkspaceAdapter::from_template(
            &template,
            &instance,
            &profile,
            provisioner,
            messenger,
        );
        adapter.bootstrap().await.expect("bootstrap should succeed");

        let result = adapter
            .send_workspace_turn("Write the PRD for group mentions", Some("prd"))
            .await
            .expect("workspace turn should succeed");

        assert_eq!(result.plan.assignments.len(), 1);
        assert_eq!(result.plan.assignments[0].role_id, "prd");
        assert_eq!(result.role_id.as_deref(), Some("prd"));
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-prd");
        assert_eq!(sent[0].1, "Write the PRD for group mentions");
    }
}
