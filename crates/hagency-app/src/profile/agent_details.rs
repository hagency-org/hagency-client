//! Agent identity comes from the signed-in server's owner and public rosters.
//! A Matrix ID or display name alone never establishes Agent ownership.
use makepad_widgets::*;
use matrix_sdk::ruma::{OwnedUserId, UserId};
use serde_json::Value;

use crate::{
    hagency::{backend::Service, model::Command},
    sliding_sync::{get_client, spawn_async_task},
};

#[derive(Clone, Debug, PartialEq)]
struct AgentIdentity {
    owner: OwnedUserId,
    visibility: Option<bool>, // true: public; None: legacy server did not return it.
}

fn find_identity(
    value: &Value,
    target: &UserId,
    viewer: &UserId,
    owned: bool,
) -> Result<Option<AgentIdentity>, String> {
    if owned && value["ownerMxid"].as_str() != Some(viewer.as_str()) {
        return Err("matrix_account_changed".into());
    }
    let rows = value["agents"].as_array().ok_or("invalid_agent_roster")?;
    let mut matches = rows
        .iter()
        .filter(|row| row["puppetMxid"].as_str() == Some(target.as_str()));
    let Some(row) = matches.next() else {
        // The public catalog is capped. An absent row never proves a person is
        // not an Agent, especially when the server returned a full page.
        return if !owned && rows.len() >= 256 {
            Err("agent_roster_incomplete".into())
        } else {
            Ok(None)
        };
    };
    if matches.next().is_some() {
        return Err("ambiguous_agent_identity".into());
    }
    row_identity(row, target, viewer, owned).map(Some)
}

fn row_identity(
    row: &Value,
    target: &UserId,
    viewer: &UserId,
    owned: bool,
) -> Result<AgentIdentity, String> {
    let owner = if owned {
        if row
            .get("ownerMxid")
            .is_some_and(|id| id.as_str() != Some(viewer.as_str()))
        {
            return Err("invalid_agent_owner".into());
        }
        viewer.to_owned()
    } else {
        let owner = UserId::parse(row["ownerMxid"].as_str().ok_or("invalid_agent_owner")?)
            .map_err(|_| "invalid_agent_owner")?;
        if row["isMine"].as_bool() != Some(owner == viewer) {
            return Err("invalid_agent_owner".into());
        }
        owner
    };
    if owner == target || target.localpart() == "_hagency_service" {
        return Err("invalid_agent_owner".into());
    }
    let visibility = match row["visibility"].as_str() {
        Some("public") => Some(true),
        Some("private") if owned => Some(false),
        None if owned && row["visibility"].is_null() => None,
        _ => return Err("invalid_agent_visibility".into()),
    };
    Ok(AgentIdentity { owner, visibility })
}

fn roster_agent_ids(
    value: &Value,
    viewer: &UserId,
    owned: bool,
) -> Result<std::collections::HashSet<OwnedUserId>, String> {
    if owned && value["ownerMxid"].as_str() != Some(viewer.as_str()) {
        return Err("matrix_account_changed".into());
    }
    let rows = value["agents"].as_array().ok_or("invalid_agent_roster")?;
    let mut ids = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let Some(target) = row["puppetMxid"]
            .as_str()
            .and_then(|id| UserId::parse(id).ok())
        else {
            continue;
        };
        if !seen.insert(target.clone()) {
            ids.remove(&target);
            continue;
        }
        if row_identity(row, &target, viewer, owned).is_ok() {
            ids.insert(target);
        }
    }
    Ok(ids)
}

/// Batch lookup for contact rows, never one request per visible/recycled row.
pub(crate) async fn visible_agent_ids(
    viewer: &UserId,
) -> Result<std::collections::HashSet<OwnedUserId>, String> {
    let service = Service::for_current()?;
    let owned = service.execute(Command::Agents).await?;
    let mut ids = roster_agent_ids(&owned, viewer, true)?;
    if let Ok(public) = service.execute(Command::DiscoverAgents).await {
        ids.extend(roster_agent_ids(&public, viewer, false)?);
    }
    Ok(ids)
}

#[derive(Clone, Debug, PartialEq)]
struct ProfilePin {
    viewer: OwnedUserId,
    origin: String,
    epoch: u64,
    target: OwnedUserId,
}
impl ProfilePin {
    fn current(target: &UserId) -> Option<Self> {
        if crate::sliding_sync::account_changing()
            || crate::logout::logout_state_machine::is_logout_in_progress()
        {
            return None;
        }
        let client = get_client()?;
        Some(Self {
            viewer: client.user_id()?.to_owned(),
            origin: client.homeserver().to_string(),
            epoch: crate::account_session::epoch(),
            target: target.to_owned(),
        })
    }
    fn is_current(&self) -> bool {
        Self::current(&self.target).as_ref() == Some(self)
    }
}

#[derive(Debug)]
struct AgentDetailsLoaded {
    pin: ProfilePin,
    request: u64,
    result: Result<Option<AgentIdentity>, String>,
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let IdentityRow = View {
        width: Fill height: Fit flow: Right spacing: 16 align: Align{y: 0.5}
        padding: Inset{left: 24 right: 24 top: 12 bottom: 12}
        title := DetailLabel {width: 100 draw_text.color: mod.widgets.RINX_MUTED}
        value := DetailLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} max_lines: 4 text_overflow: Ellipsis}
    }
    mod.widgets.AgentProfileDetails = #(AgentProfileDetails::register_widget(vm)) {
        ..mod.widgets.View
        width: Fill height: Fit flow: Down visible: false
        info := DetailSection {
            account_type := IdentityRow {title.text: #(crate::i18n::tr("Account type")) title.i18n_text: "Account type"}
            owner := IdentityRow {title.text: #(crate::i18n::tr("Agent owner")) title.i18n_text: "Agent owner"}
            visibility := IdentityRow {title.text: #(crate::i18n::tr("Visibility")) title.i18n_text: "Visibility"}
        }
        status := DetailSection {
            View {
                width: Fill height: Fit flow: Right spacing: 12 padding: Inset{left: 24 right: 24 top: 12 bottom: 12} align: Align{y: 0.5}
                text := DetailLabel {width: Fill height: Fit flow: Flow.Right{wrap: true} draw_text.color: mod.widgets.RINX_MUTED}
                retry := RinxGhostButton {text: #(crate::i18n::tr("Retry")) i18n_text: "Retry"}
            }
        }
        DetailGap {}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct AgentProfileDetails {
    #[deref]
    view: View,
    #[rust]
    pin: Option<ProfilePin>,
    #[rust]
    request: u64,
    #[rust]
    allows_moments: bool,
}
impl AgentProfileDetails {
    fn status(&mut self, cx: &mut Cx, loading: bool) {
        self.allows_moments = false;
        self.view.set_visible(cx, true);
        self.view.view(cx, ids!(info)).set_visible(cx, false);
        self.view.view(cx, ids!(status)).set_visible(cx, true);
        self.view.button(cx, ids!(retry)).set_visible(cx, !loading);
        self.view.label(cx, ids!(status.text)).set_text(
            cx,
            crate::i18n::tr(if loading {
                "Checking Agent information…"
            } else {
                "Could not load Agent details."
            }),
        );
        self.view.redraw(cx);
    }
    fn refresh(&mut self, cx: &mut Cx) {
        let Some(pin) = self.pin.clone().filter(ProfilePin::is_current) else {
            return;
        };
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        self.request = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let request = self.request;
        self.status(cx, true);
        let Ok(service) = Service::for_current() else {
            self.status(cx, false);
            return;
        };
        spawn_async_task(async move {
            let target = pin.target.clone();
            let viewer = pin.viewer.clone();
            // Dropping a timed-out JoinHandle detaches the read. Owner bootstrap
            // must finish its authorization bookkeeping even if this UI moved on.
            let work = tokio::spawn(async move {
                let owned = service.execute(Command::Agents).await?;
                if let Some(identity) = find_identity(&owned, &target, &viewer, true)? {
                    return Ok(Some(identity));
                }
                let public = service.execute(Command::DiscoverAgents).await?;
                find_identity(&public, &target, &viewer, false)
            });
            let result = match tokio::time::timeout(std::time::Duration::from_secs(25), work).await
            {
                Ok(Ok(result)) => result,
                _ => Err("agent_details_unavailable".into()),
            };
            Cx::post_action(AgentDetailsLoaded {
                pin,
                request,
                result,
            });
        });
    }
    fn accepts(&self, loaded: &AgentDetailsLoaded) -> bool {
        self.request == loaded.request
            && self.pin.as_ref() == Some(&loaded.pin)
            && loaded.pin.is_current()
    }
    fn show_identity(&mut self, cx: &mut Cx, identity: Option<&AgentIdentity>) {
        self.allows_moments = identity.is_none();
        self.view.set_visible(cx, identity.is_some());
        self.view.view(cx, ids!(status)).set_visible(cx, false);
        self.view
            .view(cx, ids!(info))
            .set_visible(cx, identity.is_some());
        if let Some(identity) = identity {
            let mine = self
                .pin
                .as_ref()
                .is_some_and(|pin| pin.viewer == identity.owner);
            self.view
                .label(cx, ids!(account_type.value))
                .set_text(cx, crate::i18n::tr(if mine { "My Agent" } else { "Agent" }));
            self.view
                .label(cx, ids!(owner.value))
                .set_text(cx, identity.owner.as_str());
            self.view.label(cx, ids!(visibility.value)).set_text(
                cx,
                crate::i18n::tr(match identity.visibility {
                    Some(true) => "Public",
                    Some(false) => "Private",
                    None => "Unknown",
                }),
            );
        }
        self.view.redraw(cx);
    }
    fn set_user(&mut self, cx: &mut Cx, user: Option<&UserId>) {
        let pin = user.and_then(ProfilePin::current);
        if self.pin == pin {
            return;
        }
        self.pin = pin;
        self.request = 0;
        self.show_identity(cx, None);
        self.allows_moments = false;
        self.refresh(cx);
    }
}
impl Widget for AgentProfileDetails {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.pin.as_ref().is_some_and(|pin| !pin.is_current()) {
            self.set_user(cx, None);
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            for action in actions {
                if let Some(loaded) = action.downcast_ref::<AgentDetailsLoaded>()
                    && self.accepts(loaded)
                {
                    match &loaded.result {
                        Ok(identity) => self.show_identity(cx, identity.as_ref()),
                        Err(_) => self.status(cx, false),
                    }
                }
            }
            if self.view.button(cx, ids!(retry)).clicked(actions) {
                self.refresh(cx);
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}
impl AgentProfileDetailsRef {
    pub fn allows_moments(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.allows_moments)
    }
    pub fn set_user(&self, cx: &mut Cx, user: Option<&UserId>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_user(cx, user);
        }
    }
}

pub(crate) async fn visible_agent_owners(viewer:&UserId)->Result<std::collections::HashMap<OwnedUserId,OwnedUserId>,String> {
    let service=Service::for_current()?;
    let owned=service.execute(Command::Agents).await?;
    let public=service.execute(Command::DiscoverAgents).await?;
    let mut owners=std::collections::HashMap::new();
    for (roster,own) in [(&owned,true),(&public,false)] {
        let ids=roster_agent_ids(roster,viewer,own)?;
        for target in ids {
            if let Some(identity)=find_identity(roster,&target,viewer,own)? {
                if owners.get(&target).is_some_and(|owner|owner!=&identity.owner) {return Err("ambiguous_agent_identity".into());}
                owners.insert(target,identity.owner);
            }
        }
    }
    Ok(owners)
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;

    #[test]
    fn own_agent_has_verified_owner_and_explicit_or_unknown_visibility() {
        let viewer = UserId::parse("@owner:example.org").unwrap();
        let target = UserId::parse("@agent:example.org").unwrap();
        let mut roster =
            json!({"ownerMxid":viewer,"agents":[{"puppetMxid":target,"visibility":"private"}]});
        assert_eq!(
            find_identity(&roster, &target, &viewer, true).unwrap(),
            Some(AgentIdentity {
                owner: viewer.clone(),
                visibility: Some(false)
            })
        );
        roster["agents"][0]["visibility"] = "public".into();
        assert_eq!(
            find_identity(&roster, &target, &viewer, true)
                .unwrap()
                .unwrap()
                .visibility,
            Some(true)
        );
        roster["agents"][0]
            .as_object_mut()
            .unwrap()
            .remove("visibility");
        assert_eq!(
            find_identity(&roster, &target, &viewer, true)
                .unwrap()
                .unwrap()
                .visibility,
            None
        );
        roster["ownerMxid"] = "@other:example.org".into();
        assert!(find_identity(&roster, &target, &viewer, true).is_err());
    }

    #[test]
    fn public_agent_requires_exact_identity_owner_and_public_visibility() {
        let viewer = UserId::parse("@viewer:example.org").unwrap();
        let target = UserId::parse("@agent:example.org").unwrap();
        let owner = UserId::parse("@owner:example.org").unwrap();
        let row =
            json!({"puppetMxid":target,"ownerMxid":owner,"isMine":false,"visibility":"public"});
        let roster = json!({"agents":[row.clone()]});
        assert!(
            roster_agent_ids(&roster, &viewer, false)
                .unwrap()
                .contains(&target)
        );
        assert!(
            roster_agent_ids(&json!({"agents":[row.clone(),row.clone()]}), &viewer, false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            find_identity(&roster, &target, &viewer, false).unwrap(),
            Some(AgentIdentity {
                owner,
                visibility: Some(true)
            })
        );
        assert_eq!(
            find_identity(
                &roster,
                &UserId::parse("@other:example.org").unwrap(),
                &viewer,
                false
            )
            .unwrap(),
            None
        );
        for (key, value) in [
            ("visibility", json!("private")),
            ("isMine", json!(true)),
            ("ownerMxid", json!("invalid")),
            ("ownerMxid", json!(target)),
        ] {
            let mut invalid = roster.clone();
            invalid["agents"][0][key] = value;
            assert!(find_identity(&invalid, &target, &viewer, false).is_err());
            assert!(
                roster_agent_ids(&invalid, &viewer, false)
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(
            find_identity(
                &json!({"agents":[row.clone(),row]}),
                &target,
                &viewer,
                false
            )
            .is_err()
        );
        assert!(
            find_identity(
                &json!({"agents":vec![json!({"puppetMxid":"@unrelated:example.org"});256]}),
                &target,
                &viewer,
                false
            )
            .is_err()
        );
        // A familiar prefix is not evidence that an arbitrary Matrix user is an Agent.
        assert_eq!(
            find_identity(
                &json!({"agents":[]}),
                &UserId::parse("@_hagency_agt_fake:example.org").unwrap(),
                &viewer,
                false
            )
            .unwrap(),
            None
        );
    }

    fn draw(cx: &mut Cx, widget: &mut AgentProfileDetails, width: f64) {
        cx.redraw_all();
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(width, 760.0));
        let mut list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(width, 760.0), Layout::flow_overlay());
        widget.draw_walk_all(
            &mut cx2d,
            &mut Scope::empty(),
            Walk {
                width: Size::Fixed(width),
                height: Size::fit(),
                ..Walk::default()
            },
        );
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    #[test]
    fn profile_rows_show_mine_owner_visibility_and_clear_with_selection() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut widget = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.AgentProfileDetails {}});
            let widget = AgentProfileDetails::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            widget
        });
        let viewer = UserId::parse("@owner:example.org").unwrap();
        widget.pin = Some(ProfilePin {
            viewer: viewer.clone(),
            origin: "https://example.org/".into(),
            epoch: 1,
            target: UserId::parse("@agent:example.org").unwrap(),
        });
        for width in [320.0, 790.0] {
            for (owner, visibility, kind, audience) in [
                (viewer.clone(), Some(false), "My Agent", "Private"),
                (
                    UserId::parse("@another-owner-with-a-long-identifier:example.org").unwrap(),
                    Some(true),
                    "Agent",
                    "Public",
                ),
                (viewer.clone(), None, "My Agent", "Unknown"),
            ] {
                widget.show_identity(
                    &mut cx,
                    Some(&AgentIdentity {
                        owner: owner.clone(),
                        visibility,
                    }),
                );
                draw(&mut cx, &mut widget, width);
                assert_eq!(
                    widget.view.label(&cx, ids!(account_type.value)).text(),
                    crate::i18n::tr(kind)
                );
                assert_eq!(
                    widget.view.label(&cx, ids!(owner.value)).text(),
                    owner.as_str()
                );
                assert_eq!(
                    widget.view.label(&cx, ids!(visibility.value)).text(),
                    crate::i18n::tr(audience)
                );
                let value = widget.view.label(&cx, ids!(owner.value)).area().rect(&cx);
                assert!(
                    value.size.x > 0.0 && value.pos.x + value.size.x <= width + 1.0,
                    "{value:?} within {width}"
                );
                assert!(widget.view.view(&cx, ids!(info)).visible());
                assert!(!widget.allows_moments, "Agents have no Moments entry");
            }
        }
        widget.show_identity(&mut cx, None);
        assert!(widget.allows_moments, "Ordinary contacts keep Moments");
        widget.set_user(&mut cx, None);
        assert!(!widget.allows_moments);
        assert!(!widget.view.visible());
        assert!(widget.pin.is_none());
        assert_eq!(widget.request, 0);
        widget.status(&mut cx, false);
        assert!(
            !widget.allows_moments,
            "Do not flash Moments while identity is unknown"
        );
        assert!(!widget.view.view(&cx, ids!(info)).visible());
        assert!(widget.view.button(&cx, ids!(retry)).visible());
    }
}
