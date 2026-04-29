#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow {
        source: &'static str,
        reason: String,
    },
    Deny {
        source: &'static str,
        reason: String,
    },
}

pub fn rule_matches(rule: &str, tool: &str, args: &str) -> bool {
    let rule = rule.trim();
    if rule.is_empty() {
        return false;
    }
    if rule == "*" {
        return true;
    }

    if let Some((rule_tool, rule_content)) = rule.split_once(':') {
        if rule_tool.trim() != tool {
            return false;
        }
        let content = rule_content.trim();
        if content.is_empty() || content == "*" {
            return true;
        }
        return args.trim_start().starts_with(content);
    }

    rule == tool
}

pub fn evaluate_rule_permissions(
    tool: &str,
    args: &str,
    allow_rules: &[String],
    deny_rules: &[String],
) -> PermissionDecision {
    if let Some(rule) = deny_rules
        .iter()
        .find(|rule| rule_matches(rule.as_str(), tool, args))
    {
        return PermissionDecision::Deny {
            source: "rule",
            reason: format!("blocked by deny rule `{}`", rule),
        };
    }

    if !allow_rules.is_empty()
        && !allow_rules
            .iter()
            .any(|rule| rule_matches(rule.as_str(), tool, args))
    {
        return PermissionDecision::Deny {
            source: "rule",
            reason: "blocked: tool call does not match allow rules".to_string(),
        };
    }

    PermissionDecision::Allow {
        source: "rule",
        reason: "allowed by rule set".to_string(),
    }
}

#[allow(dead_code)]
pub fn check_rule_permissions(
    tool: &str,
    args: &str,
    allow_rules: &[String],
    deny_rules: &[String],
) -> Option<String> {
    match evaluate_rule_permissions(tool, args, allow_rules, deny_rules) {
        PermissionDecision::Allow { .. } => None,
        PermissionDecision::Deny { reason, .. } => Some(reason),
    }
}
