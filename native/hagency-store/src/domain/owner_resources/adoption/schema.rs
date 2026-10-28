//! Exact provenance for the first, deliberately narrow definition-only transfer.
//! Build only an in-memory schema from trusted SQL; never initialize a source.
use super::*;

fn objects(db: &Connection) -> Result<Vec<[String; 3]>, Error> {
    let mut query = db.prepare("SELECT CASE WHEN length(type)<=16 THEN type END, CASE WHEN length(CAST(name AS BLOB))<=128 THEN name END, CASE WHEN length(CAST(sql AS BLOB))<=65536 THEN sql END FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name LIMIT 1025")?;
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for row in query.query_map([], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?]))? {
        let row: [String; 3] = row?;
        bytes = bytes.saturating_add(row.iter().map(String::len).sum::<usize>());
        if result.len() >= 1024 || bytes > 2 * 1024 * 1024 {
            return Err(Error::Capacity);
        }
        result.push(row);
    }
    Ok(result)
}

pub(super) fn validate(
    domain: &Connection,
    personal: &Connection,
    version: i32,
    deadline: Instant,
    archived: (bool, bool),
) -> Result<(), Error> {
    validate_domain(domain, version, deadline, archived.0)?;
    validate_personal(personal, deadline, archived.1)
}

pub(super) fn validate_domain(
    domain: &Connection,
    version: i32,
    deadline: Instant,
    archived: bool,
) -> Result<(), Error> {
    let expected = Connection::open_in_memory()?;
    expected.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
    expected.execute_batch(include_str!("../../../domain.sql"))?;
    for &(next, sql) in crate::domain::DOMAIN_MIGRATIONS {
        if next <= version {
            expected.execute_batch(sql)?;
        }
    }
    if archived {
        expected.execute_batch(super::archives::DOMAIN_ARCHIVE_SCHEMA)?;
    }
    if objects(domain)? != objects(&expected)? {
        return Err(Error::Schema);
    }
    if Instant::now() >= deadline {
        return Err(Error::Busy);
    }
    Ok(())
}

pub(super) fn validate_personal(
    personal: &Connection,
    deadline: Instant,
    archived: bool,
) -> Result<(), Error> {
    // This is the exact additive shape produced by the current personal
    // opener, without calling it or performing its owner/recovery behavior.
    // Changes to that source must explicitly requalify this adoption slice.
    let expected = Connection::open_in_memory()?;
    expected.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
    expected.execute_batch(include_str!(
        "../../../../../hagency-agent-local/src/schema.sql"
    ))?;
    expected.execute_batch("ALTER TABLE model_profiles ADD COLUMN web_search INTEGER NOT NULL DEFAULT 1 CHECK(web_search IN (0,1)); CREATE TABLE IF NOT EXISTS external_sessions (scope TEXT NOT NULL, profile TEXT NOT NULL, session TEXT NOT NULL, counters TEXT NOT NULL DEFAULT '', PRIMARY KEY(scope,profile));")?;
    if archived {
        expected.execute_batch(super::archives::PERSONAL_ARCHIVE_SCHEMA)?;
    }
    if objects(personal)? != objects(&expected)? {
        return Err(Error::Schema);
    }
    if Instant::now() >= deadline {
        return Err(Error::Busy);
    }
    Ok(())
}
