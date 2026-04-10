import type { WorkspaceSpec } from '../core/types.js';

export function createAutoresearchWorkspace(params: {
  id: string;
  name: string;
  cwd: string;
  model?: string;
}): WorkspaceSpec {
  return {
    id: params.id,
    name: params.name,
    provider: 'claude-agent-sdk',
    model: params.model ?? 'claude-sonnet-4-5',
    cwd: params.cwd,
    permissionMode: 'acceptEdits',
    settingSources: ['project'],
    allowedTools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep', 'WebFetch', 'WebSearch', 'Bash'],
    orchestratorPrompt:
      'You orchestrate an autonomous research workspace. Keep hypotheses explicit, separate evidence from interpretation, and favor compact research artifacts that can feed later evaluation loops.',
    defaultRoleId: 'lead',
    roles: [
      {
        id: 'lead',
        name: 'Lead',
        outputRoot: 'research/00-lead/',
        agent: {
          description: 'Frames the research question and decides what evidence is worth collecting next.',
          prompt:
            'You are a research lead. Turn vague topics into testable questions, define success criteria, and keep each loop scoped tightly.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'scout',
        name: 'Scout',
        outputRoot: 'research/10-scout/',
        agent: {
          description: 'Collects outside signals, references, and raw observations.',
          prompt:
            'You are a web research scout. Gather high-signal evidence, cite sources inline, and keep notes concise enough for downstream synthesis.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep', 'WebSearch', 'WebFetch'],
        },
      },
      {
        id: 'experimenter',
        name: 'Experimenter',
        outputRoot: 'research/20-experiments/',
        agent: {
          description: 'Turns a hypothesis into a measurable experiment design.',
          prompt:
            'You design small, measurable experiments. Define variables, success metrics, instrumentation, and stopping criteria with minimal ceremony.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep', 'Bash'],
        },
      },
      {
        id: 'critic',
        name: 'Critic',
        outputRoot: 'research/30-critic/',
        agent: {
          description: 'Challenges assumptions, spots confounders, and tightens reasoning.',
          prompt:
            'You are a skeptical research critic. Look for weak evidence, missing controls, and untested assumptions before the team moves on.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
    ],
  };
}
