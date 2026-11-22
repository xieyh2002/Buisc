use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use serde_json::Value;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lyrics {
    segments: Vec<LyricSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LyricSegment {
    start_ms: u64,
    end_ms: u64,
    text: Arc<str>,
}

impl Lyrics {
    pub(crate) fn segments(&self) -> &[LyricSegment] {
        &self.segments
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn line_at(&self, position_ms: u64) -> Option<&str> {
        let index = self
            .segments
            .partition_point(|segment| segment.start_ms <= position_ms)
            .checked_sub(1)?;
        let segment = &self.segments[index];
        (position_ms < segment.end_ms).then_some(segment.text.as_ref())
    }

    pub fn active_index(&self, position_ms: u64) -> Option<usize> {
        let index = self
            .segments
            .partition_point(|segment| segment.start_ms <= position_ms)
            .checked_sub(1)?;
        (position_ms < self.segments[index].end_ms).then_some(index)
    }
}

impl LyricSegment {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

impl Lyrics {
    pub(crate) fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        let payload: Value =
            serde_json::from_slice(bytes).map_err(|_| "B 站字幕数据格式不正确".to_owned())?;
        let entries = match &payload {
            Value::Array(entries) => entries.as_slice(),
            Value::Object(object) => object
                .get("body")
                .or_else(|| object.get("result"))
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .ok_or_else(|| "B 站字幕数据缺少正文".to_owned())?,
            _ => return Err("B 站字幕数据格式不正确".to_owned()),
        };

        let lines = entries
            .iter()
            .filter_map(lyric_line_from_value)
            .collect::<Vec<_>>();
        Ok(Self {
            segments: build_timeline(lines),
        })
    }
}

fn lyric_line_from_value(value: &Value) -> Option<LyricSegment> {
    let object = value.as_object()?;
    let start_ms = seconds_to_millis(object.get("from"))?;
    let end_ms = seconds_to_millis(object.get("to"))?;
    if end_ms <= start_ms {
        return None;
    }
    let text = normalize_lyric_text(
        object
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    if text.is_empty() {
        return None;
    }
    Some(LyricSegment {
        start_ms,
        end_ms,
        text: text.into(),
    })
}

fn build_timeline(mut lines: Vec<LyricSegment>) -> Vec<LyricSegment> {
    if lines.is_empty() {
        return Vec::new();
    }
    lines.sort_by(|left, right| {
        left.start_ms
            .cmp(&right.start_ms)
            .then_with(|| left.end_ms.cmp(&right.end_ms))
            .then_with(|| left.text.cmp(&right.text))
    });

    let mut events = Vec::with_capacity(lines.len() * 2);
    for (index, line) in lines.iter().enumerate() {
        events.push((line.start_ms, true, index));
        events.push((line.end_ms, false, index));
    }
    events.sort_by_key(|(time, _, _)| *time);

    let mut active = vec![false; lines.len()];
    let mut heap: BinaryHeap<(u64, Reverse<u64>, Reverse<usize>)> = BinaryHeap::new();
    let mut segments: Vec<LyricSegment> = Vec::new();
    let mut event_index = 0;
    let mut cursor = events[0].0;
    // At each boundary, the most recently started active line takes precedence.
    while event_index < events.len() {
        let time = events[event_index].0;
        if time > cursor {
            while heap
                .peek()
                .is_some_and(|(_, _, Reverse(index))| !active[*index])
            {
                heap.pop();
            }
            if let Some((_, _, Reverse(index))) = heap.peek() {
                let text = Arc::clone(&lines[*index].text);
                if let Some(previous) = segments.last_mut()
                    && previous.end_ms == cursor
                    && previous.text == text
                {
                    previous.end_ms = time;
                } else {
                    segments.push(LyricSegment {
                        start_ms: cursor,
                        end_ms: time,
                        text,
                    });
                }
            }
        }

        while event_index < events.len() && events[event_index].0 == time {
            let (_, starts, index) = events[event_index];
            active[index] = starts;
            if starts {
                heap.push((
                    lines[index].start_ms,
                    Reverse(lines[index].end_ms),
                    Reverse(index),
                ));
            }
            event_index += 1;
        }
        cursor = time;
    }
    segments
}

fn seconds_to_millis(value: Option<&Value>) -> Option<u64> {
    let seconds = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let milliseconds = (seconds * 1000.0).round();
    (milliseconds <= u64::MAX as f64).then_some(milliseconds as u64)
}

fn normalize_lyric_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sorts_and_selects_overlapping_lines() {
        let lyrics = Lyrics::from_json(
            br#"{
                "body": [
                    {"from": 2.0, "to": 6.0, "content": " later "},
                    {"from": 0, "to": 3.5, "content": "first"},
                    {"from": "1.25", "to": "2.75", "content": "middle\nline"},
                    {"from": 4.0, "to": 4.0, "content": "ignored"},
                    {"from": -1.0, "to": 2.0, "content": "negative"}
                ]
            }"#,
        )
        .expect("parse lyrics");

        assert!(!lyrics.is_empty());
        assert_eq!(lyrics.line_at(0), Some("first"));
        assert_eq!(lyrics.line_at(1_249), Some("first"));
        assert_eq!(lyrics.line_at(1_250), Some("middle line"));
        assert_eq!(lyrics.line_at(2_000), Some("later"));
        assert_eq!(lyrics.line_at(3_499), Some("later"));
        assert_eq!(lyrics.line_at(3_500), Some("later"));
        assert_eq!(lyrics.line_at(6_000), None);
    }

    #[test]
    fn accepts_result_array_and_empty_body() {
        let result =
            Lyrics::from_json(br#"{"result":[{"from":1,"to":2,"content":"result body"}]}"#)
                .expect("parse result array");
        assert_eq!(result.line_at(1_500), Some("result body"));

        let empty = Lyrics::from_json(br#"{"body":[]}"#).expect("parse empty lyrics");
        assert!(empty.is_empty());
        assert_eq!(Lyrics::default().line_at(0), None);
    }

    #[test]
    fn overlapping_lines_fall_back_to_the_previous_active_line() {
        let lyrics = Lyrics::from_json(
            br#"{
                "body": [
                    {"from": 0, "to": 10, "content": "base"},
                    {"from": 2, "to": 4, "content": "short"},
                    {"from": 6, "to": 8, "content": "later"}
                ]
            }"#,
        )
        .expect("parse overlapping lyrics");

        assert_eq!(lyrics.line_at(1_999), Some("base"));
        assert_eq!(lyrics.line_at(2_000), Some("short"));
        assert_eq!(lyrics.line_at(4_000), Some("base"));
        assert_eq!(lyrics.line_at(6_000), Some("later"));
        assert_eq!(lyrics.line_at(8_000), Some("base"));
        assert_eq!(lyrics.line_at(10_000), None);
    }

    #[test]
    fn rejects_malformed_subtitle_documents() {
        for document in [
            br#"{"body":"not an array"}"#.as_slice(),
            br#"{"font_size":0.4}"#.as_slice(),
            br#"not json"#.as_slice(),
        ] {
            assert!(Lyrics::from_json(document).is_err());
        }
    }

    #[test]
    fn lyrics_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Lyrics>();
    }

    #[test]
    fn active_index_tracks_lyric_boundaries() {
        let lyrics = Lyrics::from_json(
            br#"{"body":[
                {"from":0,"to":1,"content":"one"},
                {"from":1,"to":2,"content":"two"}
            ]}"#,
        )
        .expect("parse lyrics");
        assert_eq!(lyrics.segments().len(), 2);
        assert_eq!(lyrics.segments()[1].text(), "two");
        assert_eq!(lyrics.active_index(0), Some(0));
        assert_eq!(lyrics.active_index(999), Some(0));
        assert_eq!(lyrics.active_index(1_000), Some(1));
        assert_eq!(lyrics.active_index(2_000), None);
    }
}
