//! The terminal: what Vario prints and the questions it asks.

use std::io::{self, BufRead, IsTerminal, Write};

pub struct Ui {
    input: Box<dyn BufRead>,
    out: Box<dyn Write>,
    /// A person can answer questions. Without one, Vario changes nothing unless
    /// `--yes` is given.
    pub interactive: bool,
}

impl Ui {
    pub fn stdio() -> Self {
        Ui {
            interactive: io::stdin().is_terminal() && io::stdout().is_terminal(),
            input: Box::new(io::stdin().lock()),
            out: Box::new(io::stdout()),
        }
    }

    /// Print one line. A closed output (`vario doctor | head`) is not an error.
    pub fn say(&mut self, line: impl AsRef<str>) {
        let _ = writeln!(self.out, "{}", line.as_ref());
    }

    /// Ask a yes/no question. An empty answer or the end of input gives `default`;
    /// "y" or "yes" is yes, anything else is no.
    pub fn confirm(&mut self, question: &str, default: bool) -> bool {
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        let _ = write!(self.out, "{question} {hint} ");
        let _ = self.out.flush();
        let mut answer = String::new();
        if self.input.read_line(&mut answer).unwrap_or(0) == 0 {
            self.say("");
            return default;
        }
        match answer.trim().to_lowercase().as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        }
    }
}

#[cfg(test)]
pub mod testing {
    use std::cell::RefCell;
    use std::io::{self, Write};
    use std::rc::Rc;

    use super::Ui;

    /// Output the test can read after handing a writer to the `Ui`.
    #[derive(Clone, Default)]
    pub struct Captured(Rc<RefCell<Vec<u8>>>);

    impl Captured {
        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.borrow()).into_owned()
        }
    }

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A `Ui` that answers with `answers`, one per line, and records what is printed.
    pub fn scripted(answers: &str, interactive: bool) -> (Ui, Captured) {
        let captured = Captured::default();
        let input = io::Cursor::new(answers.as_bytes().to_vec());
        let ui = Ui {
            input: Box::new(input),
            out: Box::new(captured.clone()),
            interactive,
        };
        (ui, captured)
    }

    #[test]
    fn answers_and_defaults() {
        let (mut ui, out) = scripted("y\n\nno\nOui\n", true);
        assert!(ui.confirm("A?", false));
        assert!(ui.confirm("B?", true));
        assert!(!ui.confirm("C?", true));
        assert!(!ui.confirm("D?", true), "only y and yes mean yes");
        assert!(ui.confirm("E?", true), "end of input gives the default");
        assert!(out.text().starts_with("A? [y/N] B? [Y/n] "));
    }
}
