//! Bounded, account-private reads of sealed task-graph history.
use super::*;

impl OwnerResourceRegistry {
    pub async fn operator_graph_archives(
        &self,
        profile: &ProfileAuthority,
    ) -> Result<Vec<OperatorGraphArchiveReceipt>, Error> {
        profile.check()?;
        let p = profile.clone();
        let registry = self.clone();
        let result = self.0.store.call(64 * 1024, move |db| {
            p.check()?;
            if registry.0.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority); }
            registry_identity(&db.db, &registry.0.id)?;
            let ids = db.db.prepare("SELECT command_id FROM owner_operator_graph_archives WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 ORDER BY command_id LIMIT 17")?
                .query_map(params![registry.0.id,p.digest,p.account], |r| r.get::<_,String>(0))?
                .collect::<Result<Vec<_>,_>>()?;
            if ids.len() > 16 { return Err(Error::Capacity); }
            let mut out = Vec::new();
            for id in ids {
                let saved = read(&db.db,&id)?.ok_or(Error::NotFound)?;
                visible(&saved,&p,&registry.0.id)?;
                out.push(receipt(id,saved.manifest,saved.state));
            }
            p.check()?;
            Ok(out)
        }).await?;
        profile.check()?;
        Ok(result)
    }

    /// Native projection input only. Never reads messages or their private
    /// tokens. The graph document is bounded to four MiB and read in one job.
    /// Partial archives, missing chunks and cross-account reads fail closed.
    pub async fn operator_graph_history_document(
        &self,
        profile: &ProfileAuthority,
        command_id: String,
    ) -> Result<Option<Vec<u8>>, Error> {
        profile.check()?;
        identifier(&command_id, 128)?;
        let p = profile.clone();
        let registry = self.clone();
        let result = self.0.store.call(4 * 1024 * 1024 + 4096, move |db| {
            p.check()?;
            if registry.0.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority); }
            registry_identity(&db.db,&registry.0.id)?;
            let saved = read(&db.db,&command_id)?.ok_or(Error::NotFound)?;
            visible(&saved,&p,&registry.0.id)?;
            if saved.state != "archived" { return Err(Error::Unqualified); }
            let Some(length) = saved.manifest.lengths[0] else { return Ok(None); };
            let mut bytes = Vec::with_capacity(length as usize);
            for index in 0..(length as usize).div_ceil(GRAPH_ARCHIVE_CHUNK_BYTES) {
                p.check()?;
                let chunk: Vec<u8> = db.db.query_row("SELECT CASE WHEN length(bytes)<=1048576 THEN bytes END FROM owner_operator_graph_archive_chunks WHERE command_id=?1 AND document=0 AND chunk_index=?2",params![command_id,index],|r|r.get(0))?;
                if chunk.len()!=saved.manifest.chunk_length(0,index)? { return Err(Error::Quarantined); }
                bytes.extend(chunk);
            }
            p.check()?;
            Ok(Some(bytes))
        }).await?;
        profile.check()?;
        Ok(result)
    }
}
