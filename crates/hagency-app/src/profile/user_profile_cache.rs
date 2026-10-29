//! A cache of user profiles and room membership info, indexed by user ID.
//!
//! The cache is only accessible from the main UI thread.

use makepad_widgets::{warning, Cx, SignalToUI};
use matrix_sdk::{
    room::RoomMember,
    ruma::{OwnedRoomId, OwnedUserId, UserId},
};
use std::{
    cell::RefCell,
    collections::{btree_map::Entry, BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock, Mutex,
    },
    time::{Duration, Instant},
};

const MAX_PROFILES: usize = 2048;
const MAX_MEMBERSHIPS: usize = 8192;
const MAX_ROOMS_PER_USER: usize = 64;
const MAX_PENDING_UPDATES: usize = 512;
const UPDATE_BATCH: usize = 32;
const UPDATE_TIME: Duration = Duration::from_millis(4);
const VISIBLE_GRACE: Duration = Duration::from_secs(2);
const IDLE_TTL: Duration = Duration::from_secs(15 * 60);
const REQUEST_TTL: Duration = Duration::from_secs(30);
type ProfileKey = (OwnedUserId, Option<OwnedRoomId>);

// One deferred wakeup for all budget misses. The permit is captured before the
// task starts so account-scope cancellation also releases this admission flag.
#[derive(Default)]
struct RetryState {
    active: AtomicBool,
    due: Mutex<Option<u64>>,
}
static RETRY_WAKE: LazyLock<Arc<RetryState>> = LazyLock::new(|| Arc::new(RetryState::default()));
struct RetryWake(Arc<RetryState>);
impl RetryWake {
    fn claim(active: Arc<RetryState>) -> Option<Self> {
        active
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(Self(active))
    }
}
impl Drop for RetryWake {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::Release);
    }
}
fn schedule_profile_retry() {
    let Some(permit) = RetryWake::claim(RETRY_WAKE.clone()) else {
        return;
    };
    let epoch = crate::account_session::epoch();
    crate::account_session::spawn(retry_wakeup(permit, epoch, VISIBLE_GRACE));
}

async fn retry_wakeup(permit: RetryWake, epoch: u64, delay: Duration) {
    tokio::time::sleep(delay).await;
    // A wakeup with no completion still needs a UI redraw so visible consumers
    // actually repeat their lookup. Carry the originating account epoch.
    *permit.0.due.lock().unwrap() = Some(epoch);
    // Release before signalling: a lookup triggered by the wakeup may need
    // to defer again if the visible working set still exceeds its budget.
    drop(permit);
    SignalToUI::set_ui_signal();
}

#[derive(Clone, Copy)]
struct Access {
    used: Instant,
    requested: Option<Instant>,
}
#[derive(Default)]
struct ProfileCache {
    entries: BTreeMap<OwnedUserId, UserProfileCacheEntry>,
    access: BTreeMap<ProfileKey, Access>,
    evicted: u64,
    refused: u64,
}

/// Application-owned counts, not a whole-process memory measurement.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Opt-in diagnostics for native probes and stress tooling.
pub struct ProfileCacheStats {
    pub profiles: usize,
    pub memberships: usize,
    pub pending_updates: usize,
    pub retry_scheduled: bool,
    pub evicted: u64,
    pub refused: u64,
    pub coalesced: u64,
    pub dropped: u64,
}

impl ProfileCache {
    fn remove(&mut self, key: &ProfileKey) {
        self.access.remove(key);
        if let Some(room) = &key.1 {
            if let Some(UserProfileCacheEntry::Loaded { rooms, .. }) = self.entries.get_mut(&key.0)
            {
                rooms.remove(room);
            }
        } else {
            self.entries.remove(&key.0);
            self.access.retain(|(user, _), _| user != &key.0);
        }
        self.evicted += 1;
    }

    fn evict_oldest(&mut self, membership: bool, user: Option<&OwnedUserId>, now: Instant) -> bool {
        let victim = self
            .access
            .iter()
            .filter(|((candidate, room), access)| {
                room.is_some() == membership
                    && user.is_none_or(|user| user == candidate)
                    && now.saturating_duration_since(access.used) >= VISIBLE_GRACE
            })
            .min_by_key(|(_, access)| access.used)
            .map(|(key, _)| key.clone());
        if let Some(key) = victim {
            self.remove(&key);
            true
        } else {
            self.refused += 1;
            false
        }
    }

    fn reserve_member(&mut self, user: &OwnedUserId, room: &OwnedRoomId, now: Instant) -> bool {
        let key = (user.clone(), Some(room.clone()));
        if self.access.contains_key(&key) {
            return true;
        }
        let memberships = self
            .access
            .keys()
            .filter(|(_, room)| room.is_some())
            .count();
        let own = self
            .access
            .keys()
            .filter(|(id, room)| id == user && room.is_some())
            .count();
        if own >= MAX_ROOMS_PER_USER && !self.evict_oldest(true, Some(user), now) {
            return false;
        }
        if memberships >= MAX_MEMBERSHIPS
            && own < MAX_ROOMS_PER_USER
            && !self.evict_oldest(true, None, now)
        {
            return false;
        }
        self.access.insert(
            key,
            Access {
                used: now,
                requested: Some(now),
            },
        );
        true
    }

    /// Recently looked-up profiles/member relations are protected from background
    /// completion churn. At capacity with only visible entries, defer new lookups.
    /// A missing completion or failed account-wide request is retryable after 30s.
    fn lookup(
        &mut self,
        user: &OwnedUserId,
        room: Option<&OwnedRoomId>,
        fetch: bool,
        now: Instant,
    ) -> Option<Option<OwnedRoomId>> {
        let key = (user.clone(), None);
        if self.access.get(&key).is_some_and(|a| {
            now.saturating_duration_since(a.used) >= IDLE_TTL
                || a.requested
                    .is_some_and(|at| now.saturating_duration_since(at) >= REQUEST_TTL)
        }) {
            self.remove(&key);
        }
        if !self.entries.contains_key(user) {
            if !fetch {
                return None;
            }
            if self.entries.len() >= MAX_PROFILES && !self.evict_oldest(false, None, now) {
                return None;
            }
            self.entries
                .insert(user.clone(), UserProfileCacheEntry::Requested);
            self.access.insert(
                key.clone(),
                Access {
                    used: now,
                    requested: Some(now),
                },
            );
            let room = room
                .filter(|room| self.reserve_member(user, room, now))
                .cloned();
            return Some(room);
        }
        if let Some(access) = self.access.get_mut(&key) {
            access.used = now;
        }
        let room = room?;
        let key = (user.clone(), Some(room.clone()));
        if self.access.get(&key).is_some_and(|a| {
            now.saturating_duration_since(a.used) >= IDLE_TTL
                || a.requested
                    .is_some_and(|at| now.saturating_duration_since(at) >= REQUEST_TTL)
        }) {
            self.remove(&key);
        }
        if let Some(access) = self.access.get_mut(&key) {
            access.used = now;
        }
        if !fetch
            || matches!(
                self.entries.get(user),
                Some(UserProfileCacheEntry::Requested)
            )
        {
            return None;
        }
        let missing = matches!(self.entries.get(user), Some(UserProfileCacheEntry::Loaded { rooms, .. }) if !rooms.contains_key(room));
        if missing
            && !self.access.get(&key).is_some_and(|a| a.requested.is_some())
            && self.reserve_member(user, room, now)
        {
            if let Some(UserProfileCacheEntry::Loaded { rooms, .. }) = self.entries.get_mut(user) {
                rooms.insert(room.clone(), RoomMemberEntry::Requested);
            }
            return Some(Some(room.clone()));
        }
        None
    }

    fn apply(&mut self, update: UserProfileUpdate) -> bool {
        let key = update.key();
        // Late responses for evicted users/relations must not displace visible data.
        if !self.access.contains_key(&key) {
            return false;
        }
        update.apply_to_cache(&mut self.entries);
        if let Some(access) = self.access.get_mut(&key) {
            access.requested = None;
        }
        true
    }

    fn retry_dropped(&mut self, now: Instant) {
        for access in self.access.values_mut() {
            if access.requested.is_some() {
                access.requested = Some(now.checked_sub(REQUEST_TTL).unwrap_or(now));
            }
        }
    }

    fn clear_pending(&mut self) {
        self.entries
            .retain(|_, entry| !matches!(entry, UserProfileCacheEntry::Requested));
        for entry in self.entries.values_mut() {
            if let UserProfileCacheEntry::Loaded { rooms, .. } = entry {
                rooms.retain(|_, member| matches!(member, RoomMemberEntry::Loaded(_)));
            }
        }
        self.access
            .retain(|(user, room), _| match self.entries.get(user) {
                Some(UserProfileCacheEntry::Loaded { rooms, .. }) => {
                    room.as_ref().is_none_or(|room| rooms.contains_key(room))
                }
                _ => false,
            });
    }
}

use crate::{
    shared::avatar::AvatarState,
    sliding_sync::{submit_async_request, MatrixRequest},
};

use super::user_profile::UserProfile;

thread_local! {
    /// A cache of each user's profile and the rooms they are a member of, indexed by user ID.
    ///
    /// To be of any use, this cache must only be accessed by the main UI thread.
    static USER_PROFILE_CACHE: RefCell<ProfileCache> = RefCell::new(ProfileCache::default());
}
enum UserProfileCacheEntry {
    /// A request has been issued and we're waiting for it to complete.
    Requested,
    /// The profile has been successfully loaded from the server.
    Loaded {
        user_profile: UserProfile,
        rooms: BTreeMap<OwnedRoomId, RoomMemberEntry>,
    },
}

/// An entry in a loaded profile's map of per-room member info.
pub enum RoomMemberEntry {
    /// A request has been issued and we're waiting for it to complete.
    Requested,
    /// The room member info has been successfully loaded from the server.
    Loaded(RoomMember),
    /// The request completed but didn't return any member info.
    /// This means that the user isn't in that room, or the lookup failed.
    ///
    /// This won't be retried until the next transition from offline --> online.
    Failed,
}
impl RoomMemberEntry {
    /// Returns the loaded room member info, if any.
    pub fn loaded(&self) -> Option<&RoomMember> {
        match self {
            RoomMemberEntry::Loaded(member) => Some(member),
            RoomMemberEntry::Requested | RoomMemberEntry::Failed => None,
        }
    }
}

/// Removes all `Requested` entries from the cache, allowing them to be re-fetched.
///
/// This should be called when the app transitions from offline back to online,
/// because any in-flight requests that were submitted while offline have likely
/// failed without updating the cache, leaving stale `Requested` entries that
/// permanently block re-fetching.
pub fn clear_all_pending_requests() {
    USER_PROFILE_CACHE.with_borrow_mut(ProfileCache::clear_pending);
}

#[derive(Default)]
struct PendingUpdates {
    order: VecDeque<ProfileKey>,
    updates: BTreeMap<ProfileKey, UserProfileUpdate>,
    epoch: u64,
    overflowed: bool,
    coalesced: u64,
    dropped: u64,
}
impl PendingUpdates {
    fn push(&mut self, epoch: u64, update: UserProfileUpdate) {
        if self.epoch != epoch {
            self.order.clear();
            self.updates.clear();
            self.overflowed = false;
            self.epoch = epoch;
        }
        // Splitting full completions prevents a later partial update for the
        // same member from accidentally discarding its account profile fields.
        if let UserProfileUpdate::Full {
            new_profile,
            room_id,
            room_member,
        } = update
        {
            self.push(epoch, UserProfileUpdate::UserProfileOnly(new_profile));
            self.push(
                epoch,
                UserProfileUpdate::RoomMemberOnly {
                    room_id,
                    room_member,
                },
            );
            return;
        }
        let key = update.key();
        if let Some(previous) = self.updates.get_mut(&key) {
            self.coalesced += 1;
            // As with apply_to_cache, failed lookups cannot erase loaded membership.
            if !matches!(previous, UserProfileUpdate::RoomMemberOnly { .. })
                || !matches!(update, UserProfileUpdate::RoomMemberFailed { .. })
            {
                *previous = update;
            }
            return;
        }
        if self.order.len() >= MAX_PENDING_UPDATES {
            if let Some(oldest) = self.order.pop_front() {
                self.updates.remove(&oldest);
            }
            self.dropped += 1;
            self.overflowed = true;
        }
        self.order.push_back(key.clone());
        self.updates.insert(key, update);
    }
    fn pop(&mut self) -> Option<UserProfileUpdate> {
        let key = self.order.pop_front()?;
        self.updates.remove(&key)
    }
}

/// Completions are coalesced by user/member identity and bounded independently
/// of the UI cache. On overload, pending lookups become retryable on the UI.
static PENDING_USER_PROFILE_UPDATES: LazyLock<Mutex<PendingUpdates>> =
    LazyLock::new(|| Mutex::new(PendingUpdates::default()));

pub fn enqueue_user_profile_update(update: UserProfileUpdate) {
    PENDING_USER_PROFILE_UPDATES
        .lock()
        .unwrap()
        .push(crate::account_session::epoch(), update);
    SignalToUI::set_ui_signal();
}

/// Inspect on the UI thread, including bounded completion-queue pressure.
#[allow(dead_code)] // Public probe used by optional native/stress diagnostics.
pub fn profile_cache_stats(_cx: &Cx) -> ProfileCacheStats {
    let pending = PENDING_USER_PROFILE_UPDATES.lock().unwrap();
    USER_PROFILE_CACHE.with_borrow(|cache| ProfileCacheStats {
        profiles: cache.entries.len(),
        memberships: cache
            .access
            .keys()
            .filter(|(_, room)| room.is_some())
            .count(),
        pending_updates: pending.updates.len(),
        retry_scheduled: RETRY_WAKE.active.load(Ordering::Acquire),
        evicted: cache.evicted,
        refused: cache.refused,
        coalesced: pending.coalesced,
        dropped: pending.dropped,
    })
}

/// A user profile update, which can include changes to a user's full profile
/// and/or room membership info.
pub enum UserProfileUpdate {
    /// A fully-fetched user profile, with info about the user's membership in a given room.
    Full {
        new_profile: UserProfile,
        room_id: OwnedRoomId,
        room_member: RoomMember,
    },
    /// An update to the user's room membership info only, without any profile changes.
    RoomMemberOnly {
        room_id: OwnedRoomId,
        room_member: RoomMember,
    },
    /// An update to the user's profile only, without changes to room membership info.
    UserProfileOnly(UserProfile),
    /// A room-specific user profile request failed, meaning the user isn't in that room.
    RoomMemberFailed {
        user_id: OwnedUserId,
        room_id: OwnedRoomId,
    },
}
impl UserProfileUpdate {
    /// Returns the user ID associated with this update.
    #[allow(unused)]
    pub fn user_id(&self) -> &UserId {
        match self {
            UserProfileUpdate::Full { new_profile, .. } => &new_profile.user_id,
            UserProfileUpdate::RoomMemberOnly { room_member, .. } => room_member.user_id(),
            UserProfileUpdate::UserProfileOnly(profile) => &profile.user_id,
            UserProfileUpdate::RoomMemberFailed { user_id, .. } => user_id,
        }
    }

    fn key(&self) -> ProfileKey {
        let room = match self {
            Self::Full { room_id, .. }
            | Self::RoomMemberOnly { room_id, .. }
            | Self::RoomMemberFailed { room_id, .. } => Some(room_id.clone()),
            Self::UserProfileOnly(_) => None,
        };
        (self.user_id().to_owned(), room)
    }

    /// Applies this update to the given user profile info cache.
    fn apply_to_cache(self, cache: &mut BTreeMap<OwnedUserId, UserProfileCacheEntry>) {
        match self {
            UserProfileUpdate::Full {
                new_profile,
                room_id,
                room_member,
            } => match cache.entry(new_profile.user_id.clone()) {
                Entry::Occupied(mut entry) => match entry.get_mut() {
                    e @ UserProfileCacheEntry::Requested => {
                        *e = UserProfileCacheEntry::Loaded {
                            user_profile: new_profile,
                            rooms: {
                                let mut room_members_map = BTreeMap::new();
                                room_members_map
                                    .insert(room_id, RoomMemberEntry::Loaded(room_member));
                                room_members_map
                            },
                        };
                    }
                    UserProfileCacheEntry::Loaded {
                        user_profile,
                        rooms,
                    } => {
                        *user_profile = new_profile;
                        rooms.insert(room_id, RoomMemberEntry::Loaded(room_member));
                    }
                },
                Entry::Vacant(entry) => {
                    entry.insert(UserProfileCacheEntry::Loaded {
                        user_profile: new_profile,
                        rooms: {
                            let mut room_members_map = BTreeMap::new();
                            room_members_map.insert(room_id, RoomMemberEntry::Loaded(room_member));
                            room_members_map
                        },
                    });
                }
            },
            UserProfileUpdate::RoomMemberOnly {
                room_id,
                room_member,
            } => {
                match cache.entry(room_member.user_id().to_owned()) {
                    Entry::Occupied(mut entry) => match entry.get_mut() {
                        e @ UserProfileCacheEntry::Requested => {
                            // This shouldn't happen, but we can still technically handle it correctly.
                            warning!(
                                "BUG: User profile cache entry was `Requested` for user {} when handling RoomMemberOnly update",
                                room_member.user_id()
                            );
                            *e = UserProfileCacheEntry::Loaded {
                                user_profile: UserProfile {
                                    user_id: room_member.user_id().to_owned(),
                                    username: None,
                                    avatar_state: AvatarState::Known(
                                        room_member.avatar_url().map(|url| url.to_owned()),
                                    ),
                                },
                                rooms: {
                                    let mut room_members_map = BTreeMap::new();
                                    room_members_map
                                        .insert(room_id, RoomMemberEntry::Loaded(room_member));
                                    room_members_map
                                },
                            };
                        }
                        UserProfileCacheEntry::Loaded { rooms, .. } => {
                            rooms.insert(room_id, RoomMemberEntry::Loaded(room_member));
                        }
                    },
                    Entry::Vacant(entry) => {
                        // This shouldn't happen, but we can still technically handle it correctly.
                        warning!(
                            "BUG: User profile cache entry not found for user {} when handling RoomMemberOnly update",
                            room_member.user_id()
                        );
                        entry.insert(UserProfileCacheEntry::Loaded {
                            user_profile: UserProfile {
                                user_id: room_member.user_id().to_owned(),
                                username: None,
                                avatar_state: AvatarState::Known(
                                    room_member.avatar_url().map(|url| url.to_owned()),
                                ),
                            },
                            rooms: {
                                let mut room_members_map = BTreeMap::new();
                                room_members_map
                                    .insert(room_id, RoomMemberEntry::Loaded(room_member));
                                room_members_map
                            },
                        });
                    }
                }
            }
            UserProfileUpdate::UserProfileOnly(new_profile) => {
                match cache.entry(new_profile.user_id.clone()) {
                    Entry::Occupied(mut entry) => match entry.get_mut() {
                        e @ UserProfileCacheEntry::Requested => {
                            *e = UserProfileCacheEntry::Loaded {
                                user_profile: new_profile,
                                rooms: BTreeMap::new(),
                            };
                        }
                        UserProfileCacheEntry::Loaded { user_profile, .. } => {
                            *user_profile = new_profile;
                        }
                    },
                    Entry::Vacant(entry) => {
                        entry.insert(UserProfileCacheEntry::Loaded {
                            user_profile: new_profile,
                            rooms: BTreeMap::new(),
                        });
                    }
                }
            }
            UserProfileUpdate::RoomMemberFailed { user_id, room_id } => {
                if let Some(UserProfileCacheEntry::Loaded { rooms, .. }) = cache.get_mut(&user_id) {
                    // Don't overwrite actual member profile data that does exist.
                    let member = rooms.entry(room_id).or_insert(RoomMemberEntry::Failed);
                    if matches!(member, RoomMemberEntry::Requested) {
                        *member = RoomMemberEntry::Failed;
                    }
                }
            }
        }
    }
}

/// Processes all pending user profile updates in the queue.
///
/// Repaints profile consumers only when data arrived. The `Cx` reference also
/// confines the cache to the UI thread.
pub fn process_user_profile_updates(cx: &mut Cx) {
    let mut pending = PENDING_USER_PROFILE_UPDATES.lock().unwrap();
    USER_PROFILE_CACHE.with_borrow_mut(|cache| {
        process_updates_on_ui(
            cx,
            cache,
            &mut pending,
            &RETRY_WAKE,
            crate::account_session::epoch(),
        );
    });
}

fn process_updates_on_ui(
    cx: &mut Cx,
    cache: &mut ProfileCache,
    pending: &mut PendingUpdates,
    retry: &RetryState,
    epoch: u64,
) {
    // Consume even when this account has never received a completion: its
    // empty queue may still carry the initial/default epoch.
    let retry_due = retry.due.lock().unwrap().take() == Some(epoch);
    let (changed, more) = if pending.epoch == epoch {
        drain_updates(cache, pending)
    } else {
        (false, false)
    };
    if changed || retry_due {
        cx.redraw_all();
    }
    if more {
        SignalToUI::set_ui_signal();
    }
}

fn drain_updates(cache: &mut ProfileCache, pending: &mut PendingUpdates) -> (bool, bool) {
    let started = Instant::now();
    let retry = std::mem::take(&mut pending.overflowed);
    let mut changed = retry;
    if retry {
        cache.retry_dropped(started);
    }
    for _ in 0..UPDATE_BATCH {
        if started.elapsed() >= UPDATE_TIME {
            break;
        }
        let Some(update) = pending.pop() else {
            break;
        };
        changed |= cache.apply(update);
    }
    (changed, !pending.order.is_empty())
}

/// Invokes the given closure with cached user profile info for the given user ID
/// (optionally in the given room) if it exists in the cache, otherwise does nothing.
///
/// This function requires passing in a reference to `Cx`,
/// which isn't used, but acts as a guarantee that this function
/// must only be called by the main UI thread.
pub fn with_user_profile<F, R>(
    _cx: &mut Cx,
    user_id: OwnedUserId,
    room_id: Option<&OwnedRoomId>,
    fetch_if_missing: bool,
    f: F,
) -> Option<R>
where
    F: FnOnce(&UserProfile, &BTreeMap<OwnedRoomId, RoomMemberEntry>) -> R,
{
    USER_PROFILE_CACHE.with_borrow_mut(|cache| {
        let refused = cache.refused;
        let request = cache.lookup(&user_id, room_id, fetch_if_missing, Instant::now());
        if cache.refused != refused {
            schedule_profile_retry();
        }
        if let Some(room_id) = request {
            submit_async_request(MatrixRequest::GetUserProfile {
                user_id: user_id.clone(),
                room_id,
                local_only: false,
            });
        }
        match cache.entries.get(&user_id) {
            Some(UserProfileCacheEntry::Loaded {
                user_profile,
                rooms,
            }) => Some(f(user_profile, rooms)),
            _ => None,
        }
    })
}

/// Returns the given user's displayable name (optionally in the given room),
/// using the user's account-wide displayable name as a fallback.
///
/// If either the `user_id` or `room_id` wasn't found in the cache,
/// and if `fetch_if_missing` is true, then this function will submit a request
/// to asynchronously fetch the user's room membership info from the server.
///
/// This function requires passing in a reference to `Cx`,
/// which isn't used, but acts as a guarantee that this function
/// must only be called by the main UI thread.
pub fn get_user_display_name_for_room(
    cx: &mut Cx,
    user_id: OwnedUserId,
    room_id: Option<&OwnedRoomId>,
    fetch_if_missing: bool,
) -> CachedName {
    let opt = with_user_profile(cx, user_id, room_id, fetch_if_missing, |profile, rooms| {
        room_id
            .and_then(|id| rooms.get(id))
            .and_then(RoomMemberEntry::loaded)
            .map_or_else(
                || CachedName::FoundInProfile(profile.username.clone()),
                |rm| CachedName::FoundInRoom(rm.display_name().map(|n| n.to_owned())),
            )
    });
    opt.unwrap_or(CachedName::NotFound)
}

/// A user's display name in our cache.
pub enum CachedName {
    /// The user's display name was found for the specified room (most accurate).
    /// If `None`, they did not set a display name for that room.
    FoundInRoom(Option<String>),
    /// The user's display name was found in their general account profile.
    /// If `None`, they have not set a display name at all.
    FoundInProfile(Option<String>),
    /// No info about the user was found in the cache.
    NotFound,
}
impl CachedName {
    pub fn was_found(&self) -> bool {
        matches!(self, Self::FoundInRoom(_) | Self::FoundInProfile(_))
    }

    pub fn into_option(self) -> Option<String> {
        self.into()
    }

    pub fn as_deref(&self) -> Option<&str> {
        match self {
            CachedName::FoundInRoom(name) | CachedName::FoundInProfile(name) => name.as_deref(),
            CachedName::NotFound => None,
        }
    }
}
impl From<CachedName> for Option<String> {
    fn from(cached_name: CachedName) -> Self {
        match cached_name {
            CachedName::FoundInRoom(name) => name,
            CachedName::FoundInProfile(name) => name,
            CachedName::NotFound => None,
        }
    }
}

/// Clears cached user profile.
/// This function requires passing in a reference to `Cx`,
/// which acts as a guarantee that these thread-local caches are cleared on the main UI thread,
pub fn clear_user_profile_cache(_cx: &mut Cx) {
    *PENDING_USER_PROFILE_UPDATES.lock().unwrap() = PendingUpdates::default();
    RETRY_WAKE.due.lock().unwrap().take();
    USER_PROFILE_CACHE.with_borrow_mut(|cache| *cache = ProfileCache::default());
}

#[cfg(test)]
mod tests_room_member_entry {
    use super::*;
    use matrix_sdk::ruma::{room_id, user_id};

    fn loaded_entry(rooms: BTreeMap<OwnedRoomId, RoomMemberEntry>) -> UserProfileCacheEntry {
        UserProfileCacheEntry::Loaded {
            user_profile: UserProfile {
                user_id: user_id!("@alice:matrix.org").to_owned(),
                username: Some("Alice".into()),
                avatar_state: AvatarState::Unknown,
            },
            rooms,
        }
    }

    fn apply_failed(cache: &mut BTreeMap<OwnedUserId, UserProfileCacheEntry>) {
        UserProfileUpdate::RoomMemberFailed {
            user_id: user_id!("@alice:matrix.org").to_owned(),
            room_id: room_id!("!room:matrix.org").to_owned(),
        }
        .apply_to_cache(cache);
    }

    fn room_entry(
        cache: &BTreeMap<OwnedUserId, UserProfileCacheEntry>,
    ) -> Option<&RoomMemberEntry> {
        match cache.get(user_id!("@alice:matrix.org"))? {
            UserProfileCacheEntry::Loaded { rooms, .. } => rooms.get(room_id!("!room:matrix.org")),
            UserProfileCacheEntry::Requested => None,
        }
    }

    #[test]
    fn pending_room_entry_becomes_failed() {
        let mut rooms = BTreeMap::new();
        rooms.insert(
            room_id!("!room:matrix.org").to_owned(),
            RoomMemberEntry::Requested,
        );
        let mut cache = BTreeMap::new();
        cache.insert(
            user_id!("@alice:matrix.org").to_owned(),
            loaded_entry(rooms),
        );

        apply_failed(&mut cache);
        assert!(matches!(room_entry(&cache), Some(RoomMemberEntry::Failed)));
    }

    #[test]
    fn missing_room_entry_is_inserted_as_failed() {
        let mut cache = BTreeMap::new();
        cache.insert(
            user_id!("@alice:matrix.org").to_owned(),
            loaded_entry(BTreeMap::new()),
        );

        apply_failed(&mut cache);
        assert!(matches!(room_entry(&cache), Some(RoomMemberEntry::Failed)));
    }

    #[test]
    fn failure_for_an_uncached_user_is_ignored() {
        let mut cache = BTreeMap::new();
        apply_failed(&mut cache);
        assert!(cache.is_empty());
    }

    #[test]
    fn a_still_pending_account_wide_entry_is_left_alone() {
        let mut cache = BTreeMap::new();
        cache.insert(
            user_id!("@alice:matrix.org").to_owned(),
            UserProfileCacheEntry::Requested,
        );

        apply_failed(&mut cache);
        assert!(matches!(
            cache.get(user_id!("@alice:matrix.org")),
            Some(UserProfileCacheEntry::Requested)
        ));
    }

    #[test]
    fn failed_is_not_a_loaded_member() {
        assert!(RoomMemberEntry::Failed.loaded().is_none());
        assert!(RoomMemberEntry::Requested.loaded().is_none());
    }
}

#[cfg(test)]
mod bounded_profile_tests {
    use super::*;

    fn user(index: usize) -> OwnedUserId {
        format!("@user{index}:example.org").try_into().unwrap()
    }
    fn room(index: usize) -> OwnedRoomId {
        format!("!room{index}:example.org").try_into().unwrap()
    }
    fn profile(user: OwnedUserId, name: &str) -> UserProfileUpdate {
        UserProfileUpdate::UserProfileOnly(UserProfile {
            user_id: user,
            username: Some(name.into()),
            avatar_state: AvatarState::Unknown,
        })
    }
    fn loaded(cache: &mut ProfileCache, user: &OwnedUserId, now: Instant) {
        assert_eq!(cache.lookup(user, None, true, now), Some(None));
        assert!(cache.apply(profile(user.clone(), "loaded")));
    }

    #[test]
    fn budget_misses_share_one_wakeup_and_cancelled_wakeup_can_be_rescheduled() {
        let active = Arc::new(RetryState::default());
        let wakeup = RetryWake::claim(active.clone()).unwrap();
        for _ in 0..10_000 {
            assert!(RetryWake::claim(active.clone()).is_none());
        }
        // Dropping an unpolled account task or a finished timer both free it.
        let unpolled = async move {
            let _wakeup = wakeup;
            std::future::pending::<()>().await;
        };
        drop(unpolled);
        assert!(!active.active.load(Ordering::Acquire));
        let next = RetryWake::claim(active.clone()).unwrap();
        assert!(active.active.load(Ordering::Acquire));
        drop(next);
        assert!(!active.active.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn expired_retry_redraws_an_idle_page_even_before_its_first_completion() {
        let mut cache = ProfileCache::default();
        let now = Instant::now();
        for index in 0..MAX_PROFILES {
            loaded(&mut cache, &user(index), now);
        }
        assert_eq!(cache.lookup(&user(MAX_PROFILES), None, true, now), None);
        let state = Arc::new(RetryState::default());
        let permit = RetryWake::claim(state.clone()).unwrap();
        // Execute the production timer completion, with an abbreviated test delay.
        retry_wakeup(permit, 7, Duration::from_millis(1)).await;
        assert_eq!(*state.due.lock().unwrap(), Some(7));
        assert!(!state.active.load(Ordering::Acquire));
        let mut pending = PendingUpdates::default();
        assert_ne!(
            pending.epoch, 7,
            "no completion has initialized this account's queue"
        );
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.new_draw_event.redraw_all = false;
        process_updates_on_ui(&mut cx, &mut cache, &mut pending, &state, 7);
        assert!(
            cx.new_draw_event.redraw_all,
            "timer Signal must cause actual repaint without any completion"
        );
        assert_eq!(
            *state.due.lock().unwrap(),
            None,
            "the redraw cause is consumed once"
        );
        assert_eq!(
            cache.lookup(&user(MAX_PROFILES), None, true, now + VISIBLE_GRACE),
            Some(None),
            "repaint-time lookup can retry after the visible grace expires"
        );
        cx.new_draw_event.redraw_all = false;
        process_updates_on_ui(&mut cx, &mut cache, &mut pending, &state, 7);
        assert!(
            !cx.new_draw_event.redraw_all,
            "an unrelated Signal must not start an idle repaint loop"
        );
        *state.due.lock().unwrap() = Some(6);
        process_updates_on_ui(&mut cx, &mut cache, &mut pending, &state, 7);
        assert!(
            !cx.new_draw_event.redraw_all,
            "another account's timer cannot repaint the new account"
        );
    }

    #[test]
    fn profile_budget_protects_visible_entries_then_evicts_least_recently_used() {
        let mut cache = ProfileCache::default();
        let start = Instant::now();
        for index in 0..MAX_PROFILES {
            loaded(
                &mut cache,
                &user(index),
                start + Duration::from_micros(index as u64),
            );
        }
        let filled = start + Duration::from_micros(MAX_PROFILES as u64);
        assert_eq!(cache.lookup(&user(MAX_PROFILES), None, true, filled), None);
        assert_eq!(cache.entries.len(), MAX_PROFILES);
        assert_eq!(cache.refused, 1);
        let later = filled + VISIBLE_GRACE + Duration::from_millis(1);
        cache.lookup(&user(0), None, false, later);
        loaded(&mut cache, &user(MAX_PROFILES), later);
        assert!(
            cache.entries.contains_key(&user(0)),
            "currently visible profile must survive"
        );
        assert!(
            !cache.entries.contains_key(&user(1)),
            "oldest inactive entry is evicted"
        );
        assert_eq!(cache.entries.len(), MAX_PROFILES);
        assert!(
            !cache.apply(profile(user(1), "late")),
            "late completion cannot repopulate an evicted entry"
        );
        assert_eq!(cache.entries.len(), MAX_PROFILES);
    }

    #[test]
    fn per_user_and_global_member_budgets_survive_many_room_lookups() {
        let mut cache = ProfileCache::default();
        let now = Instant::now();
        for index in 0..MAX_MEMBERSHIPS / MAX_ROOMS_PER_USER {
            let id = user(index);
            loaded(&mut cache, &id, now);
            for member in 0..MAX_ROOMS_PER_USER {
                assert_eq!(
                    cache.lookup(&id, Some(&room(member)), true, now),
                    Some(Some(room(member)))
                );
                assert!(cache.apply(UserProfileUpdate::RoomMemberFailed {
                    user_id: id.clone(),
                    room_id: room(member)
                }));
            }
        }
        let memberships = |cache: &ProfileCache| {
            cache
                .access
                .keys()
                .filter(|(_, room)| room.is_some())
                .count()
        };
        assert_eq!(memberships(&cache), MAX_MEMBERSHIPS);
        assert_eq!(
            cache.lookup(&user(0), Some(&room(MAX_ROOMS_PER_USER)), true, now),
            None
        );
        let later = now + VISIBLE_GRACE + Duration::from_millis(1);
        assert_eq!(
            cache.lookup(&user(0), Some(&room(MAX_ROOMS_PER_USER)), true, later),
            Some(Some(room(MAX_ROOMS_PER_USER)))
        );
        assert_eq!(memberships(&cache), MAX_MEMBERSHIPS);
        let id = user(MAX_PROFILES);
        loaded(&mut cache, &id, later);
        assert_eq!(
            cache.lookup(&id, Some(&room(0)), true, later),
            Some(Some(room(0)))
        );
        assert_eq!(memberships(&cache), MAX_MEMBERSHIPS);
        for entry in cache.entries.values() {
            if let UserProfileCacheEntry::Loaded { rooms, .. } = entry {
                assert!(rooms.len() <= MAX_ROOMS_PER_USER);
            }
        }
    }

    #[test]
    fn dropped_or_missing_completions_retry_without_reconnect_and_idle_data_expires() {
        let mut cache = ProfileCache::default();
        let now = Instant::now();
        let id = user(0);
        assert_eq!(cache.lookup(&id, None, true, now), Some(None));
        assert_eq!(cache.lookup(&id, None, true, now + REQUEST_TTL / 2), None);
        assert_eq!(cache.lookup(&id, None, true, now + REQUEST_TTL), Some(None));
        cache.retry_dropped(now + REQUEST_TTL + Duration::from_secs(1));
        assert_eq!(
            cache.lookup(&id, None, true, now + REQUEST_TTL + Duration::from_secs(1)),
            Some(None)
        );
        assert!(cache.apply(profile(id.clone(), "loaded")));
        assert_eq!(
            cache.lookup(
                &id,
                None,
                true,
                now + REQUEST_TTL + IDLE_TTL + Duration::from_secs(2)
            ),
            Some(None)
        );
    }

    #[test]
    fn partial_profile_completion_does_not_duplicate_pending_member_request() {
        let mut cache = ProfileCache::default();
        let now = Instant::now();
        let id = user(0);
        let member_room = room(0);
        assert_eq!(
            cache.lookup(&id, Some(&member_room), true, now),
            Some(Some(member_room.clone()))
        );
        assert!(cache.apply(profile(id.clone(), "name")));
        assert_eq!(cache.lookup(&id, Some(&member_room), true, now), None);
        assert_eq!(
            cache.lookup(&id, Some(&member_room), true, now + REQUEST_TTL),
            Some(Some(member_room))
        );
    }

    #[test]
    fn completion_flood_coalesces_and_has_bounded_slots_with_retry_notification() {
        let mut queue = PendingUpdates::default();
        let id = user(0);
        for _ in 0..10_000 {
            queue.push(7, profile(id.clone(), "older"));
        }
        queue.push(7, profile(id, "latest"));
        assert_eq!(queue.order.len(), 1);
        assert_eq!(queue.coalesced, 10_000);
        let Some(UserProfileUpdate::UserProfileOnly(latest)) = queue.pop() else {
            panic!("profile expected");
        };
        assert_eq!(latest.username.as_deref(), Some("latest"));
        for index in 0..MAX_PENDING_UPDATES * 2 {
            queue.push(7, profile(user(index), "loaded"));
        }
        assert_eq!(queue.updates.len(), MAX_PENDING_UPDATES);
        assert_eq!(queue.order.len(), MAX_PENDING_UPDATES);
        assert_eq!(queue.dropped, MAX_PENDING_UPDATES as u64);
        assert!(queue.overflowed);
        queue.push(8, profile(user(0), "new account"));
        assert_eq!(
            queue.order.len(),
            1,
            "account epoch cannot retain old completions"
        );
        assert!(!queue.overflowed);
    }

    #[test]
    fn ui_batch_yields_and_signals_until_the_completion_backlog_is_drained() {
        let mut cache = ProfileCache::default();
        let mut pending = PendingUpdates::default();
        let now = Instant::now();
        for index in 0..UPDATE_BATCH * 3 {
            cache.lookup(&user(index), None, true, now);
            pending.push(0, profile(user(index), "loaded"));
        }
        let (changed, needs_signal) = drain_updates(&mut cache, &mut pending);
        assert!(changed);
        assert!(
            needs_signal,
            "remaining work must request another UI wakeup"
        );
        assert!(
            pending.order.len() >= UPDATE_BATCH * 2,
            "one callback must not drain an unbounded burst"
        );
        for _ in 0..UPDATE_BATCH * 3 {
            drain_updates(&mut cache, &mut pending);
        }
        assert!(pending.order.is_empty());
        assert!(
            cache
                .entries
                .values()
                .all(|entry| matches!(entry, UserProfileCacheEntry::Loaded { .. }))
        );
        assert_eq!(drain_updates(&mut cache, &mut pending), (false, false));
    }
}
