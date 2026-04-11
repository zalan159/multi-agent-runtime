use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MultiAgentProvider {
    ClaudeAgentSdk,
    CodexSdk,
    Cteno,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    DontAsk,
    BypassPermissions,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    User,
    Project,
    Local,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceVisibility {
    Public,
    Private,
    Coordinator,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimMode {
    Direct,
    Claim,
    CoordinatorOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Pending,
    Claimed,
    Supporting,
    Released,
    Declined,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaimPolicy {
    pub mode: ClaimMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_assignees: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_supporting_claims: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_role_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_user_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_coordinator_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_dispatch_lifecycle: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_member_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_visibility: Option<WorkspaceVisibility>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoleAgentSpec {
    pub description: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoleSpec {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_root: Option<String>,
    pub agent: RoleAgentSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSpec {
    pub id: String,
    pub name: String,
    pub provider: MultiAgentProvider,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<SettingSource>>,
    pub roles: Vec<RoleSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_policy: Option<ClaimPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_policy: Option<ActivityPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoleTaskRequest {
    pub role_id: String,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<WorkspaceVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_role_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTurnRequest {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<WorkspaceVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_assignments: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefer_role_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTurnAssignment {
    pub role_id: String,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<WorkspaceVisibility>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTurnPlan {
    pub coordinator_role_id: String,
    pub response_text: String,
    pub assignments: Vec<WorkspaceTurnAssignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}
