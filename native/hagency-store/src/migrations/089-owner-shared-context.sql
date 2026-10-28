-- Immutable context descriptions and input pins. No bearer/process authority.
CREATE TABLE owner_shared_input_modes (
 work_id TEXT PRIMARY KEY REFERENCES owner_shared_work_inbox(id),
 mode TEXT NOT NULL CHECK(mode IN('plain_text_v1','thread_context_v1')),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TABLE owner_shared_contexts (
 work_id TEXT PRIMARY KEY REFERENCES owner_shared_input_modes(work_id),
 assignment_id TEXT NOT NULL,
 room_id TEXT NOT NULL,
 thread_root TEXT NOT NULL,
 ordinal INTEGER NOT NULL CHECK(ordinal>0 AND ordinal<=9007199254740991),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=98304),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 UNIQUE(assignment_id,room_id,thread_root,ordinal)
) STRICT;
CREATE TABLE owner_shared_reply_deliveries (
 work_id TEXT PRIMARY KEY REFERENCES owner_shared_work_inbox(id),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=65536),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TABLE owner_shared_context_call_pins (
 call_id TEXT PRIMARY KEY REFERENCES owner_managed_calls(id),
 work_id TEXT NOT NULL UNIQUE REFERENCES owner_shared_contexts(work_id),
 context_digest TEXT NOT NULL CHECK(length(context_digest)=64),
 input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TRIGGER owner_shared_context_mode BEFORE INSERT ON owner_shared_contexts WHEN NOT EXISTS(SELECT 1 FROM owner_shared_input_modes WHERE work_id=NEW.work_id AND mode='thread_context_v1') BEGIN SELECT RAISE(ABORT,'context mode required'); END;
CREATE TRIGGER owner_shared_input_before_call BEFORE INSERT ON owner_shared_input_modes WHEN EXISTS(SELECT 1 FROM owner_managed_calls c JOIN owner_shared_work_inbox w ON c.execution_id=w.execution_id WHERE w.id=NEW.work_id) BEGIN SELECT RAISE(ABORT,'input mode before call required'); END;
CREATE TRIGGER owner_shared_context_before_call BEFORE INSERT ON owner_shared_contexts WHEN EXISTS(SELECT 1 FROM owner_managed_calls c JOIN owner_shared_work_inbox w ON c.execution_id=w.execution_id WHERE w.id=NEW.work_id) BEGIN SELECT RAISE(ABORT,'context before call required'); END;
CREATE TRIGGER owner_shared_context_pin_exact BEFORE INSERT ON owner_shared_context_call_pins WHEN NOT EXISTS(SELECT 1 FROM owner_shared_contexts x JOIN owner_shared_work_inbox w ON w.id=x.work_id JOIN owner_managed_calls c ON c.execution_id=w.execution_id AND c.child_id=w.child_id WHERE x.work_id=NEW.work_id AND c.id=NEW.call_id AND c.state='prepared' AND x.digest=NEW.context_digest AND x.input_digest=NEW.input_digest) BEGIN SELECT RAISE(ABORT,'exact prepared context call required'); END;
CREATE TRIGGER owner_shared_input_modes_immutable BEFORE UPDATE ON owner_shared_input_modes BEGIN SELECT RAISE(ABORT,'immutable shared context'); END;
CREATE TRIGGER owner_shared_input_modes_retained BEFORE DELETE ON owner_shared_input_modes BEGIN SELECT RAISE(ABORT,'retained shared context'); END;
CREATE TRIGGER owner_shared_contexts_immutable BEFORE UPDATE ON owner_shared_contexts BEGIN SELECT RAISE(ABORT,'immutable shared context'); END;
CREATE TRIGGER owner_shared_contexts_retained BEFORE DELETE ON owner_shared_contexts BEGIN SELECT RAISE(ABORT,'retained shared context'); END;
CREATE TRIGGER owner_shared_reply_deliveries_immutable BEFORE UPDATE ON owner_shared_reply_deliveries BEGIN SELECT RAISE(ABORT,'immutable shared context'); END;
CREATE TRIGGER owner_shared_reply_deliveries_retained BEFORE DELETE ON owner_shared_reply_deliveries BEGIN SELECT RAISE(ABORT,'retained shared context'); END;
CREATE TRIGGER owner_shared_context_call_pins_immutable BEFORE UPDATE ON owner_shared_context_call_pins BEGIN SELECT RAISE(ABORT,'immutable shared context'); END;
CREATE TRIGGER owner_shared_context_call_pins_retained BEFORE DELETE ON owner_shared_context_call_pins BEGIN SELECT RAISE(ABORT,'retained shared context'); END;
