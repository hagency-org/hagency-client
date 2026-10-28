-- A reviewed native recipient snapshot is custody only, never permission to
-- release or rewrite an outbox message. Replacement shares the graph CAS.
CREATE TABLE owner_operator_graph_recipients (
 registry_id TEXT NOT NULL CHECK(length(registry_id)=64),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 owner_mxid TEXT NOT NULL,
 scope_id TEXT NOT NULL,
 graph_id TEXT NOT NULL,
 node_id TEXT NOT NULL CHECK(length(node_id) BETWEEN 1 AND 255),
 revision INTEGER NOT NULL CHECK(revision>1),
 snapshot BLOB NOT NULL CHECK(typeof(snapshot)='blob' AND length(snapshot)<=32768),
 snapshot_digest TEXT NOT NULL CHECK(length(snapshot_digest)=64),
 PRIMARY KEY(registry_id,profile,owner_mxid,scope_id,graph_id,node_id),
 FOREIGN KEY(registry_id,profile,owner_mxid,scope_id,graph_id)
 REFERENCES owner_operator_graphs(registry_id,profile,owner_mxid,scope_id,graph_id)
 DEFERRABLE INITIALLY DEFERRED
);
CREATE TRIGGER owner_operator_graph_recipients_capacity BEFORE INSERT ON owner_operator_graph_recipients
WHEN (SELECT COUNT(*) FROM owner_operator_graph_recipients WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id)>=256
 AND NOT EXISTS(SELECT 1 FROM owner_operator_graph_recipients WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id AND node_id=NEW.node_id)
BEGIN SELECT RAISE(ABORT,'graph recipient capacity'); END;
CREATE TRIGGER owner_operator_graph_recipients_insert BEFORE INSERT ON owner_operator_graph_recipients
WHEN NOT EXISTS(SELECT 1 FROM owner_operator_graphs WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id AND revision=NEW.revision-1)
 OR EXISTS(SELECT 1 FROM owner_operator_graph_outbox WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id AND node_id=NEW.node_id AND state<>'pending_binding')
BEGIN SELECT RAISE(ABORT,'recipient requires reviewed held graph'); END;
CREATE TRIGGER owner_operator_graph_recipients_update BEFORE UPDATE ON owner_operator_graph_recipients
WHEN OLD.registry_id IS NOT NEW.registry_id OR OLD.profile IS NOT NEW.profile
 OR OLD.owner_mxid IS NOT NEW.owner_mxid OR OLD.scope_id IS NOT NEW.scope_id
 OR OLD.graph_id IS NOT NEW.graph_id OR OLD.node_id IS NOT NEW.node_id
 OR NEW.revision<=OLD.revision
 OR NOT EXISTS(SELECT 1 FROM owner_operator_graphs WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id AND revision=NEW.revision-1)
 OR EXISTS(SELECT 1 FROM owner_operator_graph_outbox WHERE registry_id=NEW.registry_id AND profile=NEW.profile AND owner_mxid=NEW.owner_mxid AND scope_id=NEW.scope_id AND graph_id=NEW.graph_id AND node_id=NEW.node_id AND state<>'pending_binding')
BEGIN SELECT RAISE(ABORT,'retain reviewed graph recipient identity'); END;
CREATE TRIGGER owner_operator_graph_recipients_retained BEFORE DELETE ON owner_operator_graph_recipients
BEGIN SELECT RAISE(ABORT,'retain reviewed graph recipient'); END;
