-- Binding rows may be removed and later reconstructed from room observations.
-- Keep their generation while the engagement exists, so an original approval
-- context can never become current again just because its row was recreated.
CREATE TABLE approval_binding_incarnations (
 engagement_id TEXT PRIMARY KEY REFERENCES engagements(id) ON DELETE CASCADE,
 incarnation INTEGER NOT NULL CHECK(incarnation BETWEEN 1 AND 9007199254740991)
) STRICT;

-- A binding may already have been removed on an older version. Include the
-- durable contexts and grants, preserving their original bytes and authority.
INSERT INTO approval_binding_incarnations(engagement_id,incarnation)
 SELECT engagement_id,max(incarnation) FROM (
  SELECT engagement_id,incarnation FROM approval_bindings
  UNION ALL
  SELECT engagement_id,json_extract(config,'$.binding.generation') FROM approval_contexts
  UNION ALL
  SELECT engagement_id,binding_generation FROM approval_grants
 ) GROUP BY engagement_id
 ON CONFLICT(engagement_id) DO UPDATE SET
  incarnation=max(approval_binding_incarnations.incarnation,excluded.incarnation);
