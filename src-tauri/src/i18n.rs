//! English text for errors, notices, the tray menu and Discord messages: the source string is the key.

use std::fmt::Display;

pub const LANGS: [&str; 1] = ["en"];

pub fn set(_pref: &str) {}

pub fn current() -> &'static str {
    "en"
}

fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = text.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

pub fn tr(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(key, args)
}

pub fn trn(n: u64, one: &'static str, other: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let text = if n == 1 { one } else { other };
    fill(&fill(text, &[("n", &n)]), args)
}

macro_rules! tr {
    ($key:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::tr($key, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}

macro_rules! trn {
    ($count:expr, $one:literal, $other:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::trn(($count) as u64, $one, $other, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("{a} and {b}", &[("a", &1), ("b", &"x")]), "1 and x");
        assert_eq!(fill("no placeholder", &[]), "no placeholder");
        assert_eq!(fill("{a} {a}", &[("a", &2)]), "2 2");
    }

    #[test]
    fn plural_picks_the_english_form() {
        assert_eq!(trn(1, "{n} client", "{n} clients", &[]), "1 client");
        assert_eq!(trn(0, "{n} client", "{n} clients", &[]), "0 clients");
        assert_eq!(trn(5, "{n} client", "{n} clients", &[("x", &"y")]), "5 clients");
    }
}
