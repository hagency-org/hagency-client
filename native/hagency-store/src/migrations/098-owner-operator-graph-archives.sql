-- Lossless selected-root history only. Sealing an archive does not map names
-- to principals, fence old writers or enqueue any task/runtime work.
CREATE TABLE owner_operator_graph_archives (
 command_id TEXT PRIMARY KEY CHECK(length(command_id) BETWEEN 1 AND 128),
 registry_id TEXT NOT NULL CHECK(length(registry_id)=64),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 owner_mxid TEXT NOT NULL,
 source_binding TEXT NOT NULL CHECK(length(source_binding)=64),
 manifest TEXT NOT NULL CHECK(length(CAST(manifest AS BLOB))<=4096),
 state TEXT NOT NULL CHECK(state IN ('copying','archived')),
 UNIQUE(profile,source_binding)
);
CREATE TABLE owner_operator_graph_archive_chunks (
 command_id TEXT NOT NULL REFERENCES owner_operator_graph_archives(command_id),
 document INTEGER NOT NULL CHECK(document BETWEEN 0 AND 2),
 chunk_index INTEGER NOT NULL CHECK(chunk_index BETWEEN 0 AND 63),
 bytes BLOB NOT NULL CHECK(typeof(bytes)='blob' AND length(bytes) BETWEEN 1 AND 1048576),
 PRIMARY KEY(command_id,document,chunk_index)
);
CREATE TRIGGER owner_operator_graph_archive_identity_immutable
BEFORE UPDATE ON owner_operator_graph_archives
WHEN OLD.command_id IS NOT NEW.command_id OR OLD.registry_id IS NOT NEW.registry_id
 OR OLD.profile IS NOT NEW.profile OR OLD.owner_mxid IS NOT NEW.owner_mxid
 OR OLD.source_binding IS NOT NEW.source_binding OR OLD.manifest IS NOT NEW.manifest
 OR OLD.state='archived' OR NEW.state<>'archived'
BEGIN SELECT RAISE(ABORT,'retain original graph archive identity'); END;
CREATE TRIGGER owner_operator_graph_archive_retained
BEFORE DELETE ON owner_operator_graph_archives
BEGIN SELECT RAISE(ABORT,'retain graph archive'); END;
CREATE TRIGGER owner_operator_graph_archive_chunk_immutable
BEFORE UPDATE ON owner_operator_graph_archive_chunks
BEGIN SELECT RAISE(ABORT,'immutable graph archive chunk'); END;
CREATE TRIGGER owner_operator_graph_archive_chunk_retained
BEFORE DELETE ON owner_operator_graph_archive_chunks
BEGIN SELECT RAISE(ABORT,'retain graph archive chunk'); END;
CREATE TRIGGER owner_operator_graph_archive_sealed
BEFORE INSERT ON owner_operator_graph_archive_chunks
WHEN (SELECT state FROM owner_operator_graph_archives WHERE command_id=NEW.command_id)<>'copying'
BEGIN SELECT RAISE(ABORT,'graph archive already sealed'); END;
