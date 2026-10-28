-- Native-only canonical graph custody. Outbox rows are deliberately not runtime
-- dispatches: current Matrix/project/agent binding is still required.
CREATE TABLE owner_operator_graphs (
 registry_id TEXT NOT NULL CHECK(length(registry_id)=64),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 owner_mxid TEXT NOT NULL,
 scope_id TEXT NOT NULL CHECK(length(scope_id) BETWEEN 1 AND 255),
 graph_id TEXT NOT NULL CHECK(length(graph_id) BETWEEN 1 AND 255),
 revision INTEGER NOT NULL CHECK(revision>0),
 document BLOB NOT NULL CHECK(typeof(document)='blob' AND length(document)<=4194304),
 document_digest TEXT NOT NULL CHECK(length(document_digest)=64),
 PRIMARY KEY(registry_id,profile,owner_mxid,scope_id,graph_id)
);
CREATE TABLE owner_operator_graph_commands (
 profile TEXT NOT NULL CHECK(length(profile)=64),
 command_id TEXT NOT NULL CHECK(length(command_id) BETWEEN 1 AND 128),
 registry_id TEXT NOT NULL CHECK(length(registry_id)=64),
 owner_mxid TEXT NOT NULL,
 scope_id TEXT NOT NULL,
 graph_id TEXT NOT NULL,
 expected_revision INTEGER NOT NULL CHECK(expected_revision>=0),
 input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
 request_digest TEXT NOT NULL CHECK(length(request_digest)=64),
 receipt TEXT NOT NULL CHECK(length(CAST(receipt AS BLOB))<=4096),
 PRIMARY KEY(profile,command_id),
 FOREIGN KEY(registry_id,profile,owner_mxid,scope_id,graph_id)
 REFERENCES owner_operator_graphs(registry_id,profile,owner_mxid,scope_id,graph_id)
 DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE owner_operator_graph_outbox (
 registry_id TEXT NOT NULL CHECK(length(registry_id)=64),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 owner_mxid TEXT NOT NULL,
 scope_id TEXT NOT NULL,
 graph_id TEXT NOT NULL,
 node_id TEXT NOT NULL CHECK(length(node_id) BETWEEN 1 AND 255),
 message_id TEXT NOT NULL UNIQUE CHECK(length(message_id)=64),
 payload BLOB NOT NULL CHECK(typeof(payload)='blob' AND length(payload)<=4194304),
 payload_digest TEXT NOT NULL CHECK(length(payload_digest)=64),
 state TEXT NOT NULL CHECK(state IN ('pending_binding','suppressed')),
 PRIMARY KEY(registry_id,profile,owner_mxid,scope_id,graph_id,node_id),
 FOREIGN KEY(registry_id,profile,owner_mxid,scope_id,graph_id)
 REFERENCES owner_operator_graphs(registry_id,profile,owner_mxid,scope_id,graph_id)
 DEFERRABLE INITIALLY DEFERRED
);
CREATE TRIGGER owner_operator_graphs_capacity BEFORE INSERT ON owner_operator_graphs
WHEN (SELECT COUNT(*) FROM owner_operator_graphs)>=10000
 AND NOT EXISTS(SELECT 1 FROM owner_operator_graphs WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id)
BEGIN SELECT RAISE(ABORT,'graph capacity'); END;
CREATE TRIGGER owner_operator_graph_commands_capacity BEFORE INSERT ON owner_operator_graph_commands
WHEN (SELECT COUNT(*) FROM owner_operator_graph_commands)>=100000
BEGIN SELECT RAISE(ABORT,'graph command capacity'); END;
CREATE TRIGGER owner_operator_graph_outbox_capacity BEFORE INSERT ON owner_operator_graph_outbox
WHEN (SELECT COUNT(*) FROM owner_operator_graph_outbox WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id)>=256
 OR (SELECT COALESCE(SUM(length(payload)),0) FROM owner_operator_graph_outbox WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id)+length(NEW.payload)>67108864
BEGIN SELECT RAISE(ABORT,'graph outbox capacity'); END;
CREATE TRIGGER owner_operator_graphs_revision BEFORE UPDATE ON owner_operator_graphs
WHEN OLD.registry_id IS NOT NEW.registry_id OR OLD.profile IS NOT NEW.profile
 OR OLD.owner_mxid IS NOT NEW.owner_mxid OR OLD.scope_id IS NOT NEW.scope_id
 OR OLD.graph_id IS NOT NEW.graph_id OR NEW.revision<>OLD.revision+1
BEGIN SELECT RAISE(ABORT,'retain graph identity and revision'); END;
CREATE TRIGGER owner_operator_graphs_retained BEFORE DELETE ON owner_operator_graphs
BEGIN SELECT RAISE(ABORT,'retain graph'); END;
CREATE TRIGGER owner_operator_graph_commands_immutable BEFORE UPDATE ON owner_operator_graph_commands
BEGIN SELECT RAISE(ABORT,'immutable graph command'); END;
CREATE TRIGGER owner_operator_graph_commands_retained BEFORE DELETE ON owner_operator_graph_commands
BEGIN SELECT RAISE(ABORT,'retain graph command'); END;
CREATE TRIGGER owner_operator_graph_outbox_immutable BEFORE UPDATE ON owner_operator_graph_outbox
WHEN OLD.registry_id IS NOT NEW.registry_id OR OLD.profile IS NOT NEW.profile
 OR OLD.owner_mxid IS NOT NEW.owner_mxid OR OLD.scope_id IS NOT NEW.scope_id
 OR OLD.graph_id IS NOT NEW.graph_id OR OLD.node_id IS NOT NEW.node_id
 OR OLD.message_id IS NOT NEW.message_id OR OLD.payload IS NOT NEW.payload
 OR OLD.payload_digest IS NOT NEW.payload_digest OR OLD.state<>'pending_binding'
 OR NEW.state<>'suppressed'
BEGIN SELECT RAISE(ABORT,'retain original graph message'); END;
CREATE TRIGGER owner_operator_graph_outbox_retained BEFORE DELETE ON owner_operator_graph_outbox
BEGIN SELECT RAISE(ABORT,'retain graph message'); END;
