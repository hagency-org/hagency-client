//! The model's questions to its room (Codex `request_user_input`). Each
//! request becomes one card in the run's thread, drawn under the message that
//! started the run, and any joined member may answer it. All questions of one
//! run share one wait of `ANSWER_WAIT_MS`, so the messages queued behind the
//! run wait at most that long for answers; the activity lease is renewed as
//! usual meanwhile (the wait itself never extends it). Without an answer the
//! model decides on its own. Answers are room events, never chat messages, so
//! an answer never queues behind the run. Question and answer text are never
//! logged.
use super::*;
use crate::agent_questions::{self as protocol, Card, CardState};
use crate::console::owned_runtime::e2ee::AgentDevice;
use hagency_runtime::{
    codex::{
        RequestId,
        session::{UserInputRequest, UserInputResponse},
    },
    owned::OwnedSession,
};
use serde_json::{Value, json};

/// One run asks at most this many times; later questions are answered with
/// nothing, so a model that keeps asking cannot hold the queue for hours.
const MAX_CARDS_PER_TURN: usize = 6;
/// Posting a card or its outcome. A first message in a large encrypted room
/// shares its room key with every member's devices first.
const POST_TIMEOUT: Duration = Duration::from_secs(30);
/// Reading the answers that reached the device, with membership checks. The
/// device bounds each of its own steps well inside this.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// A member's answer event as the Matrix boundary hands it over: from a
/// current member, authenticated where the room is encrypted. Content is
/// still unchecked.
pub(super) struct Candidate {
    pub(super) sender: String,
    pub(super) content: Value,
}

pub(super) trait QuestionIo {
    fn post(
        &self,
        room: &str,
        txn: &str,
        content: Value,
    ) -> impl std::future::Future<Output = Result<String, NativeError>> + Send;
    fn answers(
        &self,
        room: &str,
        card_event: &str,
    ) -> impl std::future::Future<Output = Result<Vec<Candidate>, NativeError>> + Send;
}
impl QuestionIo for AgentDevice {
    async fn post(&self, room: &str, txn: &str, content: Value) -> Result<String, NativeError> {
        self.send_room_message(room, txn, content)
            .await
            .map_err(|_| failure(503, "shared_question_send_pending"))
    }
    async fn answers(&self, room: &str, card_event: &str) -> Result<Vec<Candidate>, NativeError> {
        Ok(self
            .question_answers(room, card_event)
            .await
            .map_err(|_| failure(503, "shared_question_answers_unavailable"))?
            .into_iter()
            .map(|r| Candidate { sender: r.sender, content: r.content })
            .collect())
    }
}

pub(super) enum Source {
    Device(Arc<AgentDevice>),
    #[cfg(test)]
    Fixture(Arc<Fixture>),
}
impl Source {
    fn revoked(&self) -> bool {
        match self {
            Self::Device(device) => device.is_revoked(),
            #[cfg(test)]
            Self::Fixture(_) => false,
        }
    }
}
impl QuestionIo for Source {
    async fn post(&self, room: &str, txn: &str, content: Value) -> Result<String, NativeError> {
        match self {
            Self::Device(device) => QuestionIo::post(device.as_ref(), room, txn, content).await,
            #[cfg(test)]
            Self::Fixture(fixture) => fixture.post(room, txn, content).await,
        }
    }
    async fn answers(&self, room: &str, card_event: &str) -> Result<Vec<Candidate>, NativeError> {
        match self {
            Self::Device(device) => QuestionIo::answers(device.as_ref(), room, card_event).await,
            #[cfg(test)]
            Self::Fixture(fixture) => fixture.answers(room, card_event).await,
        }
    }
}

struct Pending {
    request: UserInputRequest,
    card: Card,
    room: String,
    card_event: String,
    deadline: tokio::time::Instant,
}

/// The whole time one run may hold its Agent's queue waiting for answers.
/// Every question in the turn shares it, so the messages behind the run wait
/// at most this long for questions in total, however many the model asks.
struct WaitBudget {
    total: Duration,
    used: Duration,
    /// Since when at least one card has been waiting.
    since: Option<tokio::time::Instant>,
}
impl WaitBudget {
    fn new(total: Duration) -> Self {
        Self { total, used: Duration::ZERO, since: None }
    }
    /// How long a card asked at `now` may wait, or `None` when too little of
    /// the budget is left for anyone to answer in.
    fn next(&self, now: tokio::time::Instant) -> Option<Duration> {
        let waiting = self.since.map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        let left = self.total.saturating_sub(self.used).saturating_sub(waiting);
        (!left.is_zero() && left >= self.total / 20).then_some(left)
    }
    /// A card started waiting at `now`.
    fn start(&mut self, now: tokio::time::Instant) {
        self.since.get_or_insert(now);
    }
    /// No card is waiting any more.
    fn stop(&mut self, now: tokio::time::Instant) {
        if let Some(since) = self.since.take() {
            self.used = self.used.saturating_add(now.saturating_duration_since(since));
        }
    }
}

#[derive(Default)]
pub(super) struct Questions {
    source: Option<Arc<Source>>,
    pending: Vec<Pending>,
    /// Settled cards whose outcome the room has not been shown yet.
    settled: Vec<(String, String, Card)>,
    asked: usize,
    /// The assignment whose Agent device posted the cards.
    assignment: Option<String>,
    budget: Option<WaitBudget>,
}

impl Questions {
    /// Work for the next maintenance pass: waiting cards or outcomes to show.
    pub(super) fn busy(&self) -> bool {
        !self.pending.is_empty() || !self.settled.is_empty()
    }

    /// Show one question request to the room. `Some`: nothing could be asked
    /// (only secret questions, more than a card holds, too many questions, or
    /// the room is unreachable), so the model gets no answers at once and
    /// decides on its own. Never fails the run.
    pub(super) async fn ask(
        &mut self,
        io: &Io,
        work: &SharedWorkItem,
        request: UserInputRequest,
    ) -> Result<Option<UserInputResponse>, NativeError> {
        self.asked += 1;
        let budget = self.budget.get_or_insert_with(|| WaitBudget::new(wait(io)));
        let wait = budget.next(tokio::time::Instant::now());
        let id = format!(
            "aq_{}",
            super::digest(&json!({"work":work.id,"request":request.id(),"item":request.item_id()}))?
        );
        let expires_at_ms = now_ms().saturating_add(wait.unwrap_or_default().as_millis() as u64);
        let card = (self.asked <= MAX_CARDS_PER_TURN && wait.is_some())
            .then(|| {
                Card::ask(
                    id,
                    work.event_id.clone(),
                    work.thread_root.clone(),
                    expires_at_ms,
                    request.questions(),
                )
            })
            .flatten();
        let (Some(card), Some(wait)) = (card, wait) else {
            tracing::info!(
                questions = request.questions().len(),
                asked = self.asked,
                "Agent question cannot be shown in the room; the model decides on its own"
            );
            return Ok(Some(request.unanswered()));
        };
        // Opening the device is not cut short: an abandoned grant would cost
        // another device login. The caller renews the run's lease meanwhile.
        let source = match self.source(io, work).await {
            Ok(source) => source,
            Err(error) => {
                tracing::warn!(code = %error.code, "Agent question has no room device; the model decides on its own");
                return Ok(Some(request.unanswered()));
            }
        };
        let txn = format!("hagency_question_{}", &card.id[3..]);
        match tokio::time::timeout(POST_TIMEOUT, source.post(&work.room_id, &txn, card.message_content())).await {
            Ok(Ok(card_event)) => {
                tracing::info!(
                    questions = card.questions.len(),
                    withheld = card.withheld,
                    wait_secs = wait.as_secs(),
                    "Agent question card posted; waiting for an answer from the room"
                );
                let now = tokio::time::Instant::now();
                self.pending.push(Pending {
                    request,
                    card,
                    room: work.room_id.clone(),
                    card_event,
                    deadline: now + wait,
                });
                if let Some(budget) = &mut self.budget {
                    budget.start(now);
                }
                Ok(None)
            }
            _ => {
                tracing::warn!("Agent question card could not be posted; the model decides on its own");
                Ok(Some(request.unanswered()))
            }
        }
    }

    /// Answers that arrived and waits that ran out, as the responses to send
    /// now. The first answer that is valid for its card wins; later answers
    /// to a settled card are never read.
    pub(super) async fn collect(&mut self, io: &Io) -> Result<Vec<UserInputResponse>, NativeError> {
        if self.source.as_ref().is_some_and(|s| s.revoked()) {
            // The device's token was replaced and its sync stopped. A fresh
            // device's first sync brings the recent answers again.
            self.source = None;
        }
        if self.source.is_none()
            && !self.pending.is_empty()
            && let Some(assignment) = self.assignment.clone()
            && let Err(error) = self.reopen(io, &assignment).await
        {
            tracing::warn!(code = %error.code, "Agent question room device unavailable; still waiting");
        }
        let mut ready = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            let pending = &self.pending[index];
            let mut answered = None;
            if let Some(source) = &self.source {
                let candidates = tokio::time::timeout(
                    READ_TIMEOUT,
                    source.answers(&pending.room, &pending.card_event),
                )
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
                for candidate in candidates {
                    let Some(answers) = pending.card.parse_answer(&pending.card_event, &candidate.content)
                    else {
                        continue;
                    };
                    let runtime = answers
                        .iter()
                        .map(|(id, value)| (id.clone(), value.clone().into_runtime()))
                        .collect();
                    if let Ok(response) = pending.request.answer(&runtime) {
                        answered = Some((candidate.sender, answers, response));
                        break;
                    }
                }
            }
            if let Some((by, answers, response)) = answered {
                let pending = self.pending.remove(index);
                tracing::info!(answered = answers.len(), "Agent question answered by a room member");
                self.settled.push((pending.room, pending.card_event, pending.card.answered(&by, answers)));
                ready.push(response);
                continue;
            }
            if tokio::time::Instant::now() >= pending.deadline {
                let pending = self.pending.remove(index);
                tracing::info!("Agent question expired without an answer; the model decides on its own");
                self.settled.push((pending.room, pending.card_event, pending.card.expired()));
                ready.push(pending.request.unanswered());
                continue;
            }
            index += 1;
        }
        self.idle_check();
        Ok(ready)
    }

    /// Stop the budget's clock once no card waits.
    fn idle_check(&mut self) {
        if self.pending.is_empty()
            && let Some(budget) = &mut self.budget
        {
            budget.stop(tokio::time::Instant::now());
        }
    }

    /// Codex withdrew a question: stop waiting and close its card.
    pub(super) fn withdrawn(&mut self, id: &RequestId) {
        if let Some(index) = self.pending.iter().position(|p| p.request.id() == id) {
            let pending = self.pending.remove(index);
            tracing::info!("Agent question withdrawn by the model before an answer");
            self.settled.push((pending.room, pending.card_event, pending.card.expired()));
        }
        self.idle_check();
    }

    /// The turn is over: every card still waiting closes unanswered.
    pub(super) fn close(&mut self) {
        for pending in std::mem::take(&mut self.pending) {
            self.settled.push((pending.room, pending.card_event, pending.card.expired()));
        }
        self.idle_check();
    }

    /// Show each settled card's outcome in the room. Best effort: a card that
    /// cannot be edited still reads as expired in every Hagency client once
    /// its time is up, and the run never waits on it.
    pub(super) async fn publish(&mut self) -> Result<(), NativeError> {
        let settled = std::mem::take(&mut self.settled);
        let Some(source) = self.source.clone() else {
            return Ok(());
        };
        for (room, card_event, card) in settled {
            let state = if card.state == CardState::Answered { "answered" } else { "expired" };
            let txn = format!("hagency_question_{state}_{}", &card.id[3..]);
            let edited = tokio::time::timeout(
                POST_TIMEOUT,
                source.post(&room, &txn, card.edit_content(&card_event)),
            )
            .await;
            if !matches!(edited, Ok(Ok(_))) {
                tracing::warn!(state, "Agent question outcome could not be shown in the room");
            }
        }
        Ok(())
    }

    async fn source(&mut self, io: &Io, work: &SharedWorkItem) -> Result<Arc<Source>, NativeError> {
        self.assignment = Some(work.assignment_id.clone());
        if let Some(source) = self.source.as_ref().filter(|s| !s.revoked()) {
            return Ok(source.clone());
        }
        self.reopen(io, &work.assignment_id).await?;
        self.source.clone().ok_or_else(invalid)
    }

    async fn reopen(&mut self, io: &Io, assignment: &str) -> Result<(), NativeError> {
        #[cfg(test)]
        if let Some(fixture) = io.crypto_fixture.as_ref().and_then(|f| f.questions.clone()) {
            self.source = Some(Arc::new(Source::Fixture(fixture)));
            return Ok(());
        }
        let b = io.bound().await?;
        let device = super::e2ee::device(io, &b, assignment).await?;
        self.source = Some(Arc::new(Source::Device(device)));
        Ok(())
    }
}

/// Before the thread starts. Every shared turn may ask its room, so Codex
/// offers the model its question tool. Never fails the run: a thread without
/// it tells the model the tool is for Plan mode only.
pub(super) fn offer(process: &mut OwnedSession) {
    if let Err(error) = process.offer_shared_user_input_tool() {
        tracing::warn!(?error, "Agent question tool not offered; the model decides on its own");
    }
}

/// Every shared turn may ask its room. Without this (a session that is not
/// running), the session itself answers each question with nothing.
pub(super) fn enable(process: &mut OwnedSession) -> Result<(), NativeError> {
    if let Err(error) = process.enable_shared_user_input() {
        tracing::warn!(?error, "Agent questions unavailable for this turn; the model decides on its own");
    }
    Ok(())
}

/// One answer to Codex. A question Codex withdrew meanwhile is moot.
pub(super) async fn answer(
    process: &mut OwnedSession,
    response: UserInputResponse,
) -> Result<(), NativeError> {
    process
        .respond_shared_user_input(response)
        .await
        .map(|_| ())
        .map_err(|_| failure(409, "shared_question_answer_failed"))
}

fn wait(io: &Io) -> Duration {
    #[cfg(test)]
    if let Some(fixture) = io.crypto_fixture.as_ref().and_then(|f| f.questions.as_ref()) {
        return fixture.wait;
    }
    let _ = io;
    Duration::from_millis(protocol::ANSWER_WAIT_MS)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn questions_in_one_turn_share_one_wait() {
        let start = tokio::time::Instant::now();
        let minutes = |n: u64| Duration::from_secs(60 * n);
        let mut budget = WaitBudget::new(minutes(10));
        assert_eq!(budget.next(start), Some(minutes(10)));
        budget.start(start);
        // A second question while the first waits gets only what is left.
        assert_eq!(budget.next(start + minutes(3)), Some(minutes(7)));
        budget.stop(start + minutes(4));
        assert_eq!(budget.next(start + minutes(20)), Some(minutes(6)), "idle time is free");
        budget.start(start + minutes(20));
        budget.stop(start + minutes(25));
        assert_eq!(budget.next(start + minutes(30)), Some(minutes(1)));
        budget.start(start + minutes(30));
        budget.stop(start + minutes(30) + Duration::from_secs(40));
        // Twenty seconds left of ten minutes is too little to ask anyone.
        assert_eq!(budget.next(start + minutes(31)), None);
    }
}

/// The supervised driver tests substitute only the Matrix boundary: posting a
/// card or its edit, and the answers that reached the Agent's device from
/// current members. Card building, answer checks, the wait, the Codex
/// response and the lease handling stay real.
#[cfg(test)]
pub(in crate::console::native) struct Fixture {
    posts: std::sync::Mutex<Vec<(String, String, Value)>>,
    answers: std::sync::Mutex<Vec<(String, Value)>>,
    members: Vec<String>,
    pub(super) wait: Duration,
    pub(in crate::console::native) unreachable: std::sync::atomic::AtomicBool,
}
#[cfg(test)]
impl Fixture {
    pub(super) fn new(members: &[&str], wait: Duration) -> Self {
        Self {
            posts: Default::default(),
            answers: Default::default(),
            members: members.iter().map(|m| (*m).to_owned()).collect(),
            wait,
            unreachable: Default::default(),
        }
    }
    /// The posted card messages (not their edits), as the room received them.
    pub(in crate::console::native) fn cards(&self) -> Vec<(String, Value)> {
        self.posts
            .lock()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, (_, _, content))| content["m.relates_to"]["rel_type"] != "m.replace")
            .map(|(index, (_, _, content))| (format!("$question_{index}:test"), content.clone()))
            .collect()
    }
    pub(in crate::console::native) fn edits(&self) -> Vec<Value> {
        self.posts
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, content)| content["m.relates_to"]["rel_type"] == "m.replace")
            .map(|(_, _, content)| content.clone())
            .collect()
    }
    pub(in crate::console::native) fn answer(&self, sender: &str, content: Value) {
        self.answers.lock().unwrap().push((sender.to_owned(), content));
    }
}
#[cfg(test)]
impl QuestionIo for Fixture {
    async fn post(&self, room: &str, txn: &str, content: Value) -> Result<String, NativeError> {
        if self.unreachable.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(failure(503, "shared_question_send_pending"));
        }
        let mut posts = self.posts.lock().unwrap();
        // A repeated transaction returns the original event, as a homeserver does.
        if let Some(index) = posts.iter().position(|(_, t, _)| t == txn) {
            return Ok(format!("$question_{index}:test"));
        }
        posts.push((room.to_owned(), txn.to_owned(), content));
        Ok(format!("$question_{}:test", posts.len() - 1))
    }
    async fn answers(&self, _room: &str, card_event: &str) -> Result<Vec<Candidate>, NativeError> {
        let mut answers = self.answers.lock().unwrap();
        let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut *answers)
            .into_iter()
            .partition(|(_, content)| content["m.relates_to"]["event_id"] == card_event);
        *answers = others;
        Ok(mine
            .into_iter()
            .filter(|(sender, _)| self.members.contains(sender))
            .map(|(sender, content)| Candidate { sender, content })
            .collect())
    }
}
