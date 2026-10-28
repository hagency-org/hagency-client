-- Explicit per-Agent owner opt-in; absence preserves the text-only protocol.
CREATE TABLE owner_shared_tool_policies (
 child_id TEXT PRIMARY KEY REFERENCES owner_managed_agents(id),
 yolo INTEGER NOT NULL CHECK(yolo IN(0,1)),
 revision INTEGER NOT NULL CHECK(revision>0 AND revision<=9007199254740991)
) STRICT;
