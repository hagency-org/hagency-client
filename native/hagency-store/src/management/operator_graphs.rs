//! Exact current native graph authority. Script arguments never select the
//! backing registry, account, scope or permission.
use super::*;
use crate::owner_resources::{OwnerResourceRegistry, ProfileAuthority};

pub const OPERATOR_GRAPH_SCOPE: &str = "operator_graphs_v1";

impl Session {
    /// Native adapters retain both handles across their operation. The profile
    /// gate checks this exact session/context/permission and adopted backing on
    /// every writer-side revalidation, including after the SQLite lock.
    pub fn operator_graph_access(
        &self,
        context: &HostContext,
        permission: Permission,
    ) -> Result<(OwnerResourceRegistry, ProfileAuthority), Error> {
        if !matches!(
            permission,
            Permission::ReadOperatorGraphs
                | Permission::CreateOperatorGraphs
                | Permission::UpdateOperatorGraphNodes
                | Permission::CancelOperatorGraphs
                | Permission::ReadOperatorGraphRecipients
                | Permission::BindOperatorGraphRecipients
        ) {
            return Err(Error::LocalAuthority);
        }
        self.check(context, permission)?;
        let SessionStore::Adopted(store) = &self.store else {
            return Err(Error::Unqualified);
        };
        let profile = self.adopted_profile(store)?;
        let registry = store.enrollment_registry(&profile)?;
        let backing = store.clone();
        let native_profile = profile.clone();
        let authority = self.authority();
        let current = context.clone();
        let gated = profile.with_native_gate(move || {
            authority.authorize(&current, permission).is_ok()
                && backing.enrollment_registry(&native_profile).is_ok()
        })?;
        self.check(context, permission)?;
        gated.check_current()?;
        Ok((registry, gated))
    }
}
