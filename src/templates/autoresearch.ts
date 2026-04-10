import type { WorkspaceTemplate } from '../core/templates.js';

export function createAutoresearchTemplate(): WorkspaceTemplate {
  return {
    templateId: 'autoresearch',
    templateName: 'Autoresearch',
    description: 'A research-oriented workspace for scouting and synthesis.',
    defaultRoleId: 'lead',
    orchestratorPrompt:
      'You orchestrate an autonomous research workspace. Keep hypotheses explicit, separate evidence from interpretation, and favor compact research artifacts that can feed later evaluation loops.',
    roles: [
      {
        id: 'lead',
        name: 'Lead',
        outputRoot: 'research/00-lead/',
        agent: {
          description: 'Frames the research question and decides what evidence is worth collecting next.',
          prompt:
            'You are a research lead. Turn vague topics into testable questions, define success criteria, and keep each loop scoped tightly.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
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
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'web_search', 'web_fetch'],
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
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
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
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
    ],
  };
}
