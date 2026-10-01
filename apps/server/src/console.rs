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
//!
//! Neither is an unbounded line (AUDIT-19 G-11): `read_until` grew the buffer
//! until the input ended or memory did, and a piped megabyte was read in full
//! only for the dispatcher to refuse it. [`MAX_CONSOLE_LINE_BYTES`] bounds it
//! at a multiple of the longest command the dispatcher accepts, and an
//! over-long line is reported by name and skipped — the rest of *that* line
//! is discarded, so the next line still runs.

use std::io::BufRead;

/// Longest console line read, in bytes, before the line is refused by name.
///
/// Read is `read_until`-shaped but bounded: a line may not grow past this, so
/// a pipe that never sends a newline cannot make the reader allocate without
/// limit. The number is chosen against
/// [`mc_server::commands::MAX_COMMAND_CHARS`] (32 500 characters — the
/// dispatcher's own ceiling, itself the vanilla client's chat-command cap):
/// this is a quarter of it, which no command a human types or pastes reaches,
/// so the bound only ever fires on input that was going to be refused anyway —
/// earlier, cheaper, and visible in the log. The `const` assertion in the
/// tests keeps the two from crossing.
pub const MAX_CONSOLE_LINE_BYTES: usize = 8192;

/// Read one console line: the next non-empty, UTF-8 line, without its
/// terminator.
///
/// Returns `None` on EOF (clean shell quit, piped input exhausted). Empty
/// lines are skipped — sending them to the dispatcher would only answer
/// "unknown command". A final line without a trailing newline still
/// counts: EOF terminates the line, it does not discard it.
///
/// A line whose bytes are not UTF-8 is skipped after a warning, and reading
/// continues with the next line (AUDIT-19 C19-M4). A line longer than
/// [`MAX_CONSOLE_LINE_BYTES`] is skipped after its own named warning (AUDIT-19
/// G-11), and the remainder of that line is consumed so the next read starts
/// at a line boundary rather than mid-paste. Only EOF and a genuine read error
/// end input.
pub fn next_console_line(reader: &mut impl BufRead) -> Option<String> {
    let mut raw = Vec::new();
    loop {
        raw.clear();
        // `read_until` on bytes, not `read_line` on a `String`: the byte-level
        // call reports "this line is not UTF-8" as a line we can skip, while
        // `read_line` reports it as `InvalidData` — indistinguishable from a
        // fatal error once it reaches a `match`.
        //
        // The read is bounded by hand rather than by `take`, because an
        // over-long line still has to be *drained* to its newline: taking the
        // cap would leave the tail to be read as the next command.
        loop {
            // `fill_buf`'s borrow ends here: the position is computed, the
            // buffer is dropped, and only then is the reader consumed.
            let (take, newline) = match reader.fill_buf() {
                // EOF: a clean shell quit or an exhausted pipe.
                Ok([]) => {
                    return if raw.is_empty() {
                        None
                    } else {
                        finish_line(&raw)
                    };
                }
                Ok(buffer) => {
                    let newline = buffer.iter().position(|byte| *byte == b'\n');
                    (newline.map_or(buffer.len(), |index| index + 1), newline)
                }
                Err(error) => {
                    // A real read error still ends input, but not silently: the
                    // old bare `return None` made a dead console look exactly like
                    // a clean quit.
                    tracing::warn!(%error, "console read failed; ending console input");
                    return None;
                }
            };
            if raw.len() + take > MAX_CONSOLE_LINE_BYTES {
                // Over the cap: name it, drain the rest of *this* line so the
                // next read starts at a line boundary, and skip the line. The
                // drain starts at the newline in this very buffer when there
                // is one, so a line that ends after the tail was buffered is
                // not double-counted.
                reader.consume(take);
                let tail = skip_to_newline(reader);
                tracing::warn!(
                    bytes = raw.len() + take + tail,
                    limit = MAX_CONSOLE_LINE_BYTES,
                    "console line is longer than the limit; skipping it"
                );
                break;
            }
            // Move the bytes out before consuming them: `consume` needs the
            // reader mutably, and the slice borrows it. The borrow ends with
            // this statement, so the two calls cannot overlap.
            raw.extend_from_slice(&reader.fill_buf().expect("just filled")[..take]);
            reader.consume(take);
            if newline.is_some() {
                if let Some(line) = finish_line(&raw) {
                    return Some(line);
                }
                // An empty or whitespace-only line: read the next one.
                break;
            }
        }
    }
}

/// Discard input up to and including the next newline, returning how much.
///
/// Used for the tail of an over-long line, so the bytes are never accumulated
/// anywhere — this is what keeps the reader's memory use bounded by
/// [`MAX_CONSOLE_LINE_BYTES`] rather than by the input. EOF ends the discard,
/// exactly as the old `read_until` ended a trailing partial line.
fn skip_to_newline(reader: &mut impl BufRead) -> usize {
    let mut skipped = 0usize;
    loop {
        match reader.fill_buf() {
            Ok([]) => return skipped,
            Ok(buffer) => {
                let newline = buffer.iter().position(|byte| *byte == b'\n');
                let take = newline.map_or(buffer.len(), |index| index + 1);
                reader.consume(take);
                skipped += take;
                if newline.is_some() {
                    return skipped;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "console read failed while discarding an over-long line");
                return skipped;
            }
        }
    }
}

/// Trim a raw line's terminator and whitespace, dropping it when it is empty.
fn finish_line(raw: &[u8]) -> Option<String> {
    let line = match std::str::from_utf8(raw) {
        Ok(line) => line,
        Err(error) => {
            // The line was consumed, so the next iteration reads the following
            // one; this cannot spin on the same bytes.
            tracing::warn!(
                valid_up_to = error.valid_up_to(),
                "console line is not UTF-8 (a GBK/cp936 byte?); skipping it"
            );
            return None;
        }
    };
    let text = line.trim_end_matches(['\r', '\n']).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
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
    use super::{MAX_CONSOLE_LINE_BYTES, next_console_line};
    use std::io::BufReader;

    #[test]
    fn the_line_cap_stays_below_the_dispatcher_ceiling_it_backs() {
        // AUDIT-19 G-11: the cap exists so a pipe without a newline cannot grow
        // the buffer without bound. It must stay under the dispatcher's own
        // limit, or the reader would refuse lines the dispatcher accepts.
        const {
            assert!(
                MAX_CONSOLE_LINE_BYTES < mc_server::commands::MAX_COMMAND_CHARS,
                "the console cap must be tighter than the command ceiling"
            );
        }
        assert_eq!(MAX_CONSOLE_LINE_BYTES, 8192);
    }

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

    #[test]
    fn an_over_long_line_is_refused_by_name_and_the_next_line_still_runs() {
        // AUDIT-19 G-11: `read_until` grew without limit, so a piped 2 MiB
        // "line" was read in full and only then refused by the dispatcher.
        // The reader now caps it, says so, discards the rest of that line and
        // keeps reading at the next line boundary. Neutralise the cap (use
        // `usize::MAX`) and the third assertion goes red: the tail of the
        // over-long line would be returned as a command.
        let oversized = "x".repeat(MAX_CONSOLE_LINE_BYTES * 3);
        let input = format!("list\n{oversized}\nstop\n");
        let mut reader = BufReader::new(input.as_bytes());
        assert_eq!(next_console_line(&mut reader).as_deref(), Some("list"));
        assert_eq!(
            next_console_line(&mut reader).as_deref(),
            Some("stop"),
            "the over-long line is skipped whole, and `stop` is still a command"
        );
        assert_eq!(next_console_line(&mut reader), None);

        // Same at EOF without a trailing newline: the partial line is
        // surfaced, not lost — `read_until` semantics survive the rewrite.
        let mut reader = BufReader::new("save-all".as_bytes());
        assert_eq!(next_console_line(&mut reader).as_deref(), Some("save-all"));

        // One byte over the cap with no newline anywhere: refused, and the
        // read ends at EOF rather than looping.
        let input = vec![b'y'; MAX_CONSOLE_LINE_BYTES + 1];
        let mut reader = BufReader::new(input.as_slice());
        assert_eq!(next_console_line(&mut reader), None);
    }

    #[test]
    fn the_buffer_never_holds_more_than_the_cap() {
        // A stand-in for an endless stdin: a reader that hands out blocks as
        // they arrive, fed by a thread. The cap is a *memory* bound, so this
        // measures what the reader accumulated instead of trusting the code
        // path — and it keeps the over-long line from being *read in full* the
        // way `read_until` used to, which is the whole point.
        use std::sync::mpsc;

        /// Hands out queued blocks, keeping whatever did not fit, like a pipe
        /// does — a reader that dropped the tail would silently eat the bytes
        /// that follow the over-long line.
        struct Preview {
            blocks: mpsc::Receiver<Vec<u8>>,
            pending: Vec<u8>,
            held: usize,
            ended: bool,
        }

        impl std::io::Read for Preview {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.pending.is_empty() && !self.ended {
                    match self.blocks.recv() {
                        Ok(block) => self.pending = block,
                        Err(_) => self.ended = true,
                    }
                }
                let take = out.len().min(self.pending.len());
                out[..take].copy_from_slice(&self.pending[..take]);
                self.pending.drain(..take);
                self.held += take;
                Ok(take)
            }
        }

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let block = vec![b'z'; 4096];
        let feeder = std::thread::spawn(move || {
            // 16 blocks = 64 KiB with no newline, then "stop" and EOF: an
            // endless-looking line that really does end.
            for _ in 0..16 {
                if tx.send(block.clone()).is_err() {
                    return;
                }
            }
            let _ = tx.send(b"list\nstop\n".to_vec());
        });

        let mut reader = std::io::BufReader::with_capacity(
            MAX_CONSOLE_LINE_BYTES,
            Preview {
                blocks: rx,
                pending: Vec::new(),
                held: 0,
                ended: false,
            },
        );
        let mut lines = Vec::new();
        while let Some(line) = next_console_line(&mut reader) {
            lines.push(line);
        }
        assert_eq!(
            lines,
            vec!["stop".to_owned()],
            "the over-long line is discarded whole; the next real line still runs"
        );
        let held = reader.get_ref().held;
        assert!(
            held > MAX_CONSOLE_LINE_BYTES,
            "the whole over-long line was consumed (and dropped, not accumulated); held={held}"
        );
        assert!(
            reader.get_ref().pending.is_empty(),
            "the reader kept every byte the feeder wrote"
        );
        feeder.join().expect("the feeder finishes");
    }
}
