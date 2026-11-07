//! Genuine schema83 source fixtures for adoption through the original WAL owner.
use rusqlite::Connection;

pub(in crate::console::native) fn rewind_schema83(source: &Connection) {
    let removed = [
        "owner_shared_unstarted_runtimes",
        "owner_resource_calendars",
        "owner_managed_call_periods",
        "owner_managed_periods",
        "owner_shared_decrypted_inputs",
        "owner_resource_ceiling_amendments",
        "owner_capacity_grant_successions",
        "owner_managed_fence_times",
        "owner_shared_budget_demand_terminals",
        "owner_shared_unstarted_attempts",
        "owner_shared_tool_policies",
        "owner_web_catalog_settings",
        "owner_operator_graph_recipients",
        "owner_operator_graph_outbox",
        "owner_operator_graph_commands",
        "owner_operator_graphs",
        "owner_operator_graph_archive_chunks",
        "owner_operator_graph_archives",
        "owned_dispatch_execution_policies",
        "approval_binding_incarnations",
        "owner_resource_provenance",
        "owner_namespace_enrollments",
        "owner_resource_namespaces",
        "management_account_profiles",
        "owner_shared_budget_demand_decisions",
        "owner_shared_budget_work_refusals",
        "owner_shared_budget_start_intents",
        "owner_managed_budget_deltas",
        "owner_managed_budget_commands",
        "owner_operator_task_transfer_comments",
        "owner_operator_task_transfer_tasks",
        "owner_shared_context_call_pins",
        "owner_shared_reply_deliveries",
        "owner_shared_contexts",
        "owner_shared_input_modes",
        "owner_shared_stop_receipts",
        "owner_shared_stop_intents",
        "owner_shared_demand_bindings",
        "owner_shared_demand_decisions",
        "owner_shared_demands",
        "owner_shared_demand_controls",
        "owner_operator_task_comments",
        "owner_operator_tasks",
        "owner_operator_task_histories",
    ];
    let rebuilt = [
        "owner_managed_agents",
        "owner_managed_fences",
        "owner_shared_work_observations",
        "owner_shared_start_intents",
    ];
    // Check every affected table before any DDL so no fixture history is lost.
    // Existing resources, seats, management identity and other history stay put.
    for table in removed.iter().chain(&rebuilt) {
        assert_eq!(
            source
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                    r.get::<_, u64>(0)
                })
                .unwrap(),
            0,
            "{table} must be empty before reconstructing schema83"
        );
    }
    let historical = Connection::open_in_memory().unwrap();
    for migration in [
        include_str!("../../../../../hagency-store/src/migrations/077-owner-managed-agents.sql"),
        include_str!("../../../../../hagency-store/src/migrations/079-owner-managed-fences.sql"),
        include_str!("../../../../../hagency-store/src/migrations/081-owner-managed-runtime.sql"),
        include_str!("../../../../../hagency-store/src/migrations/082-owner-shared-execution.sql"),
        include_str!(
            "../../../../../hagency-store/src/migrations/084-owner-shared-start-intents.sql"
        ),
    ] {
        historical.execute_batch(migration).unwrap();
    }
    // These later triggers are attached to retained tables, not the tables
    // they reference. Remove them before dropping their referenced additions.
    source
        .execute_batch(
            "DROP TRIGGER owner_managed_agent_fence_time;
             DROP TRIGGER owner_resource_amended_ceiling_retained;",
        )
        .unwrap();
    for table in removed {
        source
            .execute_batch(&format!("DROP TABLE {table};"))
            .unwrap();
    }
    for table in rebuilt {
        let definition: String = historical
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        let objects = historical
            .prepare("SELECT sql FROM sqlite_master WHERE tbl_name=?1 AND type IN ('trigger','index') AND sql IS NOT NULL")
            .unwrap()
            .query_map([table], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        source
            .execute_batch(&format!("DROP TABLE {table};{definition};"))
            .unwrap();
        for object in objects {
            source.execute_batch(&object).unwrap();
        }
    }
    assert!(
        !source
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    source.pragma_update(None, "user_version", 83).unwrap();
}
