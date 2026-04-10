use std::collections::{BTreeMap, VecDeque};

use chrono::Utc;
use multi_agent_protocol::{
    DispatchStatus, RoleTaskRequest, TaskDispatch, WorkspaceEvent, WorkspaceSpec, WorkspaceState,
    WorkspaceStatus,
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

        Self {
            state: WorkspaceState {
                workspace_id: spec.id.clone(),
                status: WorkspaceStatus::Idle,
                provider: spec.provider,
                session_id: None,
                started_at: None,
                roles,
                dispatches: BTreeMap::new(),
            },
            spec,
            pending_dispatch_queue: VecDeque::new(),
            provider_task_to_dispatch: BTreeMap::new(),
            history: Vec::new(),
        }
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

        let event = WorkspaceEvent::WorkspaceStarted {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            spec: self.spec.clone(),
        };
        self.push_event(event)
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

    pub fn queue_dispatch(&mut self, request: RoleTaskRequest) -> Result<(TaskDispatch, RuntimeTick), RuntimeError> {
        if !self.state.roles.contains_key(&request.role_id) {
            return Err(RuntimeError::UnknownRole(request.role_id));
        }

        let dispatch = TaskDispatch {
            dispatch_id: Uuid::new_v4(),
            workspace_id: self.spec.id.clone(),
            role_id: request.role_id,
            instruction: request.instruction,
            summary: request.summary,
            status: DispatchStatus::Queued,
            provider_task_id: None,
            tool_use_id: None,
            created_at: now(),
            started_at: None,
            completed_at: None,
            output_file: None,
            last_summary: None,
            result_text: None,
        };

        self.pending_dispatch_queue.push_back(dispatch.dispatch_id);
        self.state
            .dispatches
            .insert(dispatch.dispatch_id, dispatch.clone());

        let event = WorkspaceEvent::DispatchQueued {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch.clone(),
        };

        Ok((dispatch, self.push_event(event)))
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

        self.provider_task_to_dispatch
            .insert(provider_task_id.clone(), dispatch_id);

        let event = WorkspaceEvent::DispatchStarted {
            timestamp: now(),
            workspace_id: self.spec.id.clone(),
            dispatch: dispatch.clone(),
            task_id: provider_task_id,
            description,
        };

        Ok((dispatch.clone(), self.push_event(event)))
    }

    pub fn progress_dispatch(
        &mut self,
        provider_task_id: &str,
        description: impl Into<String>,
        summary: Option<String>,
        last_tool_name: Option<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let workspace_id = self.spec.id.clone();
        let description = description.into();
        let dispatch = self.find_dispatch_mut(provider_task_id)?;
        dispatch.status = DispatchStatus::Running;
        if let Some(summary) = summary.clone() {
            dispatch.last_summary = Some(summary);
        }
        let dispatch_snapshot = dispatch.clone();

        let event = WorkspaceEvent::DispatchProgress {
            timestamp: now(),
            workspace_id,
            dispatch: dispatch_snapshot,
            task_id: provider_task_id.to_string(),
            description,
            summary,
            last_tool_name,
        };

        Ok(self.push_event(event))
    }

    pub fn complete_dispatch(
        &mut self,
        provider_task_id: &str,
        status: DispatchStatus,
        output_file: Option<String>,
        summary: impl Into<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let workspace_id = self.spec.id.clone();
        let summary = summary.into();
        let dispatch = self.find_dispatch_mut(provider_task_id)?;
        dispatch.status = status;
        dispatch.completed_at = Some(now());
        dispatch.last_summary = Some(summary.clone());
        dispatch.output_file = output_file.clone();
        let dispatch_snapshot = dispatch.clone();

        let event = match status {
            DispatchStatus::Completed => WorkspaceEvent::DispatchCompleted {
                timestamp: now(),
                workspace_id: workspace_id.clone(),
                dispatch: dispatch_snapshot.clone(),
                task_id: provider_task_id.to_string(),
                output_file: output_file.unwrap_or_default(),
                summary,
            },
            DispatchStatus::Failed => WorkspaceEvent::DispatchFailed {
                timestamp: now(),
                workspace_id: workspace_id.clone(),
                dispatch: dispatch_snapshot.clone(),
                task_id: provider_task_id.to_string(),
                output_file: output_file.unwrap_or_default(),
                summary,
            },
            DispatchStatus::Stopped => WorkspaceEvent::DispatchStopped {
                timestamp: now(),
                workspace_id,
                dispatch: dispatch_snapshot,
                task_id: provider_task_id.to_string(),
                output_file: output_file.unwrap_or_default(),
                summary,
            },
            _ => {
                return Err(RuntimeError::UnknownProviderTask(provider_task_id.to_string()));
            }
        };

        Ok(self.push_event(event))
    }

    pub fn attach_result_text(
        &mut self,
        provider_task_id: &str,
        result_text: impl Into<String>,
    ) -> Result<RuntimeTick, RuntimeError> {
        let workspace_id = self.spec.id.clone();
        let result_text = result_text.into();
        let dispatch = self.find_dispatch_mut(provider_task_id)?;
        dispatch.result_text = Some(result_text.clone());
        let dispatch_snapshot = dispatch.clone();

        let event = WorkspaceEvent::DispatchResult {
            timestamp: now(),
            workspace_id,
            dispatch: dispatch_snapshot,
            task_id: provider_task_id.to_string(),
            result_text,
        };

        Ok(self.push_event(event))
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
        self.history.push(event.clone());
        RuntimeTick {
            state: self.snapshot(),
            emitted: vec![event],
        }
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use multi_agent_protocol::{
        MultiAgentProvider, RoleAgentSpec, RoleSpec, RoleTaskRequest, WorkspaceEvent, WorkspaceSpec,
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
            default_role_id: Some("coder".to_string()),
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
                    initial_prompt: None,
                    permission_mode: None,
                },
            }],
        }
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
            })
            .expect("dispatch should queue");

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

        assert!(matches!(
            &tick.emitted[0],
            WorkspaceEvent::DispatchResult { task_id, result_text, .. }
                if task_id == "provider-task-1" && result_text == "Done"
        ));
    }
}
