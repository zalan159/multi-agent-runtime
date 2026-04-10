import path from 'node:path';
import { ClaudeAgentWorkspace, createOpcSoloCompanyWorkspace } from '../dist/index.js';
import { attachConsoleLogger, createScratchDir, printFileIfExists } from './_shared.mjs';

const cwd = await createScratchDir('cteno-opc');
const workspace = new ClaudeAgentWorkspace({
  spec: createOpcSoloCompanyWorkspace({
    id: `opc-${Date.now()}`,
    name: 'OPC Solo Company Smoke',
    cwd,
  }),
});

const stopLogging = attachConsoleLogger(workspace, 'opc');

try {
  await workspace.start();
  const dispatch = await workspace.runRoleTask({
    roleId: 'finance',
    summary: 'Prepare a monthly close checklist',
    instruction:
      'Create a markdown checklist at company/10-finance/monthly-close-checklist.md for a solo SaaS founder closing the month. Include cash review, invoices, subscriptions, payroll/contractors, tax prep handoff, and KPI review. Keep it practical and concise.',
  }, { timeoutMs: 180000, resultTimeoutMs: 20000 });

  console.log('\nFINAL DISPATCH');
  console.log(JSON.stringify(dispatch, null, 2));
  await printFileIfExists(path.join(cwd, 'company/10-finance/monthly-close-checklist.md'));
} finally {
  stopLogging();
  await workspace.close();
}
