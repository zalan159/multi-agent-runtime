export type MultiAgentProvider = 'claude-agent-sdk' | 'codex-sdk';

export type WorkspaceVisibility = 'public' | 'private' | 'coordinator';
export type ClaimMode = 'direct' | 'claim' | 'coordinator_only';
export type ClaimStatus = 'pending' | 'claimed' | 'supporting' | 'released' | 'declined';
export type MemberStatus = 'idle' | 'active' | 'blocked' | 'waiting' | 'offline';
export type WorkspaceActivityKind =
  | 'user_message'
  | 'coordinator_message'
  | 'member_claimed'
  | 'member_progress'
  | 'member_blocked'
  | 'member_delivered'
  | 'member_summary'
  | 'dispatch_started'
  | 'dispatch_progress'
  | 'dispatch_completed'
  | 'system_notice';

export type PermissionMode =
  | 'default'
  | 'acceptEdits'
  | 'plan'
  | 'dontAsk'
  | 'bypassPermissions';

export interface RoleAgentSpec {
  description: string;
  prompt: string;
  tools?: string[];
  disallowedTools?: string[];
  model?: string;
  skills?: string[];
  mcpServers?: string[];
  initialPrompt?: string;
  maxTurns?: number;
  background?: boolean;
  effort?: 'low' | 'medium' | 'high' | 'max' | number;
  permissionMode?: PermissionMode;
}

export interface RoleSpec {
  id: string;
  name: string;
  description?: string;
  direct?: boolean;
  outputRoot?: string;
  agent: RoleAgentSpec;
}

export interface ClaimPolicy {
  mode: ClaimMode;
  claimTimeoutMs?: number;
  maxAssignees?: number;
  allowSupportingClaims?: boolean;
  fallbackRoleId?: string;
}

export interface ActivityPolicy {
  publishUserMessages?: boolean;
  publishCoordinatorMessages?: boolean;
  publishDispatchLifecycle?: boolean;
  publishMemberMessages?: boolean;
  defaultVisibility?: WorkspaceVisibility;
}

export interface WorkspaceSpec {
  id: string;
  name: string;
  provider: MultiAgentProvider;
  model: string;
  cwd?: string;
  orchestratorPrompt?: string;
  allowedTools?: string[];
  disallowedTools?: string[];
  permissionMode?: PermissionMode;
  settingSources?: Array<'user' | 'project' | 'local'>;
  roles: RoleSpec[];
  defaultRoleId?: string;
  coordinatorRoleId?: string;
  claimPolicy?: ClaimPolicy;
  activityPolicy?: ActivityPolicy;
}

export interface RoleTaskRequest {
  roleId: string;
  instruction: string;
  summary?: string;
  visibility?: WorkspaceVisibility;
  sourceRoleId?: string;
}

export interface WorkspaceTurnRequest {
  message: string;
  visibility?: WorkspaceVisibility;
  maxAssignments?: number;
  preferRoleId?: string;
}

export interface WorkspaceTurnAssignment {
  roleId: string;
  instruction: string;
  summary?: string;
  visibility?: WorkspaceVisibility;
}

export interface WorkspaceTurnPlan {
  coordinatorRoleId: string;
  responseText: string;
  assignments: WorkspaceTurnAssignment[];
  rationale?: string;
}

export interface WorkspaceTurnResult {
  request: WorkspaceTurnRequest;
  coordinatorDispatch?: TaskDispatch;
  plan: WorkspaceTurnPlan;
  dispatches: TaskDispatch[];
}

export interface TaskDispatch {
  dispatchId: string;
  workspaceId: string;
  roleId: string;
  instruction: string;
  summary?: string;
  visibility?: WorkspaceVisibility;
  sourceRoleId?: string;
  status: 'queued' | 'started' | 'running' | 'completed' | 'failed' | 'stopped';
  providerTaskId?: string;
  toolUseId?: string;
  createdAt: string;
  startedAt?: string;
  completedAt?: string;
  outputFile?: string;
  lastSummary?: string;
  resultText?: string;
  claimedByMemberIds?: string[];
  claimStatus?: ClaimStatus;
}

export interface WorkspaceMember {
  memberId: string;
  workspaceId: string;
  roleId: string;
  roleName: string;
  direct?: boolean;
  sessionId?: string;
  status: MemberStatus;
  publicStateSummary?: string;
  lastActivityAt?: string;
}

export interface WorkspaceActivity {
  activityId: string;
  workspaceId: string;
  kind: WorkspaceActivityKind;
  visibility: WorkspaceVisibility;
  text: string;
  createdAt: string;
  roleId?: string;
  memberId?: string;
  dispatchId?: string;
  taskId?: string;
}

export interface WorkspaceState {
  workspaceId: string;
  status: 'idle' | 'running' | 'requires_action' | 'closed';
  provider: MultiAgentProvider;
  sessionId?: string;
  startedAt?: string;
  roles: Record<string, RoleSpec>;
  dispatches: Record<string, TaskDispatch>;
  members: Record<string, WorkspaceMember>;
  activities: WorkspaceActivity[];
}
