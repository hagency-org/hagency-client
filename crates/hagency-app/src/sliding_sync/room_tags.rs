//! Use standard room account data for tags. The embedded homeserver does not
//! expose the separate per-tag endpoint. Read fresh data and preserve other tags.
use anyhow::{Result,anyhow,ensure};
use matrix_sdk::{Client,ruma::{OwnedRoomId,api::client::config::{get_room_account_data,set_room_account_data},events::{RoomAccountDataEventType,tag::Tags},serde::Raw}};
use serde_json::{Value,json};

pub(super) async fn set_favorite(client:&Client,room:&OwnedRoomId,favorite:bool)->Result<Tags> {
    update(client,room,favorite,||crate::matrix_context::is_current(client)).await
}

async fn update(client:&Client,room:&OwnedRoomId,favorite:bool,current:impl Fn()->bool)->Result<Tags> {
    ensure!(current(),"The account changed.");
    let user=client.user_id().ok_or_else(||anyhow!("Sign in before changing favorites."))?.to_owned();
    let mut content=match client.send(get_room_account_data::v3::Request::new(user.clone(),room.clone(),RoomAccountDataEventType::Tag)).await {
        Ok(reply)=>reply.account_data.deserialize_as_unchecked::<Value>()?,
        Err(error) if error.as_client_api_error().is_some_and(|e|e.error_kind()==Some(&matrix_sdk::ruma::api::error::ErrorKind::NotFound))=>json!({"tags":{}}),
        Err(error)=>return Err(error.into()),
    };
    let tags=content.get_mut("tags").and_then(Value::as_object_mut).ok_or_else(||anyhow!("Invalid room tags response."))?;
    if favorite {tags.insert("m.favourite".into(),json!({}));tags.remove("m.lowpriority");}else{tags.remove("m.favourite");}
    let result=serde_json::from_value::<Tags>(Value::Object(tags.clone()))?;
    ensure!(content.is_object(),"Invalid room tags response.");
    ensure!(current(),"The account changed.");
    client.send(set_room_account_data::v3::Request::new_raw(user,room.clone(),RoomAccountDataEventType::Tag,Raw::new(&content)?.cast_unchecked())).await?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use matrix_sdk::test_utils::mocks::MatrixMockServer;
    use wiremock::{Mock,ResponseTemplate,matchers::{method,path_regex,body_json}};
    #[tokio::test]
    async fn favorites_use_standard_account_data_and_preserve_unrelated_tags() {
        let server=MatrixMockServer::new().await;let client=server.client_builder().build().await;
        let room=matrix_sdk::ruma::room_id!("!tools:test").to_owned();
        let endpoint=r"/_matrix/client/(v3|r0)/user/.*/rooms/.*/account_data/m.tag";
        Mock::given(method("GET")).and(path_regex(endpoint)).respond_with(ResponseTemplate::new(200).set_body_json(json!({"tags":{"u.custom":{"order":0.4},"m.lowpriority":{}},"extension":"retained"}))).expect(1).mount(server.server()).await;
        Mock::given(method("PUT")).and(path_regex(endpoint)).and(body_json(json!({"tags":{"u.custom":{"order":0.4},"m.favourite":{}},"extension":"retained"}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).expect(1).mount(server.server()).await;
        let tags=update(&client,&room,true,||true).await.unwrap();assert!(tags.contains_key(&matrix_sdk::ruma::events::tag::TagName::Favorite));
    }
    #[tokio::test]
    async fn failed_tag_read_never_overwrites_stored_tags() {
        let server=MatrixMockServer::new().await;let client=server.client_builder().build().await;
        Mock::given(method("GET")).and(path_regex(r".*/account_data/m.tag")).respond_with(ResponseTemplate::new(403).set_body_json(json!({"errcode":"M_FORBIDDEN","error":"Denied"}))).expect(1).mount(server.server()).await;
        Mock::given(method("PUT")).respond_with(ResponseTemplate::new(200)).expect(0).mount(server.server()).await;
        assert!(update(&client,&matrix_sdk::ruma::room_id!("!tools:test").to_owned(),false,||true).await.is_err());
    }
}
