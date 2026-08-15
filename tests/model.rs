//! Canonical model contract tests: field presence, agent_id opacity,
//! serialization roundtrip, flat keyed records.

use corrald::core::model::*;

fn sample_agent() -> Agent {
    Agent {
        agent_id: "herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784".to_string(),
        source: "herdr".to_string(),
        tool: "claude".to_string(),
        state: AgentState::Blocked,
        reason: Some("waiting_for_approval".to_string()),
        seq: 7,
        ts: 1_755_273_000_000,
        capabilities: CAPABILITIES.iter().map(|s| s.to_string()).collect(),
        waiting_on: Some(WaitingOn {
            kind: WaitingOnKind::ApproveTool,
            prompt: "Approve this change?".to_string(),
            prompt_hash: "sha256:abc".to_string(),
            choices: vec!["Approve".to_string(), "Reject".to_string()],
        }),
        cost: None,
        parent_id: None,
        host: None,
        workspace: Workspace {
            repo: None,
            branch: None,
            worktree_path: Some(
                "/Users/jirathip/.herdr/worktrees/project-hearthwild/feat-x".to_string(),
            ),
            pr_number: None,
        },
        attachment: Some(Attachment {
            kind: "herdr-pane".to_string(),
            reference: "wQ:p1".to_string(),
        }),
        display_name: Some("fix-plush-50".to_string()),
        title: Some("Fix Blender acceptance gate".to_string()),
    }
}

#[test]
fn roundtrip_serialization() {
    let agent = sample_agent();
    let v = serde_json::to_value(&agent).unwrap();
    let back: Agent = serde_json::from_value(v).unwrap();
    assert_eq!(back, agent);
}

#[test]
fn attachment_holds_pane_ref_not_agent_id() {
    let v = serde_json::to_value(sample_agent()).unwrap();
    let obj = v.as_object().unwrap();
    assert_eq!(
        obj["agent_id"].as_str().unwrap(),
        "herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784"
    );
    assert_eq!(obj["attachment"]["kind"].as_str().unwrap(), "herdr-pane");
    assert_eq!(obj["attachment"]["ref"].as_str().unwrap(), "wQ:p1");
    assert_ne!(
        obj["agent_id"], obj["attachment"]["ref"],
        "agent_id is opaque, not the pane ref"
    );
}

#[test]
fn capability_contract_is_exact() {
    let agent = sample_agent();
    assert_eq!(
        agent.capabilities,
        vec![
            "prompt",
            "interrupt",
            "approve",
            "read_tail",
            "kill",
            "attach"
        ]
    );
}

#[test]
fn state_and_waiting_kind_enum_strings() {
    let agent = sample_agent();
    let v = serde_json::to_value(&agent).unwrap();
    assert_eq!(v["state"], "blocked");
    assert_eq!(v["waiting_on"]["kind"], "approve_tool");
    let all = serde_json::to_value(agent).unwrap();
    assert_eq!(all["seq"].as_u64(), Some(7));
    assert_eq!(all["cost"], serde_json::Value::Null);
    assert_eq!(all["parent_id"], serde_json::Value::Null);
    assert_eq!(all["host"], serde_json::Value::Null);
}

#[test]
fn snapshot_is_versioned_flat_keyed_records() {
    let mut agents = std::collections::BTreeMap::new();
    agents.insert("herdr:a".to_string(), sample_agent());
    let snap = Snapshot {
        schema_version: SCHEMA_VERSION,
        rev: 12,
        generated_at: 1_755_273_000_000,
        agents,
    };
    let v = serde_json::to_value(&snap).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["rev"], 12);
    assert!(v["agents"].is_object(), "agents must be flat keyed records");
}

#[test]
fn delta_is_flat_with_upd_del() {
    let d = Delta {
        rev: 13,
        upd: vec![sample_agent()],
        del: vec!["herdr:gone".to_string()],
    };
    let v = serde_json::to_value(&d).unwrap();
    assert_eq!(v["rev"], 13);
    assert_eq!(
        v["upd"][0]["agent_id"],
        "herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784"
    );
    assert_eq!(v["del"][0], "herdr:gone");
}

#[test]
fn herdr_status_mapping_covers_five_tools_states() {
    // AC4: the canonical state vocabulary must cover every tool's status
    // space without per-tool enums — claude/codex/opencode/gemini all report
    // through herdr's AgentStatus, mapped 1:1 here.
    for (herdr, expected) in [
        ("idle", AgentState::Idle),
        ("working", AgentState::Working),
        ("blocked", AgentState::Blocked),
        ("done", AgentState::Done),
        ("unknown", AgentState::Unknown),
    ] {
        assert_eq!(AgentState::from_herdr_status(herdr), expected);
    }
}
