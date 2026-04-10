use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;
use multi_agent_protocol::{
    DispatchStatus, RoleSpec, RoleTaskRequest, WorkspaceEvent, WorkspaceSpec,
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

    pub fn runtime(&self) -> &WorkspaceRuntime {
        &self.runtime
    }

    pub fn bootstrapped(&self) -> Option<&BootstrappedWorkspace> {
        self.bootstrapped.as_ref()
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
        let event = WorkspaceEvent::Message {
            timestamp: Utc::now().to_rfc3339(),
            workspace_id: self.runtime.spec().id.clone(),
            role: role.to_string(),
            text: text.to_string(),
            session_id: None,
            parent_tool_use_id: None,
        };

        RuntimeTick {
            state: self.runtime.snapshot(),
            emitted: vec![event],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use multi_agent_protocol::{
        MultiAgentProvider, RoleAgentSpec, RoleSpec, RoleTaskRequest, WorkspaceSpec,
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
            default_role_id: Some("coder".to_string()),
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
            })
            .await
            .expect("assign role task should succeed");

        assert_eq!(dispatch.role_id, "coder");
        assert_eq!(queued_events.len(), 1);

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "session-coder");
        assert_eq!(sent[0].1, "Implement group mentions");
    }
}
