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
            approval_id: "herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784:sha256:abc".to_string(),
            choices: vec!["Approve".to_string(), "Reject".to_string()],
        }),
        parent_id: None,
        host: None,
        workspace: Workspace {
            repo: Some("project-hearthwild".to_string()),
            branch: Some("feat-x".to_string()),
            worktree_path: Some(
                "/Users/jirathip/.herdr/worktrees/project-hearthwild/feat-x".to_string(),
            ),
            pr_number: Some(42),
            ci_status: Some(CiStatus::Success),
            dirty: true,
            ahead: 3,
            behind: 2,
            head_sha: Some("a1b3f9c48b8e9cfbe7f42ee64f4e8cd8f5f6b9a2".to_string()),
            head_subject: Some("Fix Blender acceptance gate".to_string()),
            pr_match_source: None,
            issues: Vec::new(),
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
            "read_diff",
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
        fleet_health: Vec::new(),
    };
    let v = serde_json::to_value(&snap).unwrap();
    assert_eq!(v["schema_version"], SCHEMA_VERSION);
    // v4 (P4 G21): Workspace gained `head_sha`/`head_subject` — versioned
    // strictly.
    assert_eq!(v["schema_version"], 5);
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

#[test]
fn ci_status_serializes_snake_case() {
    for (status, expected) in [
        (CiStatus::Success, "success"),
        (CiStatus::Failure, "failure"),
        (CiStatus::Pending, "pending"),
        (CiStatus::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), expected);
        let back: CiStatus = serde_json::from_value(serde_json::json!(expected)).unwrap();
        assert_eq!(back, status);
    }
}

#[test]
fn workspace_read_model_fields_serialize() {
    // P2 task-centric read-model fields (D7) + P4 G21 head fields must be
    // present on the wire with the canonical spellings, inside `workspace`
    // where P1 clients look.
    let agent = sample_agent();
    let v = serde_json::to_value(&agent).unwrap();
    let ws = &v["workspace"];
    assert_eq!(ws["repo"], "project-hearthwild");
    assert_eq!(ws["branch"], "feat-x");
    assert_eq!(ws["pr_number"], 42);
    assert_eq!(ws["ci_status"], "success");
    assert_eq!(ws["dirty"], true);
    assert_eq!(ws["ahead"], 3);
    assert_eq!(ws["behind"], 2);
    assert_eq!(ws["head_sha"], "a1b3f9c48b8e9cfbe7f42ee64f4e8cd8f5f6b9a2");
    assert_eq!(ws["head_subject"], "Fix Blender acceptance gate");
    // G23 fields: `issues` is always present (empty array when no links —
    // acceptance #23-2); `pr_match_source` is debug-only and omitted when
    // unbound.
    assert_eq!(ws["issues"], serde_json::json!([]));
    assert!(
        !v["workspace"]
            .as_object()
            .unwrap()
            .contains_key("pr_match_source"),
        "unbound agents omit the debug match source"
    );
}

#[test]
fn g23_workspace_fields_round_trip() {
    // A bound agent serializes pr_match_source + issues and decodes back.
    let agent = Agent {
        workspace: Workspace {
            repo: Some("herdr-board".to_string()),
            branch: Some("ws2/gh-plane".to_string()),
            worktree_path: Some(
                "/Users/jirathip/.herdr/worktrees/herdr-board/corral-g23".to_string(),
            ),
            pr_number: Some(42),
            ci_status: Some(CiStatus::Pending),
            pr_match_source: Some("branch".to_string()),
            issues: vec![GhIssueRef {
                repo: "herdr-board".to_string(),
                number: 22,
                state: "OPEN".to_string(),
                title: "PR badges: add headRefName to gh fragment".to_string(),
                labels: vec![],
                url: String::new(),
                body: None,
                comments: vec![],
                comment_total: None,
            }],
            ..Default::default()
        },
        ..sample_agent()
    };
    let v = serde_json::to_value(&agent).unwrap();
    let ws = &v["workspace"];
    assert_eq!(ws["pr_match_source"], "branch");
    assert_eq!(ws["issues"][0]["number"], 22);
    assert_eq!(
        ws["issues"][0]["title"],
        "PR badges: add headRefName to gh fragment"
    );
    let back: Agent = serde_json::from_value(v).expect("round trip");
    assert_eq!(
        back.workspace.pr_match_source,
        agent.workspace.pr_match_source
    );
    assert_eq!(back.workspace.issues, agent.workspace.issues);
}

#[test]
fn g23_payload_without_new_fields_decodes_with_defaults() {
    // A v3-shaped workspace with the G23 fields absent still decodes
    // (additive-only: pr_match_source -> None, issues -> empty).
    let agent: Agent = serde_json::from_str(
        r#"{
            "agent_id": "herdr:a",
            "source": "herdr",
            "tool": "claude",
            "state": "working",
            "reason": null,
            "seq": 1,
            "ts": 1755273000000,
            "capabilities": ["prompt"],
            "waiting_on": null,
            "parent_id": null,
            "host": null,
            "workspace": {"repo": "herdr-board", "branch": "ws2/gh-plane",
                          "worktree_path": "/wt/a", "pr_number": 42,
                          "ci_status": "pending", "dirty": false, "ahead": 0, "behind": 0},
            "attachment": null,
            "display_name": null,
            "title": null
        }"#,
    )
    .expect("v3-shaped agent decodes");
    assert_eq!(agent.workspace.pr_match_source, None);
    assert!(agent.workspace.issues.is_empty());
}

#[test]
fn backwards_compat_p1_agent_decodes_without_read_model_fields() {
    // Older P1-shaped agent records with the v2 fields absent still decode,
    // with defaults.
    let p1_agent = r#"{
        "agent_id": "herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784",
        "source": "herdr",
        "tool": "claude",
        "state": "blocked",
        "reason": "waiting_for_approval",
        "seq": 7,
        "ts": 1755273000000,
        "capabilities": ["prompt", "interrupt", "approve", "read_tail", "kill", "attach"],
        "waiting_on": {"kind": "approve_tool", "prompt": "Approve this change?", "prompt_hash": "sha256:abc", "choices": ["Approve", "Reject"]},
        "parent_id": null,
        "host": null,
        "workspace": {"repo": null, "branch": null, "worktree_path": "/Users/jirathip/.herdr/worktrees/project-hearthwild/feat-x", "pr_number": null},
        "attachment": {"kind": "herdr-pane", "ref": "wQ:p1"},
        "display_name": "fix-plush-50",
        "title": "Fix Blender acceptance gate"
    }"#;
    let agent: Agent = serde_json::from_str(p1_agent).expect("P1 agent must decode under v2");
    assert_eq!(agent.workspace.ci_status, None);
    assert!(!agent.workspace.dirty);
    assert_eq!(agent.workspace.ahead, 0);
    assert_eq!(agent.workspace.behind, 0);
    assert_eq!(
        agent.workspace.worktree_path.as_deref(),
        Some("/Users/jirathip/.herdr/worktrees/project-hearthwild/feat-x")
    );
}

#[test]
fn backwards_compat_p1_snapshot_decodes() {
    // The full P1 wire shape (schema_version 1, no read-model fields) decodes
    // under v2 with every agent record intact and the new fields defaulted.
    let p1_snapshot = r#"{
        "schema_version": 1,
        "rev": 12,
        "generated_at": 1755273000000,
        "agents": {
            "herdr:a": {
                "agent_id": "herdr:a",
                "source": "herdr",
                "tool": "claude",
                "state": "working",
                "reason": null,
                "seq": 1,
                "ts": 1755273000000,
                "capabilities": ["prompt"],
                "waiting_on": null,
                "parent_id": null,
                "host": null,
                "workspace": {"repo": null, "branch": null, "worktree_path": "/wt/a", "pr_number": null},
                "attachment": null,
                "display_name": null,
                "title": null
            }
        }
    }"#;
    let v: serde_json::Value = serde_json::from_str(p1_snapshot).unwrap();
    assert_eq!(
        v["schema_version"], 1,
        "P1 snapshot keeps its own version tag"
    );
    let agents: std::collections::BTreeMap<String, Agent> =
        serde_json::from_value(v["agents"].clone()).expect("P1 agents decode under v2");
    let a = &agents["herdr:a"];
    assert_eq!(a.workspace.ci_status, None);
    assert!(!a.workspace.dirty);
    assert_eq!((a.workspace.ahead, a.workspace.behind), (0, 0));
}
