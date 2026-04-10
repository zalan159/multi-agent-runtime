use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{MultiAgentProvider, RoleSpec};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    Queued,
    Started,
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Idle,
    Running,
    RequiresAction,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskDispatch {
    pub dispatch_id: Uuid,
    pub workspace_id: String,
    pub role_id: String,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: DispatchStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub workspace_id: String,
    pub status: WorkspaceStatus,
    pub provider: MultiAgentProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    pub roles: BTreeMap<String, RoleSpec>,
    pub dispatches: BTreeMap<Uuid, TaskDispatch>,
}
