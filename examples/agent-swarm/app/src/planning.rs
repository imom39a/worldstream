//! Bounded, non-authoritative decisions from a native planning Invocation.
//!
//! A decision selects retained application options. It is never a Room Action
//! or evidence that the Goal has been completed.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PLANNING_DECISION_SCHEMA: &str = "worldstream/agent-swarm-planning-decision@1";
const MAX_DECISION_BYTES: usize = 1024 * 1024;
const MAX_STEPS: usize = 16;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_INSTRUCTION_BYTES: usize = 64 * 1024;
const MAX_REASON_BYTES: usize = 8 * 1024;
const MAX_DEPENDENCIES: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningDecision {
    pub schema: String,
    pub decision: PlanningDecisionKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanningDecisionKind {
    Dispatch {
        steps: Vec<SelectedStep>,
        reason: String,
    },
    /// Select one exact application operation admitted by the local delivery
    /// policy. This never accepts model-authored commands or Room payloads.
    Operate {
        target_id: String,
        reason: String,
    },
    Wait {
        reason: String,
    },
    Handoff {
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedStep {
    pub member_key: String,
    pub target_id: String,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_ids: Option<Vec<String>>,
}

impl PlanningDecision {
    /// Checks the transport bounds independently of retained option authority.
    ///
    /// # Errors
    /// Rejects malformed or oversized decisions and duplicate dispatch targets.
    pub fn validate(&self) -> Result<(), PlanningDecisionError> {
        if self.schema != PLANNING_DECISION_SCHEMA {
            return Err(PlanningDecisionError::InvalidOutput);
        }
        let reason = match &self.decision {
            PlanningDecisionKind::Dispatch { steps, reason } => {
                if steps.is_empty() || steps.len() > MAX_STEPS {
                    return Err(PlanningDecisionError::InvalidOutput);
                }
                let mut members = BTreeSet::new();
                let mut targets = BTreeSet::new();
                for step in steps {
                    if !valid_identifier(&step.member_key)
                        || !valid_identifier(&step.target_id)
                        || !bounded_text(&step.instruction, MAX_INSTRUCTION_BYTES)
                        || !members.insert(&step.member_key)
                        || !targets.insert(&step.target_id)
                        || step.dependency_ids.as_ref().is_some_and(|ids| {
                            ids.len() > MAX_DEPENDENCIES
                                || ids.iter().any(|id| !valid_identifier(id))
                                || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
                        })
                    {
                        return Err(PlanningDecisionError::InvalidOutput);
                    }
                }
                reason
            }
            PlanningDecisionKind::Operate { target_id, reason } => {
                if !valid_identifier(target_id) {
                    return Err(PlanningDecisionError::InvalidOutput);
                }
                reason
            }
            PlanningDecisionKind::Wait { reason } | PlanningDecisionKind::Handoff { reason } => {
                reason
            }
        };
        if !bounded_text(reason, MAX_REASON_BYTES)
            || serde_json::to_vec(self)
                .map_err(|_| PlanningDecisionError::InvalidOutput)?
                .len()
                > MAX_DECISION_BYTES
        {
            return Err(PlanningDecisionError::InvalidOutput);
        }
        Ok(())
    }
}

/// Decodes exactly one final planning answer, without Markdown or extra fields.
///
/// # Errors
/// Rejects any answer outside the strict bounded planning contract.
pub fn decode_planning_decision(text: &str) -> Result<PlanningDecision, PlanningDecisionError> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_DECISION_BYTES {
        return Err(PlanningDecisionError::InvalidOutput);
    }
    let decision: PlanningDecision =
        serde_json::from_str(text).map_err(|_| PlanningDecisionError::InvalidOutput)?;
    decision.validate()?;
    Ok(decision)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn bounded_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.contains('\0')
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PlanningDecisionError {
    #[error("the provider returned an invalid planning decision")]
    InvalidOutput,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn dispatch() -> Value {
        json!({
            "schema": PLANNING_DECISION_SCHEMA,
            "decision": {
                "kind": "dispatch",
                "steps": [
                    { "member_key": "worker-a", "target_id": "work-a", "instruction": "Investigate A" },
                    { "member_key": "worker-b", "target_id": "work-b", "instruction": "Investigate B", "dependency_ids": ["work-a"] }
                ],
                "reason": "The two investigations have independent outputs."
            }
        })
    }

    #[test]
    fn decodes_parallel_dispatch_and_explicit_non_action_decisions()
    -> Result<(), PlanningDecisionError> {
        let parsed = decode_planning_decision(&dispatch().to_string())?;
        let PlanningDecisionKind::Dispatch { steps, .. } = parsed.decision else {
            unreachable!("dispatch expected");
        };
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[1].dependency_ids.as_deref(),
            Some(["work-a".to_owned()].as_slice())
        );
        for kind in ["wait", "handoff"] {
            let value = json!({"schema": PLANNING_DECISION_SCHEMA, "decision": {"kind": kind, "reason": "Await an independent review."}});
            assert!(decode_planning_decision(&value.to_string()).is_ok());
        }
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_or_unbounded_dispatches() {
        type Mutation = fn(&mut Value);
        let cases: [Mutation; 11] = [
            |value| {
                value["schema"] = json!("other");
            },
            |value| {
                value["extra"] = json!(true);
            },
            |value| {
                value["decision"]["steps"] = json!([]);
            },
            |value| {
                value["decision"]["steps"][1]["member_key"] = json!("worker-a");
            },
            |value| {
                value["decision"]["steps"][1]["target_id"] = json!("work-a");
            },
            |value| {
                value["decision"]["steps"][0]["model"] = json!("override");
            },
            |value| {
                value["decision"]["steps"][0]["instruction"] =
                    json!("x".repeat(MAX_INSTRUCTION_BYTES + 1));
            },
            |value| {
                value["decision"]["steps"][1]["dependency_ids"] = json!(["work-a", "work-a"]);
            },
            |value| {
                value["decision"]["steps"][0]["target_id"] = json!("../escape");
            },
            |value| {
                value["decision"]["reason"] = json!(" ");
            },
            |value| {
                value["decision"]["kind"] = json!("complete_goal");
            },
        ];
        for mutate in cases {
            let mut value = dispatch();
            mutate(&mut value);
            assert!(decode_planning_decision(&value.to_string()).is_err());
        }
        let valid = dispatch().to_string();
        assert!(decode_planning_decision(&format!("```json\n{valid}\n```")).is_err());
        assert!(decode_planning_decision(&format!("{valid} {{}}")).is_err());
    }
}
