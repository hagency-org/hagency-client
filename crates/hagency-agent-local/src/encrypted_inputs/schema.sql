CREATE TABLE IF NOT EXISTS encrypted_inputs (
 dispatch_id TEXT PRIMARY KEY REFERENCES local_inbox(id) DEFERRABLE INITIALLY DEFERRED,
 inbox_id TEXT NOT NULL UNIQUE, source_digest TEXT NOT NULL, agent_generation INTEGER NOT NULL CHECK(agent_generation>0),
 owner_direct INTEGER NOT NULL CHECK(owner_direct IN (0,1)), seal_digest TEXT NOT NULL UNIQUE,
 ciphertext TEXT, message TEXT, message_digest TEXT NOT NULL, plaintext_digest TEXT NOT NULL, transport_digest TEXT NOT NULL,
 CHECK((ciphertext IS NULL)=(message IS NULL))
);
CREATE TRIGGER IF NOT EXISTS immutable_encrypted_input BEFORE UPDATE ON encrypted_inputs
WHEN NEW.dispatch_id<>OLD.dispatch_id OR NEW.inbox_id<>OLD.inbox_id OR NEW.source_digest<>OLD.source_digest OR NEW.agent_generation<>OLD.agent_generation OR NEW.owner_direct<>OLD.owner_direct OR NEW.seal_digest<>OLD.seal_digest OR NEW.message_digest<>OLD.message_digest OR NEW.plaintext_digest<>OLD.plaintext_digest OR NEW.transport_digest<>OLD.transport_digest OR ((NEW.ciphertext IS NOT OLD.ciphertext OR NEW.message IS NOT OLD.message) AND NOT(OLD.ciphertext IS NOT NULL AND NEW.ciphertext IS NULL AND NEW.message IS NULL AND EXISTS(SELECT 1 FROM local_inbox i WHERE i.id=OLD.dispatch_id AND i.state IN ('replied','failed','rejected') AND json_extract(i.envelope,'$.body')='')))
BEGIN SELECT RAISE(ABORT,'immutable_encrypted_input'); END;
CREATE TRIGGER IF NOT EXISTS retain_encrypted_input BEFORE DELETE ON encrypted_inputs
BEGIN SELECT RAISE(ABORT,'encrypted_input_tombstone_required'); END;
