//! Native-only, retained-handle document observation. No create/write/open of a
//! legacy GraphStore, and no assumption that absent SQL history covers JSON.
use super::*;
use crate::operator_graphs::history::{DOCUMENTS, OperatorGraphHistorySummary, RetainedGraphHistory};

pub(in crate::console::native::source_quiescence) struct GraphDocuments {
    directory: Directory,
    handles: [Option<File>; 3],
    history: RetainedGraphHistory,
}
impl GraphDocuments {
    pub(in crate::console::native::source_quiescence) fn capture(
        root: &Path,
    ) -> Result<Self, &'static str> {
        let directory = Directory::open(root)?;
        let mut handles = [None, None, None];
        let mut documents = [None, None, None];
        for (index, (name, limit)) in DOCUMENTS.iter().enumerate() {
            if let Some(mut file) = optional(&directory, name)? {
                documents[index] = Some(read(&mut file, *limit)?);
                handles[index] = Some(file);
            }
        }
        let history = RetainedGraphHistory::inspect(documents).map_err(|e| e.code)?;
        let result = Self {
            directory,
            handles,
            history,
        };
        result.check()?;
        Ok(result)
    }
    pub(in crate::console::native::source_quiescence) fn check(&self) -> Result<(), &'static str> {
        self.check_inner(true)
    }
    pub(in crate::console::native::source_quiescence) fn check_identity(&self) -> Result<(), &'static str> {
        self.check_inner(false)
    }
    fn check_inner(&self, contents: bool) -> Result<(), &'static str> {
        self.directory.check()?;
        for (index, (name, limit)) in DOCUMENTS.iter().enumerate() {
            match (&self.handles[index], optional(&self.directory, name)?) {
                (None, None) => {}
                (Some(original), Some(mut current)) => {
                    if !hagency_platform::same_file(original, &current)
                        .map_err(|_| "graph_history_source_changed")?
                        || (contents && Some(read(&mut current, *limit)?.as_slice()) != self.history.bytes(index))
                    {
                        return Err("graph_history_source_changed");
                    }
                }
                _ => return Err("graph_history_source_changed"),
            }
        }
        self.directory.check()
    }
    pub(in crate::console::native::source_quiescence) fn lengths(&self) -> [Option<u64>; 3] {
        std::array::from_fn(|index| self.history.bytes(index).map(|bytes| bytes.len() as u64))
    }
    pub(in crate::console::native::source_quiescence) fn chunk(
        &self,
        document: usize,
        index: usize,
    ) -> Vec<u8> {
        use hagency_store::owner_resources::graph_archives::GRAPH_ARCHIVE_CHUNK_BYTES;
        let bytes = self
            .history
            .bytes(document)
            .expect("validated present document");
        let offset = index * GRAPH_ARCHIVE_CHUNK_BYTES;
        bytes[offset..bytes.len().min(offset + GRAPH_ARCHIVE_CHUNK_BYTES)].to_vec()
    }
    pub(in crate::console::native::source_quiescence) fn summary(
        &self,
    ) -> OperatorGraphHistorySummary {
        self.history.summary()
    }
}
fn optional(directory: &Directory, name: &str) -> Result<Option<File>, &'static str> {
    match directory.handle.symlink_metadata(name) {
        Ok(_) => directory.file(name, false).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("graph_history_source_unavailable"),
    }
}
fn read(file: &mut File, limit: usize) -> Result<Vec<u8>, &'static str> {
    if file
        .metadata()
        .map_err(|_| "graph_history_source_unavailable")?
        .len()
        > limit as u64
    {
        return Err("graph_history_review_required");
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "graph_history_source_unavailable")?;
    if bytes.len() > limit {
        return Err("graph_history_review_required");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
