//! Server-console stdin reader (P19-03).
//!
//! Stdin is blocking, so it is read on a dedicated blocking task and each
//! line is handed to the tick loop over a bounded channel — the loop drains
//! at most one line per sleep window, so pasting into the console cannot
//! stall ticks. EOF ends console input and nothing else: the sender drops,
//! the run loop sees a closed channel and clears it, and the server keeps
//! running (pinned by `console_eof_does_not_stop_the_server`).
//!
//! One bad byte is not EOF (AUDIT-19 C19-M4): the dev console is cp936/GBK, so
//! a line can arrive as bytes that are not UTF-8, and the old reader folded
//! that into "end of input" — the reader task returned, the channel closed and
//! the console was dead for the rest of the process's life, with no log line.
//! Such a line is logged and skipped; only EOF or a real read error ends input.

use std::io::BufRead;

/// Read one console line: the next non-empty, UTF-8 line, without its
/// terminator.
///
/// Returns `None` on EOF (clean shell quit, piped input exhausted). Empty
/// lines are skipped — sending them to the dispatcher would only answer
/// "unknown command". A final line without a trailing newline still
/// counts: EOF terminates the line, it does not discard it.
///
/// A line whose bytes are not UTF-8 is skipped after a warning, and reading
/// continues with the next line (AUDIT-19 C19-M4). Only EOF and a genuine read
/// error end input.
pub fn next_console_line(reader: &mut impl BufRead) -> Option<String> {
    let mut raw = Vec::new();
    loop {
        raw.clear();
        // `read_until` on bytes, not `read_line` on a `String`: the byte-level
        // call reports "this line is not UTF-8" as a line we can skip, while
        // `read_line` reports it as `InvalidData` — indistinguishable from a
        // fatal error once it reaches a `match`.
        match reader.read_until(b'\n', &mut raw) {
            // EOF: a clean shell quit or an exhausted pipe.
            Ok(0) => return None,
            Ok(_) => {}
            Err(error) => {
                // A real read error still ends input, but not silently: the
                // old bare `return None` made a dead console look exactly like
                // a clean quit.
                tracing::warn!(%error, "console read failed; ending console input");
                return None;
            }
        }
        let line = match std::str::from_utf8(&raw) {
            Ok(line) => line,
            Err(error) => {
                // The line was consumed by `read_until`, so the next iteration
                // reads the following one; this cannot spin on the same bytes.
                tracing::warn!(
                    valid_up_to = error.valid_up_to(),
                    "console line is not UTF-8 (a GBK/cp936 byte?); skipping it"
                );
                continue;
            }
        };
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

    #[test]
    fn invalid_utf8_line_is_skipped_and_the_console_survives() {
        // AUDIT-19 C19-M4: a cp936/GBK console hands us bytes that are not
        // UTF-8 (asserted here with a byte slice — typing cannot produce
        // them). That is a bad *line*, not the end of input: before this fix
        // the reader task returned, the channel closed and the server ran its
        // whole life without a console.
        let input: &[u8] = b"list\n\xff\xfe save-all\nstop\n";
        let mut reader = BufReader::new(input);
        assert_eq!(next_console_line(&mut reader).as_deref(), Some("list"));
        assert_eq!(
            next_console_line(&mut reader).as_deref(),
            Some("stop"),
            "the invalid line is skipped and reading continues"
        );
        assert_eq!(
            next_console_line(&mut reader),
            None,
            "only real EOF ends the console"
        );
    }

    #[test]
    fn a_trailing_invalid_line_ends_at_eof_without_spinning() {
        // The invalid bytes are consumed with their line, so the reader cannot
        // loop on them: the next call sees EOF and returns.
        let input: &[u8] = b"\x80\x81";
        let mut reader = BufReader::new(input);
        assert_eq!(next_console_line(&mut reader), None);
    }

    #[test]
    fn an_invalid_line_does_not_swallow_the_console_channel() {
        // The reader task runs until EOF; an invalid line in the middle must
        // not end it, so `list` and `stop` both reach the channel.
        let input: &[u8] = b"list\n\xff\nstop\n";
        let mut reader = BufReader::new(input);
        let mut lines = Vec::new();
        while let Some(line) = next_console_line(&mut reader) {
            lines.push(line);
        }
        assert_eq!(lines, vec!["list".to_owned(), "stop".to_owned()]);
    }
}
