-- Native custody precedes external effects. Acknowledgement drops bulky payloads
-- but keeps identity/digest tombstones so a command cannot be silently reused.
CREATE TABLE management_intents (
 actor_scope TEXT NOT NULL,
 command_id TEXT NOT NULL,
 operation TEXT NOT NULL,
 digest TEXT NOT NULL CHECK(length(digest)=64),
 input TEXT,
 reply TEXT,
 state TEXT NOT NULL CHECK(state IN ('pending','replied','acknowledged')),
 PRIMARY KEY(actor_scope,command_id),
 CHECK((state='pending' AND input IS NOT NULL AND reply IS NULL)
    OR (state='replied' AND input IS NOT NULL AND reply IS NOT NULL)
    OR (state='acknowledged' AND input IS NULL AND reply IS NULL))
);
CREATE INDEX management_intents_pending ON management_intents(actor_scope,state,command_id);
