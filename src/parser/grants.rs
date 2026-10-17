//! The `os/` grant entries of the `app` and `orbital` brackets (`docs/Almadar_Sandbox.md` Contracts §3) and
//! the app's `deployment` map. One value grammar serves both brackets.

use super::*;
use orbital_core_base::os_grants::{
    DesktopKind, FsGrant, FsMode, NetGrant, NetKind, OsGrants, StdioKind, SysKind,
};

pub(crate) const GRANT_KEYS: &[&str] = &["fs", "exec", "net", "env", "stdio", "sys", "desktop"];

const ENTRY_INVALID: &str = "ORB_OS_GRANT_ENTRY_INVALID";

fn words<T: Copy>(table: &[T], name: impl Fn(T) -> &'static str) -> Vec<&'static str> {
    table.iter().map(|k| name(*k)).collect()
}

impl Parser {
    /// The value of one grant key (`fs`, `exec`, `net`, `env`, `stdio`, `sys`, `desktop`) into `grants`; the
    /// `:` is already consumed.
    pub(crate) fn parse_grant_value(
        &mut self,
        key: &str,
        grants: &mut OsGrants,
    ) -> Result<(), ParseError> {
        match key {
            "fs" => {
                for (glob, modes) in self.parse_grant_map(key, FsMode::ALL, |k| k.name())? {
                    grants.fs.push(FsGrant { glob, modes });
                }
            }
            "net" => {
                for (target, kinds) in self.parse_grant_map(key, NetKind::ALL, |k| k.name())? {
                    grants.net.push(NetGrant { target, kinds });
                }
            }
            "exec" => grants.exec = self.parse_string_items(key)?,
            "env" => grants.env = self.parse_string_items(key)?,
            "stdio" => grants.stdio = self.parse_grant_words(key, StdioKind::ALL, |k| k.name())?,
            "sys" => grants.sys = self.parse_grant_words(key, SysKind::ALL, |k| k.name())?,
            "desktop" => {
                grants.desktop = self.parse_grant_words(key, DesktopKind::ALL, |k| k.name())?
            }
            _ => unreachable!("GRANT_KEYS covers every key"),
        }
        Ok(())
    }

    /// One word of a closed set, spelled in any installed language.
    fn parse_grant_word<T: Copy>(
        &mut self,
        key: &str,
        table: &[T],
        name: impl Fn(T) -> &'static str + Copy,
    ) -> Result<T, ParseError> {
        let span = self.peek_span();
        let allowed = words(table, name);
        let chosen = self.parse_option_word(key, ENTRY_INVALID, &allowed)?;
        table
            .iter()
            .copied()
            .find(|k| name(*k) == chosen)
            .ok_or(ParseError::InvalidOptionValue {
                code: ENTRY_INVALID,
                key: key.to_string(),
                message: format!("`{chosen}` is not one of {}", allowed.join(" | ")),
                span,
            })
    }

    /// `[read, write]` — a list of words of one closed set.
    fn parse_grant_words<T: Copy>(
        &mut self,
        key: &str,
        table: &[T],
        name: impl Fn(T) -> &'static str + Copy,
    ) -> Result<Vec<T>, ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                return Ok(out);
            }
            out.push(self.parse_grant_word(key, table, name)?);
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
    }

    /// `{ "target": [word, …], … }` — entries in source order.
    fn parse_grant_map<T: Copy>(
        &mut self,
        key: &str,
        table: &[T],
        name: impl Fn(T) -> &'static str + Copy,
    ) -> Result<Vec<(String, Vec<T>)>, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                return Ok(out);
            }
            let target = self.parse_option_string(key)?;
            self.expect_kind(&TokenKind::Colon, "':' after a grant target")?;
            let words = self.parse_grant_words(key, table, name)?;
            out.push((target, words));
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
    }

    /// `{ unit: [Orbital, …], … }` — the `deployment` value, units in source order.
    pub(crate) fn parse_deployment(&mut self) -> Result<Vec<(String, Vec<String>)>, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                return Ok(out);
            }
            let span = self.peek_span();
            let unit = match self.peek().clone() {
                TokenKind::String(s) => {
                    self.advance();
                    s
                }
                _ => self.expect_ident()?,
            };
            if out.iter().any(|(u, _)| *u == unit) {
                return Err(ParseError::DuplicateOption {
                    key: unit,
                    context: "deployment",
                    span,
                });
            }
            self.expect_kind(&TokenKind::Colon, "':' after a deployment unit")?;
            self.expect_kind(&TokenKind::LBracket, "[")?;
            let mut orbitals = Vec::new();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::RBracket) {
                    self.advance();
                    break;
                }
                orbitals.push(self.expect_ident()?);
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
            }
            out.push((unit, orbitals));
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
    }
}
