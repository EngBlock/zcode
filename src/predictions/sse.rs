//! Server-sent events (the Claude API's streaming format), read a line at
//! a time: `event:` names the event, `data:` lines carry it, a blank line
//! ends it, and lines starting with `:` are comments.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Event {
    /// `message` when the server names none.
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub struct Parser {
    event: Option<String>,
    data: Option<String>,
}

impl Parser {
    /// Takes one line (without its line break); returns the event it ends.
    pub fn line(&mut self, line: &str) -> Option<Event> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            let data = self.data.take();
            let event = self.event.take();
            return data.map(|data| Event { event: event.unwrap_or_else(|| "message".into()), data });
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => match &mut self.data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => self.data = Some(value.to_string()),
            },
            _ => {}
        }
        None
    }

    /// The event the stream ended in the middle of, if any.
    pub fn finish(&mut self) -> Option<Event> {
        self.line("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_end_at_blank_lines() {
        let mut parser = Parser::default();
        let stream = "event: message_start\ndata: {\"a\":1}\n\n: ping\nevent: content_block_delta\r\ndata: one\ndata: two\r\n\r\ndata: bare\n";
        let mut events: Vec<Event> = stream.split('\n').filter_map(|line| parser.line(line)).collect();
        events.extend(parser.finish());
        assert_eq!(
            events,
            [
                Event { event: "message_start".into(), data: "{\"a\":1}".into() },
                Event { event: "content_block_delta".into(), data: "one\ntwo".into() },
                Event { event: "message".into(), data: "bare".into() },
            ]
        );
    }
}
