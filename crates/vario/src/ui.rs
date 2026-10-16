//! The terminal: what Vario prints and the questions it asks.

use std::io::{self, BufRead, IsTerminal, Write};

use vario_core::lang::Lang;

pub struct Ui {
    input: Box<dyn BufRead>,
    out: Box<dyn Write>,
    /// A person can answer questions. Without one, Vario changes nothing unless
    /// `--yes` is given.
    pub interactive: bool,
    pub lang: Lang,
}

impl Ui {
    pub fn stdio(lang: Lang) -> Self {
        Ui {
            lang,
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
    /// "y", "yes", "o" or "oui" is yes, anything else is no.
    pub fn confirm(&mut self, question: &str, default: bool) -> bool {
        let hint = match (self.lang, default) {
            (Lang::En, true) => "[Y/n]",
            (Lang::En, false) => "[y/N]",
            (Lang::Fr, true) => "[O/n]",
            (Lang::Fr, false) => "[o/N]",
        };
        let _ = write!(self.out, "{question} {hint} ");
        let _ = self.out.flush();
        let mut answer = String::new();
        if self.input.read_line(&mut answer).unwrap_or(0) == 0 {
            self.say("");
            return default;
        }
        match answer.trim().to_lowercase().as_str() {
            "" => default,
            "y" | "yes" | "o" | "oui" => true,
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
    use vario_core::lang::Lang;

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

    /// An English `Ui` that answers with `answers`, one per line, and records what is
    /// printed.
    pub fn scripted(answers: &str, interactive: bool) -> (Ui, Captured) {
        scripted_in(Lang::En, answers, interactive)
    }

    pub fn scripted_in(lang: Lang, answers: &str, interactive: bool) -> (Ui, Captured) {
        let captured = Captured::default();
        let input = io::Cursor::new(answers.as_bytes().to_vec());
        let ui = Ui {
            input: Box::new(input),
            out: Box::new(captured.clone()),
            interactive,
            lang,
        };
        (ui, captured)
    }

    #[test]
    fn answers_and_defaults() {
        let (mut ui, out) = scripted("y\n\nno\nOui\nsure\n", true);
        assert!(ui.confirm("A?", false));
        assert!(ui.confirm("B?", true));
        assert!(!ui.confirm("C?", true));
        assert!(ui.confirm("D?", false), "French answers are accepted too");
        assert!(!ui.confirm("E?", true), "anything else means no");
        assert!(ui.confirm("F?", true), "end of input gives the default");
        assert!(out.text().starts_with("A? [y/N] B? [Y/n] "));

        let (mut ui, out) = scripted_in(Lang::Fr, "\n", true);
        assert!(!ui.confirm("Retirer ?", false));
        assert!(out.text().starts_with("Retirer ? [o/N] "));
    }
}
