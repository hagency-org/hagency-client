-- Results share the domain transaction, so a lost acknowledgement cannot create
-- another resource or replay a configuration under a different request body.
CREATE TABLE management_commands (
    actor_scope TEXT NOT NULL,
    command_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    digest TEXT NOT NULL,
    result TEXT NOT NULL,
    PRIMARY KEY(actor_scope, command_id)
);

CREATE TABLE management_identity (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    profile_digest TEXT NOT NULL CHECK(length(profile_digest)=64)
);
