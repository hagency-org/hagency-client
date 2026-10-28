use super::*;
use crate::shutdown::{Phase, Probe};
use hagency_platform::legacy_writer::{
    ExclusiveWriterGuard, LegacyWriterAdmission, LegacyWriterGuard,
};
#[tokio::test]
async fn observed_shutdown_retains_legacy_guard_through_sqlite_and_physical_lock_drop() {
    for phase in [
        Phase::ConnectionDropStarted,
        Phase::ConnectionDropFinished,
        Phase::OwnershipDropFinished,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let authority = temp.path().join("authority");
        let mut repository = DomainRepository::open(&temp.path().join("source")).unwrap();
        repository._legacy_writer = Some(LegacyWriterAdmission::Covered(
            LegacyWriterGuard::isolated_for_tests(&authority).unwrap(),
        ));
        let (probe, reached, resume) = Probe::paused(phase);
        let work = tokio::task::spawn_blocking(move || repository.drop_observed(&probe));
        tokio::time::timeout(std::time::Duration::from_secs(3), reached)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            ExclusiveWriterGuard::isolated_for_tests(&authority),
            Err(hagency_platform::legacy_writer::Error::Locked)
        ));
        resume.send(()).unwrap();
        work.await.unwrap();
        ExclusiveWriterGuard::isolated_for_tests(&authority).unwrap();
    }
}
