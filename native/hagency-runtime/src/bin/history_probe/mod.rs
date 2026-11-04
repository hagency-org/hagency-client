//! Offline provider peer for the complete native room-history execution path.
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

#[derive(Default)]
pub(super) struct Probe {
    index: usize,
    pending: bool,
}
impl Probe {
    pub(super) fn begin(&mut self) -> io::Result<()> {
        self.request(Value::Null)
    }
    fn request(&mut self, cursor: Value) -> io::Result<()> {
        self.pending = true;
        println!(
            "{}",
            json!({"method":"item/started","params":{"threadId":"text-thread","turnId":"text-turn","startedAtMs":1,"item":{"id":format!("history-item-{}",self.index),"type":"dynamicToolCall","tool":"read_room_history","arguments":{"cursor":cursor},"status":"inProgress"}}})
        );
        println!(
            "{}",
            json!({"id":format!("history-{}",self.index),"method":"item/tool/call","params":{"threadId":"text-thread","turnId":"text-turn","callId":format!("history-call-{}",self.index),"tool":"read_room_history","arguments":{"cursor":cursor}}})
        );
        io::stdout().flush()
    }
    pub(super) fn response(&mut self, root: &Path, response: &Value) -> io::Result<()> {
        if !self.pending
            || response["id"] != format!("history-{}", self.index)
            || response["result"]["success"] != true
        {
            return Err(io::Error::other("unexpected history response"));
        }
        let text = response["result"]["contentItems"][0]["text"]
            .as_str()
            .ok_or_else(|| io::Error::other("history result missing"))?;
        if text.len() > 12 * 1024 {
            return Err(io::Error::other("history result exceeded its bound"));
        }
        let page: Value = serde_json::from_str(text)?;
        let mut pages = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("history-pages.jsonl"))?;
        writeln!(pages, "{page}")?;
        println!(
            "{}",
            json!({"method":"item/completed","params":{"threadId":"text-thread","turnId":"text-turn","completedAtMs":2,"item":{"id":format!("history-item-{}",self.index),"type":"dynamicToolCall","tool":"read_room_history","arguments":{"cursor":null},"status":"completed"}}})
        );
        self.pending = false;
        self.index += 1;
        if page["complete"] != true {
            if self.index >= 8 || !page["nextCursor"].is_string() {
                return Err(io::Error::other("history did not complete"));
            }
            return self.request(page["nextCursor"].clone());
        }
        let answer = page["messages"]
            .as_array()
            .and_then(|messages| messages.iter().find_map(|message| message["body"].as_str()))
            .ok_or_else(|| io::Error::other("earlier history body missing"))?;
        let usage = json!({"totalTokens":300,"inputTokens":200,"cachedInputTokens":100,"cacheWriteInputTokens":20,"outputTokens":100,"reasoningOutputTokens":0});
        println!(
            "{}",
            json!({"method":"thread/tokenUsage/updated","params":{"threadId":"text-thread","turnId":"text-turn","tokenUsage":{"total":usage,"last":usage,"modelContextWindow":131072}}})
        );
        println!(
            "{}",
            json!({"method":"item/started","params":{"threadId":"text-thread","turnId":"text-turn","startedAtMs":3,"item":{"type":"agentMessage","id":"answer","text":"","phase":"final_answer"}}})
        );
        println!(
            "{}",
            json!({"method":"item/agentMessage/delta","params":{"threadId":"text-thread","turnId":"text-turn","itemId":"answer","delta":answer}})
        );
        println!(
            "{}",
            json!({"method":"item/completed","params":{"threadId":"text-thread","turnId":"text-turn","completedAtMs":4,"item":{"type":"agentMessage","id":"answer","text":answer,"phase":"final_answer"}}})
        );
        println!(
            "{}",
            json!({"method":"turn/completed","params":{"threadId":"text-thread","turn":{"id":"text-turn","status":"completed","items":[]}}})
        );
        io::stdout().flush()
    }
}
