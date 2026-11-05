use std::fmt::{self, Write};

pub struct Formatter<'a> {
    out: &'a mut String,
    indent_size: usize,
    indent_level: usize,
}

impl<'a> Formatter<'a> {
    pub fn new(out: &'a mut String, indent_size: usize) -> Self {
        Self {
            out,
            indent_size,
            indent_level: 0,
        }
    }

    // TODO: Refactor this.
    pub fn block<F>(&mut self, heading: &str, semicolon: bool, f: F) -> fmt::Result
    where
        F: FnOnce(&mut Self) -> fmt::Result,
    {
        writeln!(self, "{} {{", heading)?;

        self.indent(f)?;

        writeln!(self, "{}", if semicolon { "};" } else { "}" })?;

        Ok(())
    }

    pub fn indent<F>(&mut self, f: F) -> fmt::Result
    where
        F: FnOnce(&mut Self) -> fmt::Result,
    {
        self.indent_level += 1;

        let result = f(self);
        self.indent_level -= 1;
        result
    }

    #[inline]
    fn push_indentation(&mut self) {
        if self.indent_level > 0 {
            let indentation = " ".repeat(self.indent_level * self.indent_size);

            self.out.push_str(&indentation);
        }
    }
}

impl<'a> Write for Formatter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut lines = s.lines().peekable();

        while let Some(line) = lines.next() {
            if self.out.ends_with('\n') && !line.is_empty() {
                self.push_indentation();
            }

            self.out.push_str(line);

            if lines.peek().is_some() || s.ends_with('\n') {
                self.out.push('\n');
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_indent_after_error() {
        let mut text = String::new();
        let mut fmt = Formatter::new(&mut text, 4);
        assert!(fmt.indent(|_| Err(fmt::Error)).is_err());
        assert_eq!(fmt.indent_level, 0);
    }

    #[test]
    fn formats_nested_blocks() {
        let mut text = String::new();
        let mut fmt = Formatter::new(&mut text, 2);
        fmt.block("outer", false, |fmt| writeln!(fmt, "value"))
            .unwrap();
        assert_eq!(text, "outer {\n  value\n}\n");
    }
}
