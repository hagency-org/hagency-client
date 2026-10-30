CREATE TABLE owner_shared_decrypted_inputs (
 work_id TEXT PRIMARY KEY REFERENCES owner_shared_input_modes(work_id),
 work_digest TEXT NOT NULL,
 prompt TEXT NOT NULL,
 body TEXT NOT NULL,
 digest TEXT NOT NULL
);
CREATE TRIGGER owner_shared_decrypted_input_immutable BEFORE UPDATE ON owner_shared_decrypted_inputs BEGIN SELECT RAISE(ABORT,'immutable decrypted input'); END;
CREATE TRIGGER owner_shared_decrypted_input_retained BEFORE DELETE ON owner_shared_decrypted_inputs BEGIN SELECT RAISE(ABORT,'retained decrypted input'); END;
