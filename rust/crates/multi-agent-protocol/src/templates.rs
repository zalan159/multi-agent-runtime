use serde::{Deserialize, Serialize};

use crate::{
    ActivityPolicy, ClaimMode, ClaimPolicy, MultiAgentProvider, PermissionMode, RoleAgentSpec,
    RoleSpec, SettingSource, WorkspaceSpec, WorkspaceVisibility,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Shell,
    WebFetch,
    WebSearch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRoleAgentSpec {
    pub description: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<AgentCapability>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_edit_access: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRoleSpec {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_root: Option<String>,
    pub agent: TemplateRoleAgentSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTemplate {
    pub template_id: String,
    pub template_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_policy: Option<ClaimPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_policy: Option<ActivityPolicy>,
    pub roles: Vec<TemplateRoleSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInstanceParams {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProfile {
    pub provider: MultiAgentProvider,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_edit_permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<SettingSource>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
}

pub fn create_claude_workspace_profile(model: Option<&str>) -> WorkspaceProfile {
    WorkspaceProfile {
        provider: MultiAgentProvider::ClaudeAgentSdk,
        model: model.unwrap_or("claude-sonnet-4-5").to_string(),
        permission_mode: Some(PermissionMode::AcceptEdits),
        role_edit_permission_mode: Some(PermissionMode::AcceptEdits),
        setting_sources: Some(vec![SettingSource::Project]),
        allowed_tools: None,
        disallowed_tools: None,
    }
}

pub fn create_codex_workspace_profile(model: Option<&str>) -> WorkspaceProfile {
    WorkspaceProfile {
        provider: MultiAgentProvider::CodexSdk,
        model: model.unwrap_or("gpt-5.1-codex-mini").to_string(),
        permission_mode: None,
        role_edit_permission_mode: None,
        setting_sources: None,
        allowed_tools: None,
        disallowed_tools: None,
    }
}

pub fn instantiate_workspace(
    template: &WorkspaceTemplate,
    instance: &WorkspaceInstanceParams,
    profile: &WorkspaceProfile,
) -> WorkspaceSpec {
    let roles = template
        .roles
        .iter()
        .map(|role| instantiate_role(role, profile))
        .collect::<Vec<_>>();

    let derived_allowed_tools = unique_strings(
        roles
            .iter()
            .flat_map(|role| role.agent.tools.clone().unwrap_or_default())
            .collect(),
    );

    WorkspaceSpec {
        id: instance.id.clone(),
        name: instance.name.clone(),
        provider: profile.provider,
        model: profile.model.clone(),
        cwd: instance.cwd.clone(),
        orchestrator_prompt: template.orchestrator_prompt.clone(),
        allowed_tools: profile
            .allowed_tools
            .clone()
            .or_else(|| (!derived_allowed_tools.is_empty()).then_some(derived_allowed_tools)),
        disallowed_tools: profile.disallowed_tools.clone(),
        permission_mode: profile.permission_mode,
        setting_sources: profile.setting_sources.clone(),
        roles,
        default_role_id: template.default_role_id.clone(),
        coordinator_role_id: template.coordinator_role_id.clone(),
        claim_policy: template.claim_policy.clone(),
        activity_policy: template.activity_policy.clone(),
    }
}

fn instantiate_role(role: &TemplateRoleSpec, profile: &WorkspaceProfile) -> RoleSpec {
    RoleSpec {
        id: role.id.clone(),
        name: role.name.clone(),
        description: role.description.clone(),
        direct: role.direct,
        output_root: role.output_root.clone(),
        agent: RoleAgentSpec {
            description: role.agent.description.clone(),
            prompt: role.agent.prompt.clone(),
            tools: role
                .agent
                .capabilities
                .clone()
                .map(|capabilities| map_capabilities(&capabilities))
                .filter(|mapped| !mapped.is_empty()),
            disallowed_tools: None,
            model: role.agent.model.clone(),
            skills: role.agent.skills.clone(),
            mcp_servers: None,
            initial_prompt: role.agent.initial_prompt.clone(),
            permission_mode: if role.agent.requires_edit_access.unwrap_or(false) {
                profile.role_edit_permission_mode
            } else {
                None
            },
        },
    }
}

fn map_capabilities(capabilities: &[AgentCapability]) -> Vec<String> {
    unique_strings(
        capabilities
            .iter()
            .map(|capability| match capability {
                AgentCapability::Read => "Read",
                AgentCapability::Write => "Write",
                AgentCapability::Edit => "Edit",
                AgentCapability::Glob => "Glob",
                AgentCapability::Grep => "Grep",
                AgentCapability::Shell => "Bash",
                AgentCapability::WebFetch => "WebFetch",
                AgentCapability::WebSearch => "WebSearch",
            })
            .map(str::to_string)
            .collect(),
    )
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    let mut values = values;
    values.sort();
    values.dedup();
    values
}

pub fn create_coding_studio_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "coding-studio".to_string(),
        template_name: "Coding Studio".to_string(),
        description: Some("A software delivery workspace with fixed specialist roles.".to_string()),
        default_role_id: Some("pm".to_string()),
        coordinator_role_id: Some("pm".to_string()),
        orchestrator_prompt: Some(
            "You are the orchestrator for a software delivery workspace. Keep the team aligned, route work to the correct role agent, and summarize progress crisply.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::Claim,
            claim_timeout_ms: Some(1500),
            max_assignees: Some(1),
            allow_supporting_claims: Some(true),
            fallback_role_id: Some("pm".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        roles: vec![
            TemplateRoleSpec {
                id: "pm".to_string(),
                name: "PM".to_string(),
                description: None,
                direct: None,
                output_root: Some("00-management/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Plans scope, sequencing, and acceptance criteria.".to_string(),
                    prompt: "You are a product/project manager. Clarify scope, break work into milestones, and keep handoffs explicit. Prefer concise plans with acceptance criteria.".to_string(),
                    capabilities: Some(vec![AgentCapability::Read, AgentCapability::Glob, AgentCapability::Grep]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "prd".to_string(),
                name: "PRD".to_string(),
                description: None,
                direct: None,
                output_root: Some("10-prd/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Writes product requirement docs and task definitions.".to_string(),
                    prompt: "You write implementation-ready PRDs. Always produce a concrete markdown deliverable instead of notes. Include explicit sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria, and make the content specific enough for downstream implementation.".to_string(),
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: Some(
                        "Default PRD contract: write the deliverable under `10-prd/` unless the task gives another file path. Use these exact markdown section headings: `## Goal`, `## User Story`, `## Scope`, `## Non-Goals`, `## Acceptance Criteria`. Do not stop at an overview.".to_string(),
                    ),
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "architect".to_string(),
                name: "Architect".to_string(),
                description: None,
                direct: None,
                output_root: Some("30-arch/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Designs implementation plans and system changes.".to_string(),
                    prompt: "You are a software architect. Produce pragmatic design notes, data flow decisions, interfaces, and risks before coding starts.".to_string(),
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: None,
                direct: None,
                output_root: Some("40-code/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Implements code changes and keeps diffs focused.".to_string(),
                    prompt: "You are an implementation specialist. Make the requested change with minimal churn, explain assumptions briefly, and keep code consistent with the repository style.".to_string(),
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                        AgentCapability::Shell,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: Some(true),
                },
            },
            TemplateRoleSpec {
                id: "tester".to_string(),
                name: "Tester".to_string(),
                description: None,
                direct: None,
                output_root: Some("50-test/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Runs tests, validates behavior, and reports regressions.".to_string(),
                    prompt: "You are a verification specialist. Run the narrowest useful checks first, surface failures clearly, and report residual risks if full coverage is not possible.".to_string(),
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                        AgentCapability::Shell,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
            TemplateRoleSpec {
                id: "reviewer".to_string(),
                name: "Reviewer".to_string(),
                description: None,
                direct: None,
                output_root: Some("60-review/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Reviews changes for bugs, regressions, and missing tests.".to_string(),
                    prompt: "You perform code review with a bug-finding mindset. Prioritize correctness, regressions, and missing validation over style commentary.".to_string(),
                    capabilities: Some(vec![AgentCapability::Read, AgentCapability::Glob, AgentCapability::Grep]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: None,
                },
            },
        ],
    }
}

pub fn create_opc_solo_company_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "opc-solo-company".to_string(),
        template_name: "OPC Solo Company".to_string(),
        description: Some("A one-person company staffed by specialist digital operators.".to_string()),
        default_role_id: Some("ceo".to_string()),
        coordinator_role_id: Some("ceo".to_string()),
        orchestrator_prompt: Some(
            "You orchestrate a one-person company staffed by specialist digital operators. Route work to the best role, keep recommendations practical, and prefer concrete operating documents over abstract advice.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::CoordinatorOnly,
            claim_timeout_ms: None,
            max_assignees: Some(1),
            allow_supporting_claims: Some(false),
            fallback_role_id: Some("ceo".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        roles: vec![
            role_with_capabilities(
                "ceo",
                "CEO",
                "company/00-ceo/",
                "Owns priorities, approvals, and operating decisions for the solo company.",
                "You are the CEO of a one-person software company. Frame decisions clearly, make tradeoffs explicit, and turn fuzzy requests into concrete next actions.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "finance",
                "Finance",
                "company/10-finance/",
                "Prepares cash, revenue, budget, and monthly operating documents.",
                "You are a finance operator for a lean software business. Produce concise, audit-friendly checklists, budgets, and summaries with concrete assumptions and numbers where possible.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "tax",
                "Tax",
                "company/20-tax/",
                "Prepares filing checklists, tax calendars, and compliance notes.",
                "You are a tax operations specialist for a solo company. Focus on deadlines, supporting documents, risks, and what needs accountant review.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "admin",
                "Admin",
                "company/30-admin/",
                "Handles administrative SOPs, vendor coordination, and internal operations.",
                "You are an operations administrator. Turn messy business tasks into checklists, SOPs, and lightweight systems that a solo founder can actually maintain.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "recruiter",
                "Recruiter",
                "company/40-recruiting/",
                "Drafts hiring briefs, scorecards, and interview plans when the company needs help.",
                "You are a pragmatic recruiting partner for a small company. Keep hiring materials specific, lightweight, and aligned with the business stage.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
        ],
    }
}

pub fn create_autoresearch_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "autoresearch".to_string(),
        template_name: "Autoresearch".to_string(),
        description: Some("A research-oriented workspace for scouting and synthesis.".to_string()),
        default_role_id: Some("lead".to_string()),
        coordinator_role_id: Some("lead".to_string()),
        orchestrator_prompt: Some(
            "You orchestrate an autonomous research workspace. Keep hypotheses explicit, separate evidence from interpretation, and favor compact research artifacts that can feed later evaluation loops.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::Claim,
            claim_timeout_ms: Some(1000),
            max_assignees: Some(2),
            allow_supporting_claims: Some(true),
            fallback_role_id: Some("lead".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        roles: vec![
            role_with_capabilities(
                "lead",
                "Lead",
                "research/00-lead/",
                "Frames the research question and decides what evidence is worth collecting next.",
                "You are a research lead. Turn vague topics into testable questions, define success criteria, and keep each loop scoped tightly.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "scout",
                "Scout",
                "research/10-scout/",
                "Collects outside signals, references, and raw observations.",
                "You are a web research scout. Gather high-signal evidence, cite sources inline, and keep notes concise enough for downstream synthesis.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                    AgentCapability::WebSearch,
                    AgentCapability::WebFetch,
                ],
            ),
            role_with_capabilities(
                "experimenter",
                "Experimenter",
                "research/20-experiments/",
                "Turns a hypothesis into a measurable experiment design.",
                "You design small, measurable experiments. Define variables, success metrics, instrumentation, and stopping criteria with minimal ceremony.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                    AgentCapability::Shell,
                ],
            ),
            role_with_capabilities(
                "critic",
                "Critic",
                "research/30-critic/",
                "Challenges assumptions, spots confounders, and tightens reasoning.",
                "You are a skeptical research critic. Look for weak evidence, missing controls, and untested assumptions before the team moves on.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
        ],
    }
}

pub fn create_edict_governance_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        template_id: "edict-governance".to_string(),
        template_name: "Three Departments Six Ministries".to_string(),
        description: Some(
            "A governance-style multi-agent workspace for coordinated planning, review, execution, and oversight."
                .to_string(),
        ),
        default_role_id: Some("shangshu".to_string()),
        coordinator_role_id: Some("shangshu".to_string()),
        orchestrator_prompt: Some(
            "You coordinate a governance-style multi-agent workspace. Keep responsibilities crisp, route work to the right ministry, and enforce review before completion.".to_string(),
        ),
        claim_policy: Some(ClaimPolicy {
            mode: ClaimMode::Claim,
            claim_timeout_ms: Some(1500),
            max_assignees: Some(2),
            allow_supporting_claims: Some(true),
            fallback_role_id: Some("shangshu".to_string()),
        }),
        activity_policy: Some(ActivityPolicy {
            publish_user_messages: Some(true),
            publish_coordinator_messages: Some(true),
            publish_dispatch_lifecycle: Some(true),
            publish_member_messages: Some(true),
            default_visibility: Some(WorkspaceVisibility::Public),
        }),
        roles: vec![
            role_with_capabilities(
                "shangshu",
                "Shangshu",
                "governance/00-shangshu/",
                "Coordinates ministries, sequence, and final closure.",
                "You are the chief coordinator. Route work, close loops, demand concrete outputs, and make sure every major task ends with a clear disposition.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "zhongshu",
                "Zhongshu",
                "governance/10-zhongshu/",
                "Drafts mission briefs, task orders, and structured plans.",
                "You draft precise task orders, plans, and briefs. Convert vague goals into crisp instructions with deliverables and milestones.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "menxia",
                "Menxia",
                "governance/20-menxia/",
                "Reviews proposals, challenges assumptions, and enforces red-team scrutiny.",
                "You are the review gate. Challenge weak reasoning, surface risks, and reject plans that are not yet executable or safe.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            TemplateRoleSpec {
                id: "gongbu".to_string(),
                name: "Gongbu".to_string(),
                description: None,
                direct: None,
                output_root: Some("governance/30-gongbu/".to_string()),
                agent: TemplateRoleAgentSpec {
                    description: "Executes implementation, build-out, and tooling work.".to_string(),
                    prompt: "You are the implementation ministry. Build concrete outputs, keep execution disciplined, and report exact deliverables.".to_string(),
                    capabilities: Some(vec![
                        AgentCapability::Read,
                        AgentCapability::Write,
                        AgentCapability::Edit,
                        AgentCapability::Glob,
                        AgentCapability::Grep,
                        AgentCapability::Shell,
                    ]),
                    model: None,
                    skills: None,
                    initial_prompt: None,
                    requires_edit_access: Some(true),
                },
            },
            role_with_capabilities(
                "hubu",
                "Hubu",
                "governance/40-hubu/",
                "Tracks resources, budgets, dependencies, and allocation tradeoffs.",
                "You manage resources and constraints. Quantify budget, headcount, token, or time tradeoffs and keep plans grounded in capacity.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "libu",
                "Libu",
                "governance/50-libu/",
                "Prepares communication, docs, release notes, and external-facing materials.",
                "You own communication and documentation. Package decisions and outputs into clear artifacts others can consume quickly.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "xingbu",
                "Xingbu",
                "governance/60-xingbu/",
                "Owns compliance, safety, and rule enforcement.",
                "You enforce quality, compliance, and safety constraints. Flag violations early and insist on auditable fixes.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                ],
            ),
            role_with_capabilities(
                "bingbu",
                "Bingbu",
                "governance/70-bingbu/",
                "Handles operations, release readiness, incident response, and escalation.",
                "You manage operational readiness. Focus on rollout, incident handling, fallback plans, and postmortem discipline.",
                vec![
                    AgentCapability::Read,
                    AgentCapability::Write,
                    AgentCapability::Edit,
                    AgentCapability::Glob,
                    AgentCapability::Grep,
                    AgentCapability::Shell,
                ],
            ),
        ],
    }
}

fn role_with_capabilities(
    id: &str,
    name: &str,
    output_root: &str,
    description: &str,
    prompt: &str,
    capabilities: Vec<AgentCapability>,
) -> TemplateRoleSpec {
    TemplateRoleSpec {
        id: id.to_string(),
        name: name.to_string(),
        description: None,
        direct: None,
        output_root: Some(output_root.to_string()),
        agent: TemplateRoleAgentSpec {
            description: description.to_string(),
            prompt: prompt.to_string(),
            capabilities: Some(capabilities),
            model: None,
            skills: None,
            initial_prompt: None,
            requires_edit_access: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instantiate_workspace_applies_profile_and_maps_capabilities() {
        let template = create_coding_studio_template();
        let instance = WorkspaceInstanceParams {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            cwd: Some("/tmp/demo".to_string()),
        };
        let profile = create_claude_workspace_profile(None);

        let spec = instantiate_workspace(&template, &instance, &profile);

        assert_eq!(spec.provider, MultiAgentProvider::ClaudeAgentSdk);
        assert_eq!(spec.model, "claude-sonnet-4-5");
        assert_eq!(spec.default_role_id.as_deref(), Some("pm"));
        assert_eq!(spec.setting_sources, Some(vec![SettingSource::Project]));

        let coder = spec.roles.iter().find(|role| role.id == "coder").unwrap();
        assert_eq!(
            coder.agent.tools.as_ref().unwrap(),
            &vec![
                "Bash".to_string(),
                "Edit".to_string(),
                "Glob".to_string(),
                "Grep".to_string(),
                "Read".to_string(),
                "Write".to_string(),
            ]
        );
        assert_eq!(coder.agent.permission_mode, Some(PermissionMode::AcceptEdits));
    }

    #[test]
    fn codex_profile_uses_codex_provider_defaults() {
        let template = create_autoresearch_template();
        let instance = WorkspaceInstanceParams {
            id: "research".to_string(),
            name: "Research".to_string(),
            cwd: None,
        };
        let profile = create_codex_workspace_profile(None);

        let spec = instantiate_workspace(&template, &instance, &profile);

        assert_eq!(spec.provider, MultiAgentProvider::CodexSdk);
        assert_eq!(spec.model, "gpt-5.1-codex-mini");
        assert!(spec.allowed_tools.as_ref().unwrap().contains(&"WebSearch".to_string()));
        assert!(spec.allowed_tools.as_ref().unwrap().contains(&"WebFetch".to_string()));
    }
}
