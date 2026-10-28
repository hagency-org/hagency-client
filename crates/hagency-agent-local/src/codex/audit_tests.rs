use super::*;
use crate::audit::Phase;
const OWNER: &str = "@owner:example.org";
fn fixture(
    tool: &str,
    args: Value,
) -> (
    Ledger,
    Session<tokio::io::DuplexStream, tokio::io::DuplexStream>,
    String,
) {
    let mut ledger = Ledger::open(":memory:", OWNER).unwrap();
    let scope = Scope {
        agent: "agent".into(),
        binding: "binding".into(),
        room: "!room:example.org".into(),
        requester: OWNER.into(),
        thread: "thread".into(),
    };
    ledger.register_binding(OWNER, &scope).unwrap();
    let p = ToolProposal {
        scope: scope.clone(),
        dispatch: "dispatch".into(),
        tool: tool.into(),
        arguments: args,
        canonical_directory: "/workspace".into(),
        risk: "high".into(),
        policy_revision: [0; 3],
        expires: now() + 300,
    };
    let id = ledger.begin_approval_audit(OWNER, &p, now()).unwrap();
    for phase in [Phase::OwnerAllowed, Phase::Applying, Phase::Submitted] {
        ledger
            .record_approval_event(OWNER, &id, phase, now(), &value!({}))
            .unwrap();
    }
    let (read, write) = tokio::io::duplex(4096);
    let mut session = Session::new(read, write);
    session.bound_scope = Some(scope);
    session.audit_pending.insert(id.clone());
    session.audit_items.insert("item".into(), vec![id.clone()]);
    (ledger, session, id)
}
#[test]
fn native_command_receipt_requires_original_item_command_status_and_exit_code() {
    for (command, status, exit, expected) in [
        ("echo original", "completed", Some(0), Phase::Completed),
        ("echo changed", "completed", Some(0), Phase::Unknown),
        ("echo original", "inProgress", Some(0), Phase::Unknown),
        ("echo original", "completed", Some(2), Phase::Failed),
        ("echo original", "completed", None, Phase::Unknown),
    ] {
        let (mut ledger, mut session, id) = fixture(
            "codex.command",
            value!({"command":"echo original","itemId":"item"}),
        );
        session.observe_audit_item(&mut ledger,OWNER,&value!({"id":"item","type":"commandExecution","command":command,"status":status,"exitCode":exit})).unwrap();
        assert_eq!(ledger.approval_phase(OWNER, &id).unwrap(), expected);
    }
    let (mut ledger, mut session, id) = fixture(
        "codex.command",
        value!({"command":"echo original","itemId":"item"}),
    );
    session.observe_audit_item(&mut ledger,OWNER,&value!({"id":"unrelated","type":"commandExecution","command":"echo original","status":"completed","exitCode":0})).unwrap();
    assert_eq!(ledger.approval_phase(OWNER, &id).unwrap(), Phase::Submitted);
    session.finish_native_audits(&mut ledger, OWNER).unwrap();
    assert_eq!(ledger.approval_phase(OWNER, &id).unwrap(), Phase::Unknown);
}
#[test]
fn file_and_mcp_receipts_do_not_complete_a_different_effect() {
    let changes = value!([{"path":"a.txt","diff":"exact diff"}]);
    let (mut ledger, mut session, id) = fixture(
        "codex.file_change",
        value!({"request":{"itemId":"item"},"item":{"changes":changes}}),
    );
    session.observe_audit_item(&mut ledger,OWNER,&value!({"id":"item","type":"fileChange","changes":[{"path":"other"}],"status":"completed"})).unwrap();
    assert_eq!(ledger.approval_phase(OWNER, &id).unwrap(), Phase::Unknown);
    let approved =
        value!({"id":"item","server":"explicit","tool":"write","arguments":{"path":"a"}});
    for (actual, expected) in [
        (approved.clone(), Phase::Completed),
        (
            value!({"id":"item","server":"explicit","tool":"write","arguments":{"path":"other"}}),
            Phase::Unknown,
        ),
    ] {
        let (mut ledger, mut session, id) =
            fixture("mcp.explicit.write", value!({"item":approved}));
        let mut receipt = actual;
        receipt["type"] = "mcpToolCall".into();
        receipt["status"] = "completed".into();
        receipt["error"] = Value::Null;
        session
            .observe_audit_item(&mut ledger, OWNER, &receipt)
            .unwrap();
        assert_eq!(ledger.approval_phase(OWNER, &id).unwrap(), expected);
    }
}
