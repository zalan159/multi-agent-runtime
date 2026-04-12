use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;
use multi_agent_protocol::{
    build_workflow_entry_plan, decide_coordinator_action, direct_workspace_turn_plan,
    instantiate_workspace, plan_workspace_turn, resolve_workflow_vote_candidate_role_ids,
    should_approve_workflow_vote, synthesize_workflow_vote_response, ClaimStatus, DispatchStatus,
    RoleSpec, RoleTaskRequest, TaskDispatch, WorkspaceEvent, WorkspaceInstanceParams,
    WorkspaceProfile, WorkspaceSpec, WorkspaceState, WorkspaceTemplate, WorkspaceTurnPlan,
    WorkspaceTurnRequest, WorkspaceVisibility, WorkspaceWorkflowVoteResponse,
    WorkspaceWorkflowVoteWindow,
};
use multi_agent_runtime_core::{RuntimeError, RuntimeTick, WorkspaceRuntime};
use multi_agent_runtime_local::{LocalPersistenceError, LocalWorkspacePersistence, PersistedProviderBinding, PersistedProviderState, ProviderConversationKind};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvisionedRole {
    pub role_id: String,
    pub agent_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrappedWorkspace {
    pub workspace_persona_id: String,
    pub workspace_session_id: String,
    pub roles: Vec<ProvisionedRole>,
}

#[async_trait]
pub trait WorkspaceProvisioner: Send + Sync {
    async fn prepare_workspace_layout(&self, spec: &WorkspaceSpec) -> Result<(), AdapterError>;
    async fn create_workspace_persona(&self, spec: &WorkspaceSpec) -> Result<(String, String), AdapterError>;
    async fn create_role_agent(&self, spec: &WorkspaceSpec, role: &RoleSpec) -> Result<String, AdapterError>;
    async fn spawn_role_session(
        &self,
        spec: &WorkspaceSpec,
        role: &RoleSpec,
        agent_id: &str,
        workspace_persona_id: &str,
    ) -> Result<String, AdapterError>;
    async fn cleanup_workspace(
        &self,
        spec: &WorkspaceSpec,
        bootstrapped: &BootstrappedWorkspace,
    ) -> Result<(), AdapterError>;
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
    #[error("local persistence error: {0}")]
    LocalPersistence(#[from] LocalPersistenceError),
    #[error("provider metadata error: {0}")]
    Metadata(String),
}

pub struct CtenoWorkspaceAdapter<P, M> {
    runtime: WorkspaceRuntime,
    provisioner: P,
    messenger: M,
    bootstrapped: Option<BootstrappedWorkspace>,
    role_sessions: BTreeMap<String, String>,
    persistence: Option<LocalWorkspacePersistence>,
    restored_from_persistence: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CtenoProviderMetadata {
    workspace_persona_id: String,
    workspace_session_id: String,
    roles: Vec<ProvisionedRole>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceTurnResult {
    pub request: WorkspaceTurnRequest,
    pub plan: WorkspaceTurnPlan,
    pub workflow_vote_window: Option<WorkspaceWorkflowVoteWindow>,
    pub workflow_vote_responses: Vec<WorkspaceWorkflowVoteResponse>,
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
        let persistence = LocalWorkspacePersistence::from_spec(&spec).ok();
        Self {
            runtime: WorkspaceRuntime::new(spec),
            provisioner,
            messenger,
            bootstrapped: None,
            role_sessions: BTreeMap::new(),
            persistence,
            restored_from_persistence: false,
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

    pub fn restore_from_local(
        cwd: impl AsRef<std::path::Path>,
        workspace_id: &str,
        provisioner: P,
        messenger: M,
    ) -> Result<Self, AdapterError> {
        let persistence = LocalWorkspacePersistence::from_workspace(cwd, workspace_id);
        let spec = persistence.load_workspace_spec()?;
        let state = persistence.load_workspace_state()?;
        let history = persistence.load_events()?;
        let provider_state = persistence.load_provider_state()?;

        let mut adapter = Self::new(spec, provisioner, messenger);
        adapter.runtime.restore_snapshot(state, history);
        adapter.role_sessions = provider_state
            .member_bindings
            .iter()
            .map(|(role_id, binding)| (role_id.clone(), binding.provider_conversation_id.clone()))
            .collect();
        if let Some(metadata) = provider_state.metadata {
            let decoded: CtenoProviderMetadata = serde_json::from_value(metadata)
                .map_err(|error| AdapterError::Metadata(error.to_string()))?;
            adapter.bootstrapped = Some(BootstrappedWorkspace {
                workspace_persona_id: decoded.workspace_persona_id,
                workspace_session_id: decoded.workspace_session_id,
                roles: decoded.roles,
            });
        }
        adapter.restored_from_persistence = true;
        Ok(adapter)
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

    pub fn persistence_root(&self) -> Option<&std::path::Path> {
        self.persistence.as_ref().map(|p| p.root())
    }

    pub fn has_role_session(&self, session_id: &str) -> bool {
        self.role_sessions.values().any(|value| value == session_id)
    }

    pub async fn bootstrap(&mut self) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let spec = self.runtime.spec().clone();
        let mut emitted = Vec::new();

        if !self.restored_from_persistence {
            if let Some(persistence) = self.persistence.as_ref() {
                persistence.ensure_workspace_initialized(&spec)?;
            }
        }

        self.provisioner.prepare_workspace_layout(&spec).await?;

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

        self.persist_runtime(&emitted)?;

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
        self.persist_runtime(&emitted)?;
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
        self.persist_runtime(&queued_tick.emitted)?;

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
        let coordinator_decision = if let Some(role_id) = role_id {
            multi_agent_protocol::CoordinatorWorkflowDecision {
                kind: multi_agent_protocol::CoordinatorDecisionKind::Delegate,
                response_text: format!("@{} will take this next.", role_id),
                target_role_id: Some(role_id.to_string()),
                workflow_vote_reason: None,
                rationale: Some("Direct role targeting bypassed coordinator routing.".to_string()),
            }
        } else {
            decide_coordinator_action(self.runtime.spec(), &request)
        };

        if !coordinator_decision.response_text.trim().is_empty() {
            events.extend(
                self.runtime
                    .record_role_message(
                        &self
                            .runtime
                            .spec()
                            .coordinator_role_id
                            .clone()
                            .or_else(|| self.runtime.spec().default_role_id.clone())
                            .unwrap_or_else(|| "coordinator".to_string()),
                        coordinator_decision.response_text.clone(),
                        WorkspaceVisibility::Public,
                        None,
                        None,
                    )?
                    .emitted,
            );
        }

        let mut workflow_vote_window = None;
        let mut workflow_vote_responses = Vec::new();
        let plan = match coordinator_decision.kind {
            multi_agent_protocol::CoordinatorDecisionKind::Respond => WorkspaceTurnPlan {
                coordinator_role_id: self
                    .runtime
                    .spec()
                    .coordinator_role_id
                    .clone()
                    .or_else(|| self.runtime.spec().default_role_id.clone())
                    .unwrap_or_else(|| "coordinator".to_string()),
                response_text: coordinator_decision.response_text.clone(),
                assignments: Vec::new(),
                rationale: coordinator_decision.rationale.clone(),
            },
            multi_agent_protocol::CoordinatorDecisionKind::Delegate => {
                if let Some(target_role_id) = coordinator_decision.target_role_id.clone() {
                    direct_workspace_turn_plan(self.runtime.spec(), &request, &target_role_id)
                } else {
                    plan_workspace_turn(self.runtime.spec(), &request)
                }
            }
            multi_agent_protocol::CoordinatorDecisionKind::ProposeWorkflow => {
                let candidate_role_ids =
                    resolve_workflow_vote_candidate_role_ids(self.runtime.spec());
                let vote_tick = self.runtime.open_workflow_vote_window(
                    request.clone(),
                    coordinator_decision.clone(),
                    candidate_role_ids.clone(),
                );
                events.extend(vote_tick.emitted);
                let vote_window = vote_tick.state.workflow_runtime.active_vote_window.clone();
                workflow_vote_window = vote_window.clone();
                for role_id in candidate_role_ids {
                    if let Some(role) = self.runtime.spec().roles.iter().find(|role| role.id == role_id) {
                        let response = synthesize_workflow_vote_response(
                            self.runtime.spec(),
                            &request,
                            &coordinator_decision,
                            role,
                        );
                        workflow_vote_responses.push(response.clone());
                        if let Some(vote_window) = vote_window.as_ref() {
                            events.extend(
                                self.runtime
                                    .record_workflow_vote_response(vote_window, response)?
                                    .emitted,
                            );
                        }
                    }
                }
                let approved =
                    should_approve_workflow_vote(self.runtime.spec(), &workflow_vote_responses);
                if let Some(vote_window) = vote_window.clone() {
                    events.extend(
                        self.runtime
                            .close_workflow_vote_window(
                                vote_window.clone(),
                                coordinator_decision.clone(),
                                workflow_vote_responses.clone(),
                                approved,
                            )
                            .emitted,
                    );
                }
                if approved {
                    let plan = build_workflow_entry_plan(self.runtime.spec(), &request);
                    let first_assignment = plan.assignments.first();
                    events.extend(
                        self.runtime
                            .start_workflow(
                                coordinator_decision.clone(),
                                workflow_vote_window.clone(),
                                Some(request.message.clone()),
                                first_assignment.and_then(|assignment| assignment.workflow_node_id.clone()),
                                first_assignment.and_then(|assignment| assignment.stage_id.clone()),
                            )
                            .emitted,
                    );
                    plan
                } else {
                    WorkspaceTurnPlan {
                        coordinator_role_id: self
                            .runtime
                            .spec()
                            .coordinator_role_id
                            .clone()
                            .or_else(|| self.runtime.spec().default_role_id.clone())
                            .unwrap_or_else(|| "coordinator".to_string()),
                        response_text: coordinator_decision.response_text.clone(),
                        assignments: Vec::new(),
                        rationale: Some("Workflow vote rejected; staying in group chat mode.".to_string()),
                    }
                }
            }
        };

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
            let result = WorkspaceTurnResult {
                request,
                plan,
                workflow_vote_window,
                workflow_vote_responses,
                role_id: None,
                session_id,
                dispatch: None,
                dispatches: Vec::new(),
                events,
                state: self.runtime.snapshot(),
            };
            self.persist_runtime(&result.events)?;
            return Ok(result);
        };

        let role_session_id = self
            .role_sessions
            .get(&primary_assignment.role_id)
            .cloned()
            .ok_or_else(|| AdapterError::MissingRoleSession(primary_assignment.role_id.clone()))?;

        let mut dispatches = Vec::new();
        for assignment in &plan.assignments {
            let mut run_dispatches = self
                .dispatch_assignment_chain(
                    RoleTaskRequest {
                        role_id: assignment.role_id.clone(),
                        instruction: assignment.instruction.clone(),
                        summary: assignment
                            .summary
                            .clone()
                            .or_else(|| Some(summarize_workspace_message(&assignment.instruction))),
                        visibility: assignment.visibility.or(Some(WorkspaceVisibility::Public)),
                        source_role_id: Some(plan.coordinator_role_id.clone()),
                        workflow_node_id: assignment.workflow_node_id.clone(),
                        stage_id: assignment.stage_id.clone(),
                    },
                    if role_id.is_some() {
                        Some("Directly addressed by user".to_string())
                    } else {
                        Some("Claimed by coordinator routing".to_string())
                    },
                )
                .await?;
            events.append(&mut run_dispatches.1);
            dispatches.append(&mut run_dispatches.0);
        }

        let result = WorkspaceTurnResult {
            request,
            plan,
            workflow_vote_window,
            workflow_vote_responses,
            role_id: Some(primary_assignment.role_id),
            session_id: role_session_id,
            dispatch: dispatches.first().cloned(),
            dispatches,
            events,
            state: self.runtime.snapshot(),
        };
        self.persist_runtime(&result.events)?;
        Ok(result)
    }

    async fn dispatch_assignment_chain(
        &mut self,
        request: RoleTaskRequest,
        claim_note: Option<String>,
    ) -> Result<(Vec<TaskDispatch>, Vec<WorkspaceEvent>), AdapterError> {
        let dispatch = self.dispatch_assignment_once(request, claim_note).await?;
        Ok((vec![dispatch.0], dispatch.1))
    }

    async fn dispatch_assignment_once(
        &mut self,
        request: RoleTaskRequest,
        claim_note: Option<String>,
    ) -> Result<(TaskDispatch, Vec<WorkspaceEvent>), AdapterError> {
        let role_session_id = self
            .role_sessions
            .get(&request.role_id)
            .cloned()
            .ok_or_else(|| AdapterError::MissingRoleSession(request.role_id.clone()))?;

        let (dispatch, queued_tick) = self.runtime.queue_dispatch(request)?;
        let mut emitted = queued_tick.emitted;

        let should_claim = self
            .runtime
            .snapshot()
            .dispatches
            .get(&dispatch.dispatch_id)
            .and_then(|stored| stored.claim_status)
            != Some(ClaimStatus::Claimed);
        if should_claim {
            emitted.extend(
                self.runtime
                    .claim_dispatch(
                        dispatch.dispatch_id,
                        &dispatch.role_id,
                        ClaimStatus::Claimed,
                        claim_note,
                    )?
                    .emitted,
            );
        }

        self.messenger
            .send_to_session(&role_session_id, &dispatch.instruction)
            .await?;

        let provider_task_id = format!("cteno:{}:{}", role_session_id, dispatch.dispatch_id);
        let (_, started_tick) = self.runtime.start_next_dispatch(
            provider_task_id,
            dispatch
                .summary
                .clone()
                .unwrap_or_else(|| summarize_workspace_message(&dispatch.instruction)),
            None,
        )?;
        emitted.extend(started_tick.emitted);

        let final_dispatch = self
            .runtime
            .snapshot()
            .dispatches
            .get(&dispatch.dispatch_id)
            .cloned()
            .expect("dispatch should exist after start_next_dispatch");

        Ok((final_dispatch, emitted))
    }

    pub fn start_provider_task(
        &mut self,
        provider_task_id: &str,
        description: &str,
        tool_use_id: Option<String>,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let events = self
            .runtime
            .start_next_dispatch(provider_task_id.to_string(), description.to_string(), tool_use_id)?
            .1
            .emitted;
        self.persist_runtime(&events)?;
        Ok(events)
    }

    pub fn progress_provider_task(
        &mut self,
        provider_task_id: &str,
        description: &str,
        summary: Option<String>,
        last_tool_name: Option<String>,
    ) -> Result<Vec<WorkspaceEvent>, AdapterError> {
        let events = self
            .runtime
            .progress_dispatch(provider_task_id, description.to_string(), summary, last_tool_name)?
            .emitted;
        self.persist_runtime(&events)?;
        Ok(events)
    }

    pub async fn complete_provider_task(
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

        let (advance_tick, followups) = self.runtime.advance_workflow_after_dispatch(provider_task_id)?;
        emitted.extend(advance_tick.emitted);

        for followup in followups {
            let (_, mut followup_events) = self
                .dispatch_assignment_chain(
                    followup,
                    Some("Claimed by workflow progression".to_string()),
                )
                .await?;
            emitted.append(&mut followup_events);
        }

        self.persist_runtime(&emitted)?;
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

    pub async fn ingest_member_response(
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
            return self
                .complete_provider_task(
                    &provider_task_id,
                    if success {
                        DispatchStatus::Completed
                    } else {
                        DispatchStatus::Failed
                    },
                    dispatch.output_file,
                    &summary,
                    Some(response_text.to_string()),
                )
                .await;
        }

        let emitted = self.record_message(&role_id, &summary).emitted;
        self.persist_runtime(&emitted)?;
        Ok(emitted)
    }

    pub async fn delete_workspace(&mut self) -> Result<(), AdapterError> {
        if let Some(bootstrapped) = self.bootstrapped.as_ref() {
            self.provisioner
                .cleanup_workspace(self.runtime.spec(), bootstrapped)
                .await?;
        }
        self.role_sessions.clear();
        self.bootstrapped = None;
        if let Some(persistence) = self.persistence.as_ref() {
            persistence.delete_workspace()?;
        }
        Ok(())
    }

    fn build_provider_state(&self) -> PersistedProviderState {
        PersistedProviderState {
            workspace_id: self.runtime.spec().id.clone(),
            provider: multi_agent_protocol::MultiAgentProvider::Cteno,
            root_conversation_id: self
                .bootstrapped
                .as_ref()
                .map(|bootstrapped| bootstrapped.workspace_session_id.clone()),
            member_bindings: self
                .role_sessions
                .iter()
                .map(|(role_id, session_id)| {
                    (
                        role_id.clone(),
                        PersistedProviderBinding {
                            role_id: role_id.clone(),
                            provider_conversation_id: session_id.clone(),
                            kind: ProviderConversationKind::Session,
                            updated_at: Utc::now().to_rfc3339(),
                        },
                    )
                })
                .collect(),
            metadata: self.bootstrapped.as_ref().and_then(|bootstrapped| {
                serde_json::to_value(CtenoProviderMetadata {
                    workspace_persona_id: bootstrapped.workspace_persona_id.clone(),
                    workspace_session_id: bootstrapped.workspace_session_id.clone(),
                    roles: bootstrapped.roles.clone(),
                })
                .ok()
            }),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    fn persist_runtime(&self, events: &[WorkspaceEvent]) -> Result<(), AdapterError> {
        if let Some(persistence) = self.persistence.as_ref() {
            persistence.persist_runtime(&self.runtime.snapshot(), events, &self.build_provider_state())?;
        }
        Ok(())
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
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

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
        async fn prepare_workspace_layout(&self, spec: &WorkspaceSpec) -> Result<(), AdapterError> {
            self.calls.lock().unwrap().push(format!("prepare:{}", spec.id));
            Ok(())
        }

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

        async fn cleanup_workspace(
            &self,
            spec: &WorkspaceSpec,
            _bootstrapped: &BootstrappedWorkspace,
        ) -> Result<(), AdapterError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("cleanup:{}", spec.id));
            Ok(())
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

    fn temp_workspace_dir(label: &str) -> String {
        std::env::temp_dir()
            .join(format!(
                "multi-agent-runtime-cteno-{label}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .to_string_lossy()
            .to_string()
    }

    fn sample_spec() -> WorkspaceSpec {
        WorkspaceSpec {
            id: "workspace-1".to_string(),
            name: "Cteno Workspace".to_string(),
            provider: MultiAgentProvider::Cteno,
            model: "claude-sonnet-4-5".to_string(),
            cwd: Some(temp_workspace_dir("sample")),
            orchestrator_prompt: None,
            allowed_tools: Some(vec!["Read".to_string(), "Edit".to_string()]),
            disallowed_tools: None,
            permission_mode: None,
            setting_sources: None,
            default_role_id: Some("coder".to_string()),
            coordinator_role_id: Some("coder".to_string()),
            claim_policy: None,
            activity_policy: None,
            workflow_vote_policy: None,
            workflow: None,
            artifacts: None,
            completion_policy: None,
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
                workflow_node_id: None,
                stage_id: None,
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

    #[tokio::test]
    async fn restores_and_deletes_local_workspace() {
        let spec = sample_spec();
        let cwd = spec.cwd.clone().unwrap();
        let provisioner = FakeProvisioner::default();
        let messenger = FakeMessenger::default();
        let mut adapter =
            CtenoWorkspaceAdapter::new(spec.clone(), provisioner.clone(), messenger.clone());
        adapter.bootstrap().await.expect("bootstrap should succeed");

        let persisted_root = adapter
            .persistence_root()
            .expect("persistence root should exist")
            .to_path_buf();
        assert!(persisted_root.exists());

        let restored =
            CtenoWorkspaceAdapter::restore_from_local(&cwd, &spec.id, provisioner, messenger)
                .expect("restore should succeed");
        assert!(restored.bootstrapped().is_some());
        assert!(restored.has_role_session("session-coder"));

        let mut restored = restored;
        restored
            .delete_workspace()
            .await
            .expect("delete should succeed");
        assert!(!persisted_root.exists());

        let _ = fs::remove_dir_all(cwd);
    }
}
