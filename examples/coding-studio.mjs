import path from 'node:path';
import { ClaudeAgentWorkspace, createCodingStudioWorkspace } from '../dist/index.js';
import { attachConsoleLogger, createScratchDir, printFileIfExists } from './_shared.mjs';

const cwd = await createScratchDir('cteno-coding-studio');
const workspace = new ClaudeAgentWorkspace({
  spec: createCodingStudioWorkspace({
    id: `coding-studio-${Date.now()}`,
    name: 'Coding Studio Smoke',
    cwd,
  }),
});

const stopLogging = attachConsoleLogger(workspace, 'coding');

try {
  await workspace.start();
  const dispatch = await workspace.runRoleTask({
    roleId: 'prd',
    summary: 'Create a PRD stub for group mentions',
    instruction:
      'Create a short markdown PRD at 10-prd/group-mentions.md for a group-chat mention feature. Include sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria. Keep it under 250 words.',
  }, { timeoutMs: 180000, resultTimeoutMs: 20000 });

  console.log('\nFINAL DISPATCH');
  console.log(JSON.stringify(dispatch, null, 2));
  await printFileIfExists(path.join(cwd, '10-prd/group-mentions.md'));
} finally {
  stopLogging();
  await workspace.close();
}
