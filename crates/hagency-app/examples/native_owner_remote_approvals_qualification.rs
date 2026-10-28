//! Explicit isolated Synapse/two-SDK/public NativeOwner/paid Codex qualification.
//! The OAuth introspection bridge is a double; no production login claim.
#[path = "native_owner_remote_approvals_qualification/source.rs"]
mod source;
#[path = "native_owner_remote_approvals_qualification/run.rs"]
mod qualification;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(result) = hagency::native_owner::run_guardian_if_requested() {
        result?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(qualification::run())
}
