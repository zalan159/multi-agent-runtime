use std::collections::{BTreeMap, VecDeque};

use chrono::Utc;
use multi_agent_protocol::{
    instantiate_workspace, ClaimMode, ClaimStatus, DispatchStatus, MemberStatus, RoleTaskRequest,
    TaskDispatch, WorkspaceActivity, WorkspaceActivityKind, WorkspaceEvent, WorkspaceInstanceParams,
    WorkspaceMember, WorkspaceProfile, WorkspaceSpec, WorkspaceState, WorkspaceStatus,
    WorkspaceTemplate, WorkspaceVisibility,
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("unknown role: {0}")]
    UnknownRole(String),
    #[error("unknown dispatch: {0}")]
    UnknownDispatch(Uuid),
    #[error("unknown provider task: {0}")]
    UnknownProviderTask(String),
}

#[derive(Debug, Clone)]
pub struct RuntimeTick {
    pub state: WorkspaceState,
    pub emitted: Vec<WorkspaceEvent>,
}

#[derive(Debug)]
pub struct WorkspaceRuntime {
    spec: WorkspaceSpec,
    state: WorkspaceState,
    pending_dispatch_queue: VecDeque<Uuid>,
    provider_task_to_dispatch: BTreeMap<String, Uuid>,
    history: Vec<WorkspaceEvent>,
}

impl WorkspaceRuntime {
    pub fn new(spec: WorkspaceSpec) -> Self {
        let roles = spec
            .roles
            .iter()
            .cloned()
            .map(|role| (role.id.clone(), role))
            .collect();
        let members = spec
            .roles
            .iter()
            .map(|role| {
                (
                    role.id.clone(),
                    WorkspaceMember {
                        member_id: role.id.clone(),
                        workspace_id: spec.id.clone(),
                        role_id: role.id.clone(),
                        role_name: role.name.clone(),
                        direct: role.direct,
                        session_id: None,
                        status: MemberStatus::Idle,
                        public_state_summary: None,
                        last_activity_at: None,
                    },
                )
            })
            .collect();

        Self {
            state: WorkspaceState {
                workspace_id: spec.id.clone(),
                status: WorkspaceStatus::Idle,
                provider: spec.provider,
                session_id: None,
                started_at: None,
                roles,
                members,
                dispatches: BTreeMap::new(),
                activities: Vec::new(),
            },
            spec,
            pending_dispatch_queue: VecDeque::new(),
            provider_task_to_dispatch: BTreeMap::new(),
            history: Vec::new(),
        }
    }

    pub fn from_template(
        template: &WorkspaceTemplate,
        instance: &WorkspaceInstanceParams,
        profile: &WorkspaceProfile,
    ) -> Self {
        Self::new(instantiate_workspace(template, instance, profile))
    }

    pub fn spec(&self) -> &WorkspaceSpec {
        &self.spec
    }

    pub fn snapshot(&self) -> WorkspaceState {
        self.state.clone()
    }

    pub fn history(&self) -> &[WorkspaceEvent] {
        &self.history
    }

    pub fn start(&mut self) -> RuntimeTick {
        self.state.started_at = Some(now());
        self.state.status = WorkspaceStatus::Running;

        let mut events = vec![WorkspaceEvent::WorkspaceStarted {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            spec: self.spec.clone(),
        }];

        for member in self.state.members.values() {
            events.push(WorkspaceEvent::MemberRegistered {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                member: member.clone(),
            });
        }

        self.push_events(events)
    }

    pub fn initialize(
        &mut self,
        session_id: Option<String>,
        available_agents: Vec<String>,
        available_tools: Vec<String>,
        available_commands: Option<Vec<String>>,
    ) -> RuntimeTick {
        if let Some(session_id) = session_id.clone() {
            self.state.session_id = Some(session_id);
        }

        let event = WorkspaceEvent::WorkspaceInitialized {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            session_id,
            available_agents,
            available_tools,
            available_commands,
        };
        self.push_event(event)
    }

    pub fn register_member_session(
        &mut self,
        role_id: &str,
        session_id: impl Into<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let session_id = session_id.into();
        let member = self
            .state
            .members
            .get_mut(role_id)
            .ok_or_else(|| RuntimeError::UnknownRole(role_id.to_string()))?;
        member.session_id = Some(session_id);
        member.last_activity_at = Some(now());

        let event = WorkspaceEvent::MemberStateChanged {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            member: member.clone(),
        };
        Ok(self.push_event(event))
    }

    pub fn publish_user_message(&mut self, text: impl Into<String>) -> RuntimeTick {
        let text = text.into();
        let visibility = Some(WorkspaceVisibility::Public);
        let events = vec![
            WorkspaceEvent::Message {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                role: "user".to_string(),
                text: text.clone(),
                visibility,
                member_id: None,
                session_id: self.state.session_id.clone(),
                parent_tool_use_id: None,
            },
            WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::UserMessage,
                    text,
                    WorkspaceVisibility::Public,
                    None,
                    None,
                    None,
                    None,
                ),
            },
        ];
        self.push_events(events)
    }

    pub fn record_role_message(
        &mut self,
        role_id: &str,
        text: impl Into<String>,
        visibility: WorkspaceVisibility,
        session_id: Option<String>,
        parent_tool_use_id: Option<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let text = text.into();
        self.ensure_member_exists(role_id)?;
        let mut events = vec![WorkspaceEvent::Message {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            role: role_id.to_string(),
            text: text.clone(),
            visibility: Some(visibility),
            member_id: Some(role_id.to_string()),
            session_id,
            parent_tool_use_id,
        }];

        if visibility != WorkspaceVisibility::Private {
            let kind = if self.spec.coordinator_role_id.as_deref() == Some(role_id) {
                WorkspaceActivityKind::CoordinatorMessage
            } else {
                WorkspaceActivityKind::MemberSummary
            };
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    kind,
                    text.clone(),
                    visibility,
                    Some(role_id.to_string()),
                    Some(role_id.to_string()),
                    None,
                    None,
                ),
            });
        }

        self.update_member(role_id, None, Some(text));
        Ok(self.push_events(events))
    }

    pub fn queue_dispatch(
        &mut self,
        request: RoleTaskRequest,
    ) -> Result<(TaskDispatch, RuntimeTick), RuntimeError> {
        if !self.state.roles.contains_key(&request.role_id) {
            return Err(RuntimeError::UnknownRole(request.role_id));
        }

        let claim_mode = self
            .spec
            .claim_policy
            .as_ref()
            .map(|policy| policy.mode)
            .unwrap_or(ClaimMode::Direct);
        let initial_claim_status = match claim_mode {
            ClaimMode::Direct | ClaimMode::CoordinatorOnly => Some(ClaimStatus::Claimed),
            ClaimMode::Claim => Some(ClaimStatus::Pending),
        };
        let initial_claim_members = if initial_claim_status == Some(ClaimStatus::Claimed) {
            Some(vec![request.role_id.clone()])
        } else {
            None
        };

        let dispatch = TaskDispatch {
            dispatch_id: Uuid::new_v4(),
            workspace_id: self.spec.id.clone(),
            role_id: request.role_id,
            instruction: request.instruction,
            summary: request.summary,
            visibility: request.visibility.or(self.default_visibility()),
            source_role_id: request.source_role_id,
            status: DispatchStatus::Queued,
            provider_task_id: None,
            tool_use_id: None,
            created_at: now(),
            started_at: None,
            completed_at: None,
            output_file: None,
            last_summary: None,
            result_text: None,
            claimed_by_member_ids: initial_claim_members,
            claim_status: initial_claim_status,
        };

        self.pending_dispatch_queue.push_back(dispatch.dispatch_id);
        self.state
            .dispatches
            .insert(dispatch.dispatch_id, dispatch.clone());

        let mut events = vec![WorkspaceEvent::DispatchQueued {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch.clone(),
        }];

        if dispatch.claim_status == Some(ClaimStatus::Claimed) {
            let member = self
                .state
                .members
                .get(&dispatch.role_id)
                .cloned()
                .ok_or_else(|| RuntimeError::UnknownRole(dispatch.role_id.clone()))?;
            events.push(WorkspaceEvent::DispatchClaimed {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                dispatch: dispatch.clone(),
                member,
                claim_status: ClaimStatus::Claimed,
                note: Some("Assigned by policy".to_string()),
            });
        }

        Ok((dispatch, self.push_events(events)))
    }

    pub fn claim_dispatch(
        &mut self,
        dispatch_id: Uuid,
        role_id: &str,
        claim_status: ClaimStatus,
        note: Option<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        self.ensure_member_exists(role_id)?;
        let dispatch = self
            .state
            .dispatches
            .get_mut(&dispatch_id)
            .ok_or(RuntimeError::UnknownDispatch(dispatch_id))?;

        dispatch.claim_status = Some(claim_status);
        let claims = dispatch.claimed_by_member_ids.get_or_insert_with(Vec::new);
        if !claims.iter().any(|member_id| member_id == role_id) {
            claims.push(role_id.to_string());
        }
        let dispatch_snapshot = dispatch.clone();
        let member = self.state.members.get(role_id).cloned().expect("member exists");

        let mut events = vec![WorkspaceEvent::DispatchClaimed {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch_snapshot.clone(),
            member: member.clone(),
            claim_status,
            note: note.clone(),
        }];

        if claim_status != ClaimStatus::Declined {
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::MemberClaimed,
                    note.unwrap_or_else(|| format!("{} claimed the task", member.role_name)),
                    WorkspaceVisibility::Public,
                    Some(role_id.to_string()),
                    Some(role_id.to_string()),
                    Some(dispatch_id),
                    None,
                ),
            });
        }

        Ok(self.push_events(events))
    }

    pub fn start_next_dispatch(
        &mut self,
        provider_task_id: impl Into<String>,
        description: impl Into<String>,
        tool_use_id: Option<String>,
    ) -> Result<(TaskDispatch, RuntimeTick), RuntimeError> {
        let provider_task_id = provider_task_id.into();
        let description = description.into();
        let dispatch_id = self
            .pending_dispatch_queue
            .pop_front()
            .ok_or_else(|| RuntimeError::UnknownProviderTask(provider_task_id.clone()))?;

        let (role_id, dispatch_snapshot) = {
            let dispatch = self
                .state
                .dispatches
                .get_mut(&dispatch_id)
                .ok_or(RuntimeError::UnknownDispatch(dispatch_id))?;

            dispatch.status = DispatchStatus::Started;
            dispatch.provider_task_id = Some(provider_task_id.clone());
            dispatch.tool_use_id = tool_use_id;
            dispatch.started_at = Some(now());
            dispatch.last_summary = Some(description.clone());
            (dispatch.role_id.clone(), dispatch.clone())
        };

        self.provider_task_to_dispatch
            .insert(provider_task_id.clone(), dispatch_id);

        self.update_member(&role_id, Some(MemberStatus::Active), Some(description.clone()));

        let mut events = vec![WorkspaceEvent::DispatchStarted {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch_snapshot.clone(),
            task_id: provider_task_id.clone(),
            description: description.clone(),
        }];

        if self.publish_dispatch_lifecycle() {
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::DispatchStarted,
                    description,
                    dispatch_snapshot.visibility.unwrap_or(WorkspaceVisibility::Public),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.dispatch_id),
                    Some(provider_task_id),
                ),
            });
        }

        Ok((dispatch_snapshot, self.push_events(events)))
    }

    pub fn progress_dispatch(
        &mut self,
        provider_task_id: &str,
        description: impl Into<String>,
        summary: Option<String>,
        last_tool_name: Option<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let description = description.into();
        let dispatch_snapshot = {
            let dispatch = self.find_dispatch_mut(provider_task_id)?;
            dispatch.status = DispatchStatus::Running;
            if let Some(summary) = summary.clone() {
                dispatch.last_summary = Some(summary.clone());
            }
            dispatch.clone()
        };
        self.update_member(
            &dispatch_snapshot.role_id,
            Some(MemberStatus::Active),
            summary.clone().or_else(|| Some(description.clone())),
        );

        let mut events = vec![WorkspaceEvent::DispatchProgress {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch_snapshot.clone(),
            task_id: provider_task_id.to_string(),
            description: description.clone(),
            summary: summary.clone(),
            last_tool_name,
        }];

        if self.publish_dispatch_lifecycle() {
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::DispatchProgress,
                    summary.unwrap_or(description),
                    dispatch_snapshot.visibility.unwrap_or(WorkspaceVisibility::Public),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.dispatch_id),
                    Some(provider_task_id.to_string()),
                ),
            });
        }

        Ok(self.push_events(events))
    }

    pub fn complete_dispatch(
        &mut self,
        provider_task_id: &str,
        status: DispatchStatus,
        output_file: Option<String>,
        summary: impl Into<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let summary = summary.into();
        let dispatch_snapshot = {
            let dispatch = self.find_dispatch_mut(provider_task_id)?;
            dispatch.status = status;
            dispatch.completed_at = Some(now());
            dispatch.last_summary = Some(summary.clone());
            dispatch.output_file = output_file.clone();
            dispatch.clone()
        };
        self.update_member(
            &dispatch_snapshot.role_id,
            Some(match status {
                DispatchStatus::Completed => MemberStatus::Idle,
                DispatchStatus::Failed => MemberStatus::Blocked,
                DispatchStatus::Stopped => MemberStatus::Waiting,
                DispatchStatus::Queued | DispatchStatus::Started | DispatchStatus::Running => {
                    MemberStatus::Active
                }
            }),
            Some(summary.clone()),
        );

        let mut events = vec![match status {
            DispatchStatus::Completed => WorkspaceEvent::DispatchCompleted {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                dispatch: dispatch_snapshot.clone(),
                task_id: provider_task_id.to_string(),
                output_file: output_file.clone().unwrap_or_default(),
                summary: summary.clone(),
            },
            DispatchStatus::Failed => WorkspaceEvent::DispatchFailed {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                dispatch: dispatch_snapshot.clone(),
                task_id: provider_task_id.to_string(),
                output_file: output_file.clone().unwrap_or_default(),
                summary: summary.clone(),
            },
            DispatchStatus::Stopped => WorkspaceEvent::DispatchStopped {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                dispatch: dispatch_snapshot.clone(),
                task_id: provider_task_id.to_string(),
                output_file: output_file.clone().unwrap_or_default(),
                summary: summary.clone(),
            },
            _ => return Err(RuntimeError::UnknownProviderTask(provider_task_id.to_string())),
        }];

        if self.publish_dispatch_lifecycle() {
            events.push(WorkspaceEvent::ActivityPublished {
                timestamp: now(),
                workspace_id: self.spec.id.clone(),
                activity: self.make_activity(
                    WorkspaceActivityKind::DispatchCompleted,
                    summary,
                    dispatch_snapshot.visibility.unwrap_or(WorkspaceVisibility::Public),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.role_id.clone()),
                    Some(dispatch_snapshot.dispatch_id),
                    Some(provider_task_id.to_string()),
                ),
            });
        }

        Ok(self.push_events(events))
    }

    pub fn attach_result_text(
        &mut self,
        provider_task_id: &str,
        result_text: impl Into<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let result_text = result_text.into();
        let dispatch = self.find_dispatch_mut(provider_task_id)?;
        dispatch.result_text = Some(result_text.clone());
        let dispatch_snapshot = dispatch.clone();

        let event = WorkspaceEvent::DispatchResult {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch_snapshot,
            task_id: provider_task_id.to_string(),
            result_text,
        };

        Ok(self.push_event(event))
    }

    fn ensure_member_exists(&self, role_id: &str) -> Result<(), RuntimeError> {
        self.state
            .members
            .get(role_id)
            .map(|_| ())
            .ok_or_else(|| RuntimeError::UnknownRole(role_id.to_string()))
    }

    fn default_visibility(&self) -> Option<WorkspaceVisibility> {
        self.spec
            .activity_policy
            .as_ref()
            .and_then(|policy| policy.default_visibility)
    }

    fn publish_dispatch_lifecycle(&self) -> bool {
        self.spec
            .activity_policy
            .as_ref()
            .and_then(|policy| policy.publish_dispatch_lifecycle)
            .unwrap_or(true)
    }

    fn update_member(
        &mut self,
        role_id: &str,
        status: Option<MemberStatus>,
        summary: Option<String>,
    ) {
        if let Some(member) = self.state.members.get_mut(role_id) {
            if let Some(status) = status {
                member.status = status;
            }
            if let Some(summary) = summary {
                member.public_state_summary = Some(summary);
            }
            member.last_activity_at = Some(now());
        }
    }

    fn make_activity(
        &mut self,
        kind: WorkspaceActivityKind,
        text: String,
        visibility: WorkspaceVisibility,
        role_id: Option<String>,
        member_id: Option<String>,
        dispatch_id: Option<Uuid>,
        task_id: Option<String>,
    ) -> WorkspaceActivity {
        let activity = WorkspaceActivity {
            activity_id: Uuid::new_v4(),
            workspace_id: self.spec.id.clone(),
            kind,
            visibility,
            text,
            created_at: now(),
            role_id,
            member_id,
            dispatch_id,
            task_id,
        };
        self.state.activities.push(activity.clone());
        activity
    }

    fn find_dispatch_mut(&mut self, provider_task_id: &str) -> Result<&mut TaskDispatch, RuntimeError> {
        let dispatch_id = self
            .provider_task_to_dispatch
            .get(provider_task_id)
            .copied()
            .ok_or_else(|| RuntimeError::UnknownProviderTask(provider_task_id.to_string()))?;

        self.state
            .dispatches
            .get_mut(&dispatch_id)
            .ok_or(RuntimeError::UnknownDispatch(dispatch_id))
    }

    fn push_event(&mut self, event: WorkspaceEvent) -> RuntimeTick {
        self.push_events(vec![event])
    }

    fn push_events(&mut self, events: Vec<WorkspaceEvent>) -> RuntimeTick {
        self.history.extend(events.iter().cloned());
        RuntimeTick {
            state: self.snapshot(),
            emitted: events,
        }
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use multi_agent_protocol::{
        ActivityPolicy, ClaimMode, ClaimPolicy, MultiAgentProvider, RoleAgentSpec, RoleSpec,
        RoleTaskRequest, WorkspaceEvent, WorkspaceSpec, WorkspaceVisibility,
    };

    use super::*;

    fn sample_spec() -> WorkspaceSpec {
        WorkspaceSpec {
            id: "workspace-1".to_string(),
            name: "Test Workspace".to_string(),
            provider: MultiAgentProvider::Cteno,
            model: "claude-sonnet-4-5".to_string(),
            cwd: Some("/tmp/demo".to_string()),
            orchestrator_prompt: None,
            allowed_tools: None,
            disallowed_tools: None,
            permission_mode: None,
            setting_sources: None,
            default_role_id: Some("coder".to_string()),
            coordinator_role_id: Some("coder".to_string()),
            claim_policy: Some(ClaimPolicy {
                mode: ClaimMode::Claim,
                claim_timeout_ms: Some(1000),
                max_assignees: Some(1),
                allow_supporting_claims: Some(false),
                fallback_role_id: Some("coder".to_string()),
            }),
            activity_policy: Some(ActivityPolicy {
                publish_user_messages: Some(true),
                publish_coordinator_messages: Some(true),
                publish_dispatch_lifecycle: Some(true),
                publish_member_messages: Some(true),
                default_visibility: Some(WorkspaceVisibility::Public),
            }),
            roles: vec![RoleSpec {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: None,
                direct: Some(true),
                output_root: Some("40-code/".to_string()),
                agent: RoleAgentSpec {
                    description: "Writes code".to_string(),
                    prompt: "Implement the requested changes".to_string(),
                    tools: Some(vec!["Read".to_string(), "Edit".to_string()]),
                    disallowed_tools: None,
                    model: None,
                    skills: None,
                    mcp_servers: None,
                    initial_prompt: None,
                    permission_mode: None,
                },
            }],
        }
    }

    #[test]
    fn start_registers_members_and_broadcasts_user_message() {
        let mut runtime = WorkspaceRuntime::new(sample_spec());
        let tick = runtime.start();
        assert!(tick
            .emitted
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::MemberRegistered { .. })));

        let tick = runtime.publish_user_message("Build group mentions");
        assert!(tick
            .emitted
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::ActivityPublished { activity, .. } if activity.kind == WorkspaceActivityKind::UserMessage)));
    }

    #[test]
    fn dispatch_lifecycle_round_trip() {
        let mut runtime = WorkspaceRuntime::new(sample_spec());
        runtime.start();
        runtime.initialize(
            Some("session-1".to_string()),
            vec!["coder".to_string()],
            vec!["Read".to_string(), "Edit".to_string()],
            None,
        );

        let (dispatch, _) = runtime
            .queue_dispatch(RoleTaskRequest {
                role_id: "coder".to_string(),
                instruction: "Write a feature".to_string(),
                summary: Some("Implement feature".to_string()),
                visibility: Some(WorkspaceVisibility::Public),
                source_role_id: Some("coder".to_string()),
            })
            .expect("dispatch should queue");

        runtime
            .claim_dispatch(
                dispatch.dispatch_id,
                "coder",
                ClaimStatus::Claimed,
                Some("Coder picked this up".to_string()),
            )
            .expect("dispatch should be claimable");
        runtime
            .start_next_dispatch("provider-task-1", "Implement feature", Some("tool-use-1".to_string()))
            .expect("dispatch should start");
        runtime
            .progress_dispatch(
                "provider-task-1",
                "Editing files",
                Some("Editing src/lib.rs".to_string()),
                Some("Edit".to_string()),
            )
            .expect("dispatch should progress");
        runtime
            .complete_dispatch(
                "provider-task-1",
                DispatchStatus::Completed,
                Some("40-code/output.md".to_string()),
                "Feature implemented",
            )
            .expect("dispatch should complete");
        let tick = runtime
            .attach_result_text("provider-task-1", "Done")
            .expect("dispatch should accept result text");

        let stored = runtime.snapshot().dispatches[&dispatch.dispatch_id].clone();
        assert_eq!(stored.status, DispatchStatus::Completed);
        assert_eq!(stored.provider_task_id.as_deref(), Some("provider-task-1"));
        assert_eq!(stored.tool_use_id.as_deref(), Some("tool-use-1"));
        assert_eq!(stored.result_text.as_deref(), Some("Done"));
        assert_eq!(stored.claim_status, Some(ClaimStatus::Claimed));

        assert!(matches!(
            &tick.emitted[0],
            WorkspaceEvent::DispatchResult { task_id, result_text, .. }
                if task_id == "provider-task-1" && result_text == "Done"
        ));
        assert!(!runtime.snapshot().activities.is_empty());
    }
}
