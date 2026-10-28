//! Read-only web-console projections over the existing authenticated services.
//! Missing observations remain null; these screens introduce no meter or engine.
use super::{NativeError, NativeManagement, failure};
use hagency_store::management::{self as service, Permission};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod accounts;
#[cfg(test)]
mod tests;
mod usage;
mod workforce;

pub use hagency_store::management::{WebAlertsPage, WebAlertsQuery};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebOverviewQuery {
    pub after: Option<String>,
    pub limit: Option<u16>,
}
impl WebOverviewQuery {
    pub fn validate(&self) -> Result<(), NativeError> {
        if let Some(id) = &self.after {
            super::key(id)?;
        }
        if self.limit.is_some_and(|n| !(1..=16).contains(&n)) {
            return Err(failure(400, "invalid_management_arguments"));
        }
        Ok(())
    }
    fn resource_query(&self) -> service::ResourceQuery {
        service::ResourceQuery {
            after: self.after.clone().unwrap_or_default(),
            limit: usize::from(self.limit.unwrap_or(16)),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebAccountCapacityQuery {
    pub after: Option<String>,
    pub id: Option<String>,
    pub resource_after: Option<String>,
}
impl WebAccountCapacityQuery {
    pub fn validate(&self) -> Result<(), NativeError> {
        for id in [&self.after, &self.id, &self.resource_after]
            .into_iter()
            .flatten()
        {
            super::key(id)?;
        }
        if self.id.is_some() && self.after.is_some()
            || self.resource_after.is_some() && self.id.is_none()
        {
            return Err(failure(400, "invalid_management_arguments"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebTerminalQuery {
    pub id: String,
}
impl WebTerminalQuery {
    pub fn validate(&self) -> Result<(), NativeError> {
        super::key(&self.id)
    }
}

fn bounded<T: Serialize>(value: T) -> Result<T, NativeError> {
    if serde_json::to_vec(&value).map_err(|_| invalid())?.len() > 128 * 1024 {
        return Err(failure(503, "management_capacity"));
    }
    Ok(value)
}
fn invalid() -> NativeError {
    failure(503, "management_web_overview_invalid")
}
fn label(resource: &service::ResourceSummary) -> String {
    resource
        .local_name
        .as_ref()
        .map(|n| n.name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| resource.id.clone())
}
fn authorize(
    session: &service::Session,
    context: &service::HostContext,
    permissions: &[Permission],
) -> Result<(), NativeError> {
    for permission in permissions {
        session
            .authorize(context, *permission)
            .map_err(super::management::store_error)?;
    }
    Ok(())
}
fn qualify_resource_roles(offers: &mut service::WebOfferPage) {
    // Offers are owner-wide, but each capability card describes one model.
    // Never borrow another resource's qualification or its publication state.
    for role in &mut offers.items {
        role.qualified = offers.resource_qualifications.contains(&role.role);
    }
}
impl NativeManagement {
    async fn web_finish(
        &self,
        original: &service::HostContext,
        permissions: &[Permission],
    ) -> Result<(), NativeError> {
        let (current, session) = self.authorized().await?;
        if current != *original {
            return Err(failure(401, "management_scope_changed"));
        }
        authorize(&session, &current, permissions)
    }

    pub async fn web_alerts(&self, query: WebAlertsQuery) -> Result<WebAlertsPage, NativeError> {
        query.validate().map_err(super::management::store_error)?;
        let (context, session) = self.authorized().await?;
        let result = session.web_alerts(&context, query).await;
        self.finish(&context, result, false).await
    }

    pub async fn web_capability(&self, query: WebOverviewQuery) -> Result<Value, NativeError> {
        query.validate()?;
        let permissions = [Permission::ReadResources, Permission::ReadWebCatalog];
        let (context, session) = self.authorized().await?;
        authorize(&session, &context, &permissions)?;
        let page = session
            .resources(&context, query.resource_query())
            .await
            .map_err(super::management::store_error)?;
        let mut items = Vec::with_capacity(page.resources.len());
        for resource in page.resources {
            let detail = session
                .resource_detail(
                    &context,
                    service::ResourceDetailQuery {
                        id: resource.id.clone(),
                    },
                )
                .await
                .map_err(super::management::store_error)?;
            if detail.resource.revision != resource.revision {
                return Err(failure(409, "management_revision_conflict"));
            }
            let mut offers = session
                .web_offers(&context, resource.id.clone())
                .await
                .map_err(super::management::store_error)?;
            qualify_resource_roles(&mut offers);
            items.push(json!({"label":label(&resource),"resource":resource,"modelTier":detail.model_tier,"family":detail.model_family,"roles":offers.items,"offerRevision":offers.revision,"offersEditable":offers.offers_editable,"offerEditingReason":offers.offer_editing_reason,"runtimeReady":null}));
        }
        self.web_finish(&context, &permissions).await?;
        bounded(
            json!({"protocol":"hagency-web-overview-v1","observedAtMs":page.observed_at_ms,"scope":"page","items":items,"nextAfter":page.next_after,"qualificationSource":"embedded_policy","runtimeAuthorization":false}),
        )
    }

    pub async fn web_terminal(&self, query: WebTerminalQuery) -> Result<Value, NativeError> {
        query.validate()?;
        let permissions = [
            Permission::ReadAgentRequests,
            Permission::ReadResources,
            Permission::ReadAccounts,
        ];
        let (context, session) = self.authorized().await?;
        authorize(&session, &context, &permissions)?;
        // Current managed execution is headless. Resolve the actual visible
        // request first rather than accepting an arbitrary local pane/path.
        let local = session
            .web_workforce_links(&context, vec![query.id.clone()])
            .await
            .map_err(super::management::store_error)?;
        if local.is_empty() {
            let response = self
                .central(super::ManagementCommand::Agent {
                    id: query.id.clone(),
                })
                .await?;
            if response["agentRequest"]["id"] != query.id {
                return Err(invalid());
            }
        }
        self.web_finish(&context, &permissions).await?;
        Ok(
            json!({"id":query.id,"supported":false,"reason":"headless_runtime","text":null,"readOnly":true}),
        )
    }
}
