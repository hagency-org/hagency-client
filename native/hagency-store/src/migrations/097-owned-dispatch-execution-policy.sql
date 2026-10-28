-- Capture the authorized policy for this exact dispatch. No backfill: a
-- previously started dispatch keeps its original fingerprint and default mode.
CREATE TABLE owned_dispatch_execution_policies (
 dispatch_id TEXT PRIMARY KEY REFERENCES runner_dispatches(id),
 engagement_id TEXT NOT NULL REFERENCES engagements(id),
 yolo INTEGER NOT NULL CHECK(yolo IN (0,1)),
 captured_at INTEGER NOT NULL CHECK(captured_at >= 0 AND captured_at <= 9007199254740991)
) STRICT;
