use std::collections::BTreeSet;

use crate::{RoleSpec, WorkspaceSpec, WorkspaceTurnAssignment, WorkspaceTurnPlan, WorkspaceTurnRequest};

pub fn plan_workspace_turn(spec: &WorkspaceSpec, request: &WorkspaceTurnRequest) -> WorkspaceTurnPlan {
    let coordinator_role_id = resolve_coordinator_role_id(spec);
    let fallback_role_id = spec
        .claim_policy
        .as_ref()
        .and_then(|policy| policy.fallback_role_id.clone())
        .or_else(|| spec.default_role_id.clone())
        .unwrap_or_else(|| coordinator_role_id.clone());
    let max_assignments = usize::from(
        request
            .max_assignments
            .or_else(|| spec.claim_policy.as_ref().and_then(|policy| policy.max_assignees))
            .unwrap_or(1)
            .max(1),
    );

    let assignments = if let Some(preferred_role_id) = request.prefer_role_id.as_ref() {
        vec![build_assignment(preferred_role_id, &request.message, spec)]
    } else {
        let mut scored = spec
            .roles
            .iter()
            .map(|role| {
                (
                    role.id.clone(),
                    score_role_for_message(role, &request.message.to_lowercase()),
                )
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

        let mut selected = scored
            .into_iter()
            .filter(|(_, score)| *score > 0)
            .map(|(role_id, _)| role_id)
            .take(max_assignments)
            .collect::<Vec<_>>();

        if selected.is_empty() {
            selected.push(fallback_role_id.clone());
        }

        let selected = unique_role_ids(selected);
        selected
            .into_iter()
            .map(|role_id| build_assignment(&role_id, &request.message, spec))
            .collect::<Vec<_>>()
    };

    WorkspaceTurnPlan {
        coordinator_role_id,
        response_text: build_response_text(&assignments),
        assignments,
        rationale: Some("Planned from workspace roles, claim policy, and message-role affinity.".to_string()),
    }
}

pub fn direct_workspace_turn_plan(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    role_id: &str,
) -> WorkspaceTurnPlan {
    WorkspaceTurnPlan {
        coordinator_role_id: resolve_coordinator_role_id(spec),
        response_text: format!("@{} will take this next.", role_id),
        assignments: vec![build_assignment(role_id, &request.message, spec)],
        rationale: Some("Direct role targeting bypassed coordinator routing.".to_string()),
    }
}

pub fn resolve_coordinator_role_id(spec: &WorkspaceSpec) -> String {
    spec.coordinator_role_id
        .clone()
        .or_else(|| spec.default_role_id.clone())
        .or_else(|| spec.roles.first().map(|role| role.id.clone()))
        .unwrap_or_else(|| "coordinator".to_string())
}

fn build_assignment(role_id: &str, message: &str, spec: &WorkspaceSpec) -> WorkspaceTurnAssignment {
    let role_name = spec
        .roles
        .iter()
        .find(|role| role.id == role_id)
        .map(|role| role.name.clone())
        .unwrap_or_else(|| role_id.to_string());

    WorkspaceTurnAssignment {
        role_id: role_id.to_string(),
        instruction: message.trim().to_string(),
        summary: Some(format!("Handle workspace request as {}", role_name)),
        visibility: request_visibility_default(spec),
    }
}

fn request_visibility_default(spec: &WorkspaceSpec) -> Option<crate::WorkspaceVisibility> {
    spec.activity_policy
        .as_ref()
        .and_then(|policy| policy.default_visibility)
}

fn build_response_text(assignments: &[WorkspaceTurnAssignment]) -> String {
    let names = assignments
        .iter()
        .map(|assignment| format!("@{}", assignment.role_id))
        .collect::<Vec<_>>();

    match names.as_slice() {
        [] => "The coordinator will take this next.".to_string(),
        [only] => format!("{} will take this next.", only),
        _ => format!("{} will split this work.", names.join(" and ")),
    }
}

fn unique_role_ids(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn score_role_for_message(role: &RoleSpec, lowered_message: &str) -> usize {
    let mut corpus_parts = vec![
        role.id.to_lowercase(),
        role.name.to_lowercase(),
    ];
    if let Some(description) = role.description.as_ref() {
        corpus_parts.push(description.to_lowercase());
    }
    corpus_parts.push(role.agent.description.to_lowercase());
    if let Some(output_root) = role.output_root.as_ref() {
        corpus_parts.push(output_root.to_lowercase());
    }
    if let Some(hints) = role_hints(&role.id) {
        corpus_parts.extend(hints.iter().map(|hint| hint.to_string()));
    }

    build_search_tokens(&corpus_parts.join(" "))
        .into_iter()
        .filter(|token| token.len() >= 3 && lowered_message.contains(token))
        .map(|token| if token.len() > 8 { 3 } else { 1 })
        .sum()
}

fn build_search_tokens(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '@' && ch != '-')
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn role_hints(role_id: &str) -> Option<&'static [&'static str]> {
    match role_id {
        "pm" => Some(&["plan", "milestone", "scope", "coordination"]),
        "prd" => Some(&["prd", "requirement", "requirements", "spec", "user story", "acceptance criteria"]),
        "architect" => Some(&["architecture", "design", "interface", "data model", "technical plan"]),
        "coder" => Some(&["implement", "implementation", "code", "patch", "bug fix"]),
        "tester" => Some(&["test", "qa", "regression", "verification"]),
        "reviewer" => Some(&["review", "audit", "bug finding"]),
        "ceo" => Some(&["priority", "decision", "approval", "strategy"]),
        "finance" => Some(&["finance", "monthly close", "cash", "invoice", "invoices", "subscription", "revenue", "kpi", "burn", "runway", "budget"]),
        "tax" => Some(&["tax", "filing", "sales tax", "vat", "estimated tax"]),
        "admin" => Some(&["admin", "vendor", "operations", "sop", "checklist"]),
        "recruiter" => Some(&["recruit", "candidate", "hiring", "interview"]),
        "lead" => Some(&["research lead", "hypothesis", "question framing"]),
        "scout" => Some(&["research", "sources", "web", "compare", "brief"]),
        "experimenter" => Some(&["experiment", "metric", "measure", "test design"]),
        "critic" => Some(&["critique", "risk", "skeptic", "assumption"]),
        "shangshu" => Some(&["coordination", "governance", "routing"]),
        "zhongshu" => Some(&["brief", "mandate", "task order", "draft"]),
        "menxia" => Some(&["review", "challenge", "risk", "red team"]),
        "gongbu" => Some(&["implementation", "build", "execution", "tooling"]),
        "hubu" => Some(&["budget", "resources", "capacity", "allocation"]),
        "libu" => Some(&["documentation", "communication", "release notes"]),
        "xingbu" => Some(&["compliance", "safety", "policy"]),
        "bingbu" => Some(&["release", "incident", "operations", "rollout"]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        create_coding_studio_template, create_opc_solo_company_template, instantiate_workspace,
        create_claude_workspace_profile, WorkspaceInstanceParams,
    };

    use super::*;

    #[test]
    fn coding_turn_prefers_prd_for_prd_request() {
        let template = create_coding_studio_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-1".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);
        let plan = plan_workspace_turn(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Create a PRD for group mentions with acceptance criteria".to_string(),
                visibility: None,
                max_assignments: None,
                prefer_role_id: None,
            },
        );

        assert_eq!(plan.assignments[0].role_id, "prd");
    }

    #[test]
    fn opc_turn_prefers_finance_for_monthly_close() {
        let template = create_opc_solo_company_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-2".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);
        let plan = plan_workspace_turn(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Prepare a monthly close checklist and review subscriptions".to_string(),
                visibility: None,
                max_assignments: None,
                prefer_role_id: None,
            },
        );

        assert_eq!(plan.assignments[0].role_id, "finance");
    }
}
