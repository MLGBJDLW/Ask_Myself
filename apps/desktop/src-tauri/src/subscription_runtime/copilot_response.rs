//! Copilot can split one API response at reasoning boundaries. Message IDs
//! identify blocks; only apiCallId + chunkIndex identify the complete answer.
use super::{protocol_error, CoreError};
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
pub(super) struct Response {
    turn_id: Option<String>,
    attempt: u64,
    group: Option<Group>,
    answer_blocks: HashSet<String>,
}

struct Group {
    key: String,
    count: usize,
    blocks: BTreeMap<usize, String>,
    contains_tools: bool,
}

impl Response {
    pub(super) fn start(&mut self, turn_id: Option<&str>) {
        self.turn_id = turn_id.map(str::to_owned);
        self.attempt += 1;
        self.group = None;
        self.answer_blocks.clear();
    }

    pub(super) fn retry(&mut self, turn_id: &str) -> Result<Vec<String>, CoreError> {
        if self.turn_id.as_deref() != Some(turn_id) {
            return Err(protocol_error(
                "Copilot retry does not belong to the active turn",
            ));
        }
        self.attempt += 1;
        self.group = None;
        Ok(self.answer_blocks.drain().collect())
    }

    pub(super) fn observe_answer(&mut self, id: &str) -> Result<(), CoreError> {
        if !self.answer_blocks.contains(id) && self.answer_blocks.len() >= 2048 {
            return Err(protocol_error(
                "Copilot attempt exceeded its answer block budget",
            ));
        }
        self.answer_blocks.insert(id.to_string());
        Ok(())
    }

    pub(super) fn tool_started(&mut self) {
        if let Some(group) = &mut self.group {
            group.contains_tools = true;
        }
    }

    pub(super) fn ensure_complete(&self) -> Result<(), CoreError> {
        if self
            .group
            .as_ref()
            .is_some_and(|group| group.blocks.len() != group.count)
        {
            return Err(protocol_error("Copilot response is missing message chunks"));
        }
        Ok(())
    }

    pub(super) fn accept(
        &mut self,
        data: &serde_json::Value,
    ) -> Result<Option<Vec<String>>, CoreError> {
        let id = data
            .get("messageId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| protocol_error("Copilot message has no messageId"))?;
        self.observe_answer(id)?;
        let (index, count) = match (data.get("chunkIndex"), data.get("chunkCount")) {
            (None | Some(serde_json::Value::Null), None | Some(serde_json::Value::Null)) => (0, 1),
            (Some(index), Some(count)) => {
                let index = index
                    .as_u64()
                    .ok_or_else(|| protocol_error("invalid Copilot chunkIndex"))?;
                let count = count
                    .as_u64()
                    .ok_or_else(|| protocol_error("invalid Copilot chunkCount"))?;
                if count == 0 || count > 2048 || index >= count {
                    return Err(protocol_error("Copilot response exceeds its chunk bounds"));
                }
                (index as usize, count as usize)
            }
            _ => {
                return Err(protocol_error(
                    "Copilot message has incomplete chunk metadata",
                ))
            }
        };
        let key = if let Some(api) = data
            .get("apiCallId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
        {
            format!("api:{api}")
        } else if count == 1 {
            format!("message:{id}")
        } else {
            let turn = self.turn_id.as_ref().ok_or_else(|| {
                protocol_error("Copilot split response has no API or turn identity")
            })?;
            format!("turn:{turn}:{}", self.attempt)
        };
        if self.group.as_ref().is_none_or(|group| group.key != key) {
            self.ensure_complete()?;
            self.group = Some(Group {
                key,
                count,
                blocks: BTreeMap::new(),
                contains_tools: false,
            });
        }
        let group = self.group.as_mut().expect("response group initialized");
        if group.count != count
            || group
                .blocks
                .get(&index)
                .is_some_and(|previous| previous != id)
            || group
                .blocks
                .iter()
                .any(|(previous_index, previous_id)| *previous_index != index && previous_id == id)
        {
            return Err(protocol_error(
                "conflicting Copilot response chunk identity",
            ));
        }
        group.blocks.insert(index, id.to_string());
        group.contains_tools |= data
            .get("toolRequests")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|calls| !calls.is_empty());
        Ok((!group.contains_tools && group.blocks.len() == count)
            .then(|| group.blocks.values().cloned().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn retry_must_match_the_active_turn_before_abandoning_its_blocks() {
        let mut response = Response::default();
        response.start(Some("current"));
        response.observe_answer("live").unwrap();
        assert!(response.retry("old").is_err());
        assert_eq!(response.retry("current").unwrap(), vec!["live"]);
        assert!(response.retry("current").unwrap().is_empty());
    }

    #[test]
    fn malformed_chunk_identity_is_rejected_without_guessing() {
        for malformed in [
            json!({"chunkIndex":0}),
            json!({"chunkIndex":0,"chunkCount":0}),
            json!({"chunkIndex":2,"chunkCount":2}),
            json!({"chunkIndex":0,"chunkCount":2049}),
            json!({"chunkIndex":0,"chunkCount":2}),
        ] {
            let mut data = malformed;
            data["messageId"] = json!("m");
            assert!(Response::default().accept(&data).is_err());
        }
        for (id, index, count) in [("conflict", 0, 2), ("a", 1, 2), ("b", 1, 3)] {
            let mut response = Response::default();
            response
                .accept(&json!({"apiCallId":"api","messageId":"a","chunkIndex":0,"chunkCount":2}))
                .unwrap();
            assert!(response
                .accept(
                    &json!({"apiCallId":"api","messageId":id,"chunkIndex":index,"chunkCount":count})
                )
                .is_err());
        }
    }
}
