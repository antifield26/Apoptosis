//! Server-console stdin reader (P19-03).
//!
//! Stdin is blocking, so it is read on a dedicated blocking task and each
//! line is handed to the tick loop over a bounded channel — the loop drains
//! at most one line per sleep window, so pasting into the console cannot
//! stall ticks. EOF ends console input and nothing else: the sender drops,
//! the run loop sees a closed channel and clears it, and the server keeps
//! running (pinned by `console_eof_does_not_stop_the_server`).

use std::io::BufRead;

/// Read one console line: the next non-empty line, without its terminator.
///
/// Returns `None` on EOF (clean shell quit, piped input exhausted). Empty
/// lines are skipped — sending them to the dispatcher would only answer
/// "unknown command". A final line without a trailing newline still
/// counts: EOF terminates the line, it does not discard it.
pub fn next_console_line(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            // EOF (clean quit, exhausted pipe) and read errors both end
            // input: a half-read line is not a command.
            Err(_) | Ok(0) => return None,
            Ok(_) => {}
        }
        let text = line.trim_end_matches(['\r', '\n']).trim().to_owned();
        if !text.is_empty() {
            return Some(text);
        }
    }
}

/// Spawn the stdin reader feeding `tx`; EOF drops the sender.
pub fn spawn_console_reader(tx: tokio::sync::mpsc::Sender<String>) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut locked = stdin.lock();
        while let Some(line) = next_console_line(&mut locked) {
            if tx.blocking_send(line).is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::next_console_line;
    use std::io::BufReader;

    #[test]
    fn lines_come_back_trimmed_and_eof_ends() {
        let input = "list\n\n  stop  \r\n";
        let mut reader = BufReader::new(input.as_bytes());
        assert_eq!(next_console_line(&mut reader).as_deref(), Some("list"));
        assert_eq!(next_console_line(&mut reader).as_deref(), Some("stop"));
        assert_eq!(next_console_line(&mut reader), None);
    }

    #[test]
    fn unterminated_final_line_counts_and_empty_input_is_eof() {
        let mut reader = BufReader::new("save-all".as_bytes());
        assert_eq!(next_console_line(&mut reader).as_deref(), Some("save-all"));
        assert_eq!(next_console_line(&mut reader), None);
        let mut empty = BufReader::new("".as_bytes());
        assert_eq!(next_console_line(&mut empty), None);
    }
}
