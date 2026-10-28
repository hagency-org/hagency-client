impl NativeManagement {
    pub async fn operator_graph_recipient_options(
        &self,
        query: OperatorGraphRecipientOptionsQuery,
    ) -> Result<Value, NativeError> {
        query.validate().map_err(|_| argument_error())?;
        let context = self.recipient_context().await?;
        let node = self
            .operator_graph_node(OperatorGraphNodeQuery {
                id: query.id.clone(),
                node_id: query.node_id.clone(),
            })
            .await?;
        let mut options = Vec::new();
        let mut remote_next = None;
        let stage;
        match &query.kind {
            RecipientKind::SharedAgent => {
                if let Some(project) = &query.project_id {
                    stage = "agent_requests";
                    let reply = Box::pin(self.central(ManagementCommand::AgentRequests {
                        project_id: Some(project.clone()),
                        after: query.after.clone(),
                        needs_my_action: false,
                    }))
                    .await?;
                    remote_next = cursor(&reply)?;
                    for item in rows(&reply, "items")? {
                        let id = text(&item, "id")?;
                        if item["projectId"] != *project {
                            return Err(invalid());
                        }
                        let label = item["definition"]["name"]
                            .as_str()
                            .unwrap_or(&id)
                            .to_owned();
                        let target = OperatorGraphRecipientTarget::SharedAgent {
                            project_id: project.clone(),
                            agent_request_id: id.clone(),
                        };
                        target.validate().map_err(|_| invalid())?;
                        options.push(json!({"id":id,"label":bounded_label(&label),"detail":id,"status":item["state"],"target":target}));
                    }
                } else {
                    stage = "projects";
                    let reply = Box::pin(self.central(ManagementCommand::Projects {
                        after: query.after.clone(),
                        needs_my_action: false,
                    }))
                    .await?;
                    remote_next = cursor(&reply)?;
                    for item in rows(&reply, "items")? {
                        let id = text(&item, "id")?;
                        recipients::key(&id).map_err(|_| invalid())?;
                        let mut next = query.clone();
                        next.project_id = Some(id.clone());
                        next.after = None;
                        options.push(json!({"id":id,"label":bounded_label(item["name"].as_str().unwrap_or(&id)),"detail":format!("{} · {}",item["managerMxid"].as_str().unwrap_or(""),id),"next":next}));
                    }
                }
            }
            RecipientKind::PersonalAgent => {
                if let Some(agent) = &query.agent_id {
                    stage = "bindings";
                    let reply = self
                        .recipient_owner_read(OwnerOperation::Bindings {
                            agent: agent.clone(),
                        })
                        .await?;
                    for item in rows(&reply, "bindings")? {
                        let id = text(&item, "id")?;
                        if item["agentId"] != *agent {
                            return Err(invalid());
                        }
                        let target = OperatorGraphRecipientTarget::PersonalAgent {
                            agent_id: agent.clone(),
                            binding_id: id.clone(),
                        };
                        target.validate().map_err(|_| invalid())?;
                        options.push(json!({"id":id,"label":bounded_label(item["roomId"].as_str().unwrap_or(&id)),"detail":id,"scopeKind":item["scopeKind"],"target":target}));
                    }
                } else {
                    stage = "agents";
                    for item in rows(
                        &self.recipient_owner_read(OwnerOperation::Agents).await?,
                        "agents",
                    )? {
                        let id = text(&item, "id")?;
                        recipients::key(&id).map_err(|_| invalid())?;
                        let mut next = query.clone();
                        next.agent_id = Some(id.clone());
                        next.after = None;
                        options.push(json!({"id":id,"label":bounded_label(item["displayName"].as_str().unwrap_or(&id)),"detail":id,"next":next}));
                    }
                }
            }
            RecipientKind::Human => {
                if let Some(room) = &query.room_id {
                    stage = "members";
                    let project = query.project_id.as_ref().ok_or_else(argument_error)?;
                    // Verify the room belongs to the selected visible project before
                    // using current Matrix membership as the human picker.
                    let registered = select(
                        &self
                            .recipient_owner_read(OwnerOperation::ProjectRooms {
                                project: project.clone(),
                            })
                            .await?,
                        "rooms",
                        "roomId",
                        room,
                    )?;
                    if registered["projectId"] != *project || registered["active"] != true {
                        return Err(failure(409, "management_recipient_unavailable"));
                    }
                    let state = self.recipient_room_state(room).await?;
                    let events = state
                        .as_array()
                        .filter(|v| v.len() <= 16384)
                        .ok_or_else(invalid)?;
                    for event in events.iter().filter(|e| {
                        e["type"] == "m.room.member" && e["content"]["membership"] == "join"
                    }) {
                        let id = text(event, "state_key")?;
                        let target = OperatorGraphRecipientTarget::Human {
                            project_id: project.clone(),
                            room_id: room.clone(),
                            user_id: id.clone(),
                        };
                        target.validate().map_err(|_| invalid())?;
                        options.push(json!({"id":id,"label":bounded_label(event["content"]["displayname"].as_str().unwrap_or(&id)),"detail":id,"target":target}));
                    }
                } else if let Some(project) = &query.project_id {
                    stage = "rooms";
                    for item in rows(
                        &self
                            .recipient_owner_read(OwnerOperation::ProjectRooms {
                                project: project.clone(),
                            })
                            .await?,
                        "rooms",
                    )? {
                        let id = text(&item, "roomId")?;
                        recipients::matrix_id(&id, '!').map_err(|_| invalid())?;
                        if item["projectId"] != *project {
                            return Err(invalid());
                        }
                        let mut next = query.clone();
                        next.room_id = Some(id.clone());
                        next.after = None;
                        options.push(json!({"id":id,"label":bounded_label(item["name"].as_str().unwrap_or(&id)),"detail":id,"next":next}));
                    }
                } else {
                    stage = "projects";
                    for item in rows(
                        &self.recipient_owner_read(OwnerOperation::Projects).await?,
                        "projects",
                    )? {
                        let id = text(&item, "id")?;
                        recipients::key(&id).map_err(|_| invalid())?;
                        let mut next = query.clone();
                        next.project_id = Some(id.clone());
                        next.after = None;
                        options.push(json!({"id":id,"label":bounded_label(item["name"].as_str().or_else(||item["spaceId"].as_str()).unwrap_or(&id)),"detail":id,"next":next}));
                    }
                }
            }
        }
        let (items, next_after) = option_page(options, query.after.as_deref(), remote_next)?;
        self.recipient_finish(&context,json!({"graphId":query.id,"nodeId":query.node_id,"revision":node.revision,"kind":query.kind,"stage":stage,"items":items,"nextAfter":next_after,"runtimeDeliveryAuthorized":false})).await
    }
}
fn bounded_label(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect()
}
fn cursor(reply: &Value) -> Result<Option<String>, NativeError> {
    match &reply["nextAfter"] {
        Value::Null => Ok(None),
        Value::String(s) => {
            recipients::key(s).map_err(|_| invalid())?;
            Ok(Some(s.clone()))
        }
        _ => Err(invalid()),
    }
}
fn option_page(
    mut rows: Vec<Value>,
    after: Option<&str>,
    remote_next: Option<String>,
) -> Result<(Vec<Value>, Option<String>), NativeError> {
    let mut seen = BTreeSet::new();
    for item in &rows {
        let id = text(item, "id")?;
        if !seen.insert(id) {
            return Err(invalid());
        }
    }
    rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    rows.retain(|r| after.is_none_or(|after| r["id"].as_str().is_some_and(|id| id > after)));
    let more = rows.len() > 16;
    rows.truncate(16);
    let next = if more {
        rows.last()
            .and_then(|r| r["id"].as_str())
            .map(str::to_owned)
    } else {
        remote_next
    };
    if next
        .as_deref()
        .is_some_and(|next| after.is_some_and(|after| next <= after))
    {
        return Err(invalid());
    }
    Ok((rows, next))
}
