//! Provider discovery projects public model choices; native enrollment keeps
//! provider account references inside the host and never launches a model turn.
use super::{Command, EnrollResource, NativeError, NativeManagement, ProviderAction, failure};
use hagency_store::management::{
    HostContext, Permission, ProviderSelection, ProviderWitness, ResourceSummary, Session,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagementRuntime {
    Codex,
    Claude,
    Octos,
}
impl ManagementRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Octos => "octos",
        }
    }
    fn command(self, action: ProviderAction) -> Command {
        match self {
            Self::Codex => Command::Provider { action },
            Self::Claude | Self::Octos => Command::RuntimeProvider {
                runtime: self.as_str().into(),
                action,
            },
        }
    }
}
fn command(runtime: &str) -> Result<Command, NativeError> {
    match runtime {
        "codex" => Ok(Command::Provider {
            action: ProviderAction::Models,
        }),
        "claude" | "octos" => Ok(Command::RuntimeProvider {
            runtime: runtime.into(),
            action: ProviderAction::Models,
        }),
        _ => Err(failure(400, "invalid_provider_runtime")),
    }
}
pub(super) fn projection(runtime: &str, status: &Value) -> Value {
    let models = status["models"].as_array().map(|rows| rows.iter().take(100).map(|row| {
        let reasoning = row["supportedReasoningEfforts"].as_array().map(|efforts| efforts.iter()
            .take(16).filter_map(|v| v["reasoningEffort"].as_str().map(str::to_owned))
            .collect::<Vec<_>>()).unwrap_or_default();
        let candidates: Vec<Option<String>> = if reasoning.is_empty() { vec![None] }
            else { reasoning.iter().cloned().map(Some).collect() };
        let shared_reasoning: Vec<_> = candidates.into_iter().filter(|effort| {
            let profile = hagency_core::qualification::ModelProfile {
                framework: runtime.into(), model: row["model"].as_str().unwrap_or("").into(),
                provider: None, reasoning: effort.clone(),
            };
            hagency_core::qualification::roles().any(|role| hagency_core::qualification::qualifies(&profile, role, None))
        }).collect();
        let default = row["defaultReasoningEffort"].as_str().map(str::to_owned);
        let shared_default = if shared_reasoning.contains(&default) { default }
            else { shared_reasoning.first().cloned().flatten() };
        json!({
            "id":row["id"],"model":row["model"],"label":row["displayName"],
            "default":row["isDefault"],"defaultReasoning":row["defaultReasoningEffort"],
            "reasoning":reasoning,"sharedReasoning":shared_reasoning,"sharedDefaultReasoning":shared_default,
        })
    }).collect::<Vec<_>>()).unwrap_or_default();
    json!({"runtime":runtime,"state":status["state"],"connected":status["authenticated"]==true,
        "accountVerified":status["authenticated"]==true && (runtime=="codex" || status["accountVerified"]==true),
        "strictTokenCap":status["strictTokenCap"]==true,"models":models})
}
fn connection_result(
    mutation_attempted: bool,
    result: Result<Value, NativeError>,
) -> Result<Value, NativeError> {
    result.map_err(|error| {
        // A timeout or late authorization failure may follow a persisted native
        // provider choice. Refresh its state instead of asserting no change.
        if mutation_attempted && (matches!(error.status, 401 | 403) || error.status >= 500) {
            failure(409, "management_outcome_unknown")
        } else {
            error
        }
    })
}
impl NativeManagement {
    pub(super) async fn verify_local_resource(
        &self,
        context: &HostContext,
        session: &Session,
        resource: &ResourceSummary,
        resume: bool,
    ) -> Result<ResourceSummary, NativeError> {
        #[cfg(feature = "resource-adoption")]
        if session
            .uses_canonical_resources(context)
            .map_err(|_| failure(403, "management_permission_required"))?
        {
            // The adopted profile intentionally cannot execute legacy owner
            // commands. Observe only the selected provider's quota identity;
            // this does not qualify a model or make an agent runnable.
            let _guard = self.owner.0.calls.lock().await;
            let permission = if resume {
                Permission::ResumeConnections
            } else {
                Permission::VerifyConnections
            };
            session
                .authorize(context, permission)
                .map_err(|_| failure(403, "management_permission_required"))?;
            if resource.framework != "codex" {
                return Err(failure(409, "provider_identity_unavailable"));
            }
            let namespace=session.connection_namespace(context,resource,resume).await.map_err(super::management::store_error)?;
            let proof=if let Some(namespace)=namespace {
                let deadline=session.valid_until().min(Instant::now()+Duration::from_secs(30));
                self.namespace_provider(&namespace,deadline).await?
            } else {
                // Only historical resources without a namespace binding use the
                // original profile adapter. Missing recorded bindings fail above.
                let cookie=self.owner.0.cookie.lock().await.clone();
                let raw=cookie.strip_prefix("hagency_console=").ok_or_else(||failure(401,"owner_authorization_required"))?;
                let (proof,deadline)=crate::console::owner_provider::identity::verify(&self.owner.0.host.console,raw).await.map_err(|e|failure(e.status,e.code))?;
                if Instant::now()>=deadline {return Err(failure(409,"provider_identity_unavailable"));}
                proof
            };
            return self
                .verify_canonical_connection(context, resource, resume, proof)
                .await;
        }
        let status = self.owner.execute(command(&resource.framework)?).await?;
        if projection(&resource.framework, &status)["accountVerified"] != true {
            return Err(failure(409, "provider_account_verification_required"));
        }
        let row = status["models"]
            .as_array()
            .and_then(|rows| {
                rows.iter().find(|row| {
                    row["model"].as_str() == Some(resource.model.as_str())
                        && resource.reasoning.as_ref().is_none_or(|effort| {
                            row["supportedReasoningEfforts"]
                                .as_array()
                                .is_some_and(|rows| {
                                    rows.iter().any(|r| r["reasoningEffort"] == *effort)
                                })
                        })
                })
            })
            .ok_or_else(|| failure(409, "provider_model_changed"))?;
        let reference = row["credentialRef"]
            .as_str()
            .or_else(|| status["credentialRef"].as_str())
            .ok_or_else(|| failure(409, "provider_profile_unavailable"))?;
        let (fresh, live) = self.authorized().await?;
        if fresh != *context {
            return Err(failure(401, "management_scope_changed"));
        }
        live.authorize(
            context,
            if resume {
                Permission::ResumeConnections
            } else {
                Permission::VerifyConnections
            },
        )
        .map_err(|_| failure(403, "management_permission_required"))?;
        let proof = ProviderWitness::new(
            context.clone(),
            resource.framework.clone(),
            reference,
            resource.model.clone(),
            resource.reasoning.clone(),
            Instant::now() + Duration::from_secs(30),
        )
        .map_err(|_| failure(409, "provider_profile_unavailable"))?;
        let result = if resume {
            session
                .resume_resource(
                    context,
                    resource.id.clone(),
                    resource.revision.clone(),
                    proof,
                )
                .await
        } else {
            session
                .verify_resource(
                    context,
                    resource.id.clone(),
                    resource.revision.clone(),
                    proof,
                )
                .await
        };
        result.map_err(|_| failure(409, "management_executor_changed"))
    }
    #[cfg(feature = "resource-adoption")]
    pub(super) async fn verify_canonical_connection(
        &self,
        context: &HostContext,
        resource: &ResourceSummary,
        resume: bool,
        proof: hagency_store::owner_resources::VerifiedProviderAccount,
    ) -> Result<ResourceSummary, NativeError> {
        let (fresh, live) = self.authorized().await?;
        if fresh != *context {
            return Err(failure(401, "management_scope_changed"));
        }
        let permission = if resume {
            Permission::ResumeConnections
        } else {
            Permission::VerifyConnections
        };
        live.authorize(context, permission)
            .map_err(|_| failure(403, "management_permission_required"))?;
        // The provider observation may outlive the initially captured package
        // session. Use the freshly authorized same-context session; never
        // revive the expired proof or extend the provider witness deadline.
        let result = live
            .verify_canonical_connection(
                context,
                resource.id.clone(),
                resource.revision.clone(),
                proof,
                resume,
            )
            .await
            .map_err(|_| failure(409, "management_executor_changed"))?;
        let (fresh, live) = self.authorized().await?;
        if fresh != *context {
            return Err(failure(401, "management_scope_changed"));
        }
        live.authorize(context, permission)
            .map_err(|_| failure(403, "management_permission_required"))?;
        Ok(result)
    }
    /// Trusted native interaction only. The host must show its own consent
    /// screen; there is deliberately no deserializable approval flag or token.
    /// This associates an existing local provider account, never starts an agent.
    pub async fn connect_provider(
        &self,
        runtime: ManagementRuntime,
        valid_until: Instant,
    ) -> Result<Value, NativeError> {
        // Recheck the package after waiting for the owner's serialized command
        // slot, so a closed prompt cannot enqueue a later provider change.
        let _guard = self.owner.0.calls.lock().await;
        if Instant::now() >= valid_until {
            return Err(failure(403, "provider_confirmation_expired"));
        }
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::ConnectProviders)
            .map_err(|_| failure(403, "management_permission_required"))?;
        let current = self
            .owner
            .execute_locked(runtime.command(ProviderAction::Status))
            .await?;
        let (fresh, live) = self.authorized().await?;
        if fresh != context {
            return Err(failure(401, "management_scope_changed"));
        }
        live.authorize(&context, Permission::ConnectProviders)
            .map_err(|_| failure(403, "management_permission_required"))?;
        if Instant::now() >= valid_until {
            return Err(failure(403, "provider_confirmation_expired"));
        }
        // Repeated acceptance is harmless and does not stop existing runs.
        let mutation_attempted = current["authenticated"] != true;
        let result = if !mutation_attempted {
            Ok(current)
        } else {
            self.owner
                .execute_locked(runtime.command(ProviderAction::UseLocal))
                .await
        };
        let (fresh, live) = self.authorized().await.map_err(|error| {
            if mutation_attempted {
                failure(409, "management_outcome_unknown")
            } else {
                error
            }
        })?;
        if fresh != context
            || live
                .authorize(&context, Permission::ConnectProviders)
                .is_err()
        {
            return Err(failure(409, "management_outcome_unknown"));
        }
        connection_result(mutation_attempted, result)
            .map(|value| projection(runtime.as_str(), &value))
    }
    pub async fn providers(&self) -> Result<Value, NativeError> {
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::ReadProviders)
            .map_err(|_| failure(403, "management_permission_required"))?;
        let mut providers = Vec::new();
        for runtime in ["codex", "claude", "octos"] {
            let result = self.owner.execute(command(runtime)?).await;
            let (current, live) = self.authorized().await?;
            if current != context {
                return Err(failure(401, "management_scope_changed"));
            }
            live.authorize(&context, Permission::ReadProviders)
                .map_err(|_| failure(403, "management_permission_required"))?;
            providers.push(match result {
                Ok(value)=>projection(runtime,&value),
                Err(error)=>json!({"runtime":runtime,"state":"unavailable","connected":false,"models":[],"reason":error.code}),
            });
        }
        Ok(json!({"providers":providers}))
    }
    pub async fn enroll_resource(
        &self,
        input: EnrollResource,
    ) -> Result<hagency_store::ResourceConfigurationResult, NativeError> {
        self.owner.require_regular_mode()?;
        input
            .validate()
            .map_err(|_| failure(400, "invalid_resource_selection"))?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::EnrollResources)
            .map_err(|_| failure(403, "management_permission_required"))?;
        let intent = hagency_store::management::Intent::new(
            hagency_store::management::IntentOperation::EnrollResource,
            input.command_id.clone(),
            &input,
        )
        .map_err(super::management::store_error)?;
        self.with_intent(intent, self.enroll_resource_now(input))
            .await
    }
    async fn enroll_resource_now(
        &self,
        input: EnrollResource,
    ) -> Result<hagency_store::ResourceConfigurationResult, NativeError> {
        input
            .validate()
            .map_err(|_| failure(400, "invalid_resource_selection"))?;
        let (context, session) = self.authorized().await?;
        session
            .authorize(&context, Permission::EnrollResources)
            .map_err(|_| failure(403, "management_permission_required"))?;
        let status = self.owner.execute(command(&input.runtime)?).await?;
        if status["authenticated"] != true {
            return Err(failure(409, "provider_connection_required"));
        }
        let row = status["models"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|r| r["id"].as_str() == Some(input.model_id.as_str()))
            })
            .ok_or_else(|| failure(409, "provider_model_changed"))?;
        if let Some(effort) = &input.reasoning
            && !row["supportedReasoningEfforts"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|r| r["reasoningEffort"] == *effort))
        {
            return Err(failure(409, "provider_model_changed"));
        }
        let reference = row["credentialRef"]
            .as_str()
            .or_else(|| status["credentialRef"].as_str())
            .ok_or_else(|| failure(409, "provider_profile_unavailable"))?;
        let model = row["model"]
            .as_str()
            .ok_or_else(|| failure(409, "provider_model_changed"))?;
        let profile = if input.runtime == "octos" {
            let (kind, profile) = hagency_agent_local::runtime::managed_reference(reference)
                .ok_or_else(|| failure(409, "provider_profile_unavailable"))?;
            if kind != hagency_agent_local::runtime::Kind::Octos {
                return Err(failure(409, "provider_profile_unavailable"));
            }
            Some(
                profile
                    .ok_or_else(|| failure(409, "provider_profile_unavailable"))?
                    .to_owned(),
            )
        } else {
            None
        };
        let (current, live) = self.authorized().await?;
        if current != context {
            return Err(failure(401, "management_scope_changed"));
        }
        let proof = ProviderSelection::new(
            context.clone(),
            &input,
            reference,
            model.into(),
            profile,
            Instant::now() + Duration::from_secs(30),
        )
        .map_err(|_| failure(409, "provider_model_changed"))?;
        let lease = self
            .legacy_resource_lease(&context, &live, Permission::EnrollResources)
            .await?;
        let result = live
            .enroll_resource_with_legacy_lease(&context, input, proof, lease)
            .await;
        self.finish(&context, result, true).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_codex_models_use_only_live_qualified_efforts_without_switching_defaults() {
        for model in ["gpt-6.1-sol", "gpt-6-astra"] {
            let raw = json!({"authenticated":true,"models":[{
                "id":model,"model":model,"displayName":model,"isDefault":false,
                "defaultReasoningEffort":"medium",
                "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"},{"reasoningEffort":"high"},{"reasoningEffort":"xhigh"},{"reasoningEffort":"max"},{"reasoningEffort":"ultra"}]
            }]});
            let result = projection("codex", &raw);
            let row = &result["models"][0];
            assert_eq!(row["model"], model);
            assert_eq!(row["default"], false);
            assert_eq!(row["sharedReasoning"], json!(["low","medium","high","xhigh","max"]));
            assert_eq!(row["sharedDefaultReasoning"], "medium");
            // An older/restricted provider catalog must never gain settings just
            // because the role policy knows that model supports them elsewhere.
            let mut restricted = raw;
            restricted["models"][0]["supportedReasoningEfforts"] = json!([{"reasoningEffort":"high"}]);
            let result = projection("codex", &restricted);
            assert_eq!(result["models"][0]["sharedReasoning"], json!(["high"]));
            assert_eq!(result["models"][0]["sharedDefaultReasoning"], "high");
            restricted["models"][0]["supportedReasoningEfforts"] = json!([]);
            let result = projection("codex", &restricted);
            assert_eq!(result["models"][0]["sharedReasoning"], json!([]));
            assert_eq!(result["models"][0]["sharedDefaultReasoning"], Value::Null);
        }
    }
    #[test]
    fn shared_choices_intersect_provider_catalog_with_existing_resource_policy() {
        let status = json!({"authenticated":true,"models":[
            {"id":"sol","model":"gpt-5.6-sol","defaultReasoningEffort":"unsupported",
             "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"},{"reasoningEffort":"unsupported"}]},
            {"id":"future","model":"unqualified-future-model","supportedReasoningEfforts":[{"reasoningEffort":"high"}]}
        ]});
        let models = projection("codex", &status)["models"].clone();
        assert_eq!(
            models[0]["reasoning"],
            json!(["low", "high", "unsupported"])
        );
        assert_eq!(models[0]["sharedReasoning"], json!(["low", "high"]));
        assert_eq!(models[0]["sharedDefaultReasoning"], "low");
        assert_eq!(models[1]["sharedReasoning"], json!([]));
        // A provider model remains inspectable even when it cannot be enrolled.
        assert_eq!(models[1]["id"], "future");
        let claude = projection(
            "claude",
            &json!({"models":[{"id":"opus","model":"claude-opus-5"}]}),
        );
        assert_eq!(claude["models"][0]["sharedReasoning"], json!([null]));
    }
    #[test]
    fn failed_connection_reply_does_not_claim_the_provider_choice_was_rolled_back() {
        for status in [401, 403, 500, 503] {
            assert_eq!(
                connection_result(true, Err(failure(status, "late_failure")))
                    .unwrap_err()
                    .code,
                "management_outcome_unknown"
            );
            assert_eq!(
                connection_result(false, Err(failure(status, "late_failure")))
                    .unwrap_err()
                    .code,
                "late_failure"
            );
        }
        assert_eq!(
            connection_result(true, Err(failure(409, "local_provider_not_signed_in")))
                .unwrap_err()
                .code,
            "local_provider_not_signed_in"
        );
        assert_eq!(
            connection_result(true, Ok(json!({"authenticated":true}))).unwrap()["authenticated"],
            true
        );
    }
    #[test]
    fn provider_projection_strips_private_native_fields_and_reports_unverified_accounts() {
        let value = json!({"state":"ready","authenticated":true,"credentialRef":"must-not-escape","workspace":"/private/home","authUrl":"https://login/?secret=private","models":[{"id":"work","model":"kimi-k3","displayName":"Work","credentialRef":"private-ref","supportedReasoningEfforts":[{"reasoningEffort":"none","token":"private-token"}]}]});
        let projected = projection("octos", &value);
        assert_eq!(projected["connected"], true);
        assert_eq!(projected["accountVerified"], false);
        assert_eq!(projected["models"][0]["reasoning"], json!(["none"]));
        assert_eq!(
            projection(
                "claude",
                &json!({"authenticated":false,"accountVerified":true})
            )["accountVerified"],
            false
        );
        let text = projected.to_string();
        for value in [
            "must-not-escape",
            "/private",
            "secret=",
            "private-ref",
            "private-token",
            "credentialRef",
            "authUrl",
        ] {
            assert!(!text.contains(value));
        }
    }
}
