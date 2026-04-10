export type MultiAgentProvider = 'claude-agent-sdk' | 'codex-sdk';

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
}

export interface RoleTaskRequest {
  roleId: string;
  instruction: string;
  summary?: string;
}

export interface TaskDispatch {
  dispatchId: string;
  workspaceId: string;
  roleId: string;
  instruction: string;
  summary?: string;
  status: 'queued' | 'started' | 'running' | 'completed' | 'failed' | 'stopped';
  providerTaskId?: string;
  toolUseId?: string;
  createdAt: string;
  startedAt?: string;
  completedAt?: string;
  outputFile?: string;
  lastSummary?: string;
  resultText?: string;
}

export interface WorkspaceState {
  workspaceId: string;
  status: 'idle' | 'running' | 'requires_action' | 'closed';
  provider: MultiAgentProvider;
  sessionId?: string;
  startedAt?: string;
  roles: Record<string, RoleSpec>;
  dispatches: Record<string, TaskDispatch>;
}
