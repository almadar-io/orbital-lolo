//! The `os/` grant entries of the `app` and `orbital` brackets (`docs/Almadar_Sandbox.md` Contracts §3) and
//! the app's `deployment` map. One value grammar serves both brackets.

use super::*;
use orbital_core_base::os_grants::{
    AnchoredExec, AnchoredFsGrant, AnchoredNetGrant, CredentialKind, DesktopKind, DevGrant, DevKind, FsGrant, FsMode, GrantAnchor,
    KvMode, NetGrant, NetKind, NetPart, OsGrants, StdioKind, SysKind,
};

pub(crate) const GRANT_KEYS: &[&str] =
    &["fs", "exec", "net", "env", "stdio", "sys", "desktop", "kv", "dev", "credentials"];

const ENTRY_INVALID: &str = "ORB_OS_GRANT_ENTRY_INVALID";

const ANCHOR_WORDS: &[&str] = &["exe", "exe-dir", "cwd", "env"];

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
            "fs" => self.parse_fs_grants(grants)?,
            "net" => self.parse_net_grants(grants)?,
            "exec" => self.parse_exec_grants(grants)?,
            "env" => grants.env = self.parse_string_items(key)?,
            "stdio" => grants.stdio = self.parse_grant_words(key, StdioKind::ALL, |k| k.name())?,
            "sys" => grants.sys = self.parse_grant_words(key, SysKind::ALL, |k| k.name())?,
            "desktop" => {
                grants.desktop = self.parse_grant_words(key, DesktopKind::ALL, |k| k.name())?
            }
            "credentials" => {
                grants.credentials = self.parse_grant_words(key, CredentialKind::ALL, |k| k.name())?
            }
            "kv" => self.parse_kv_grants(grants)?,
            "dev" => self.parse_dev_grants(grants)?,
            _ => unreachable!("GRANT_KEYS covers every key"),
        }
        Ok(())
    }

    /// Whether the next token is an anchor word (`exe`, `exe-dir`, `cwd`, `env`), in any installed language.
    fn at_anchor(&self) -> bool {
        match self.peek() {
            TokenKind::Identifier(raw) => {
                let canon = self.lang.canon_any(raw).unwrap_or(raw.as_str());
                ANCHOR_WORDS.contains(&canon)
            }
            _ => false,
        }
    }

    /// `exe`, `exe-dir`, `cwd` or `env NAME`.
    fn parse_anchor(&mut self, key: &str) -> Result<GrantAnchor, ParseError> {
        Ok(
            match self.parse_option_word(key, ENTRY_INVALID, ANCHOR_WORDS)? {
                "exe" => GrantAnchor::Exe,
                "exe-dir" => GrantAnchor::ExeDir,
                "cwd" => GrantAnchor::Cwd,
                _ => GrantAnchor::Env(self.expect_ident()?),
            },
        )
    }

    /// `[word, …]` or one bare word.
    fn parse_grant_words_or_word<T: Copy>(
        &mut self,
        key: &str,
        table: &[T],
        name: impl Fn(T) -> &'static str + Copy,
    ) -> Result<Vec<T>, ParseError> {
        if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_grant_words(key, table, name)
        } else {
            Ok(vec![self.parse_grant_word(key, table, name)?])
        }
    }

    /// `{ "glob": [modes], anchor: "glob": [modes], … }`.
    fn parse_fs_grants(&mut self, grants: &mut OsGrants) -> Result<(), ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                return Ok(());
            }
            if self.at_anchor() {
                let anchor = self.parse_anchor("fs")?;
                self.expect_kind(&TokenKind::Colon, "':' after a grant anchor")?;
                let glob = self.parse_option_string("fs")?;
                self.expect_kind(&TokenKind::Colon, "':' after a grant target")?;
                let modes = self.parse_grant_words("fs", FsMode::ALL, |k| k.name())?;
                grants.anchored.fs.push(AnchoredFsGrant {
                    anchor,
                    glob,
                    modes,
                });
            } else {
                let glob = self.parse_option_string("fs")?;
                self.expect_kind(&TokenKind::Colon, "':' after a grant target")?;
                let modes = self.parse_grant_words("fs", FsMode::ALL, |k| k.name())?;
                grants.fs.push(FsGrant { glob, modes });
            }
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
    }

    /// `{ store: [read, write], "store-glob*": [read], … }`.
    /// `dev: { "mmio:<base>+<length>": [read, write], "irq:<line>": [wait, ack] }`.
    fn parse_dev_grants(&mut self, grants: &mut OsGrants) -> Result<(), ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                return Ok(());
            }
            let span = self.peek_span();
            let TokenKind::String(target) = self.peek().clone() else {
                return Err(ParseError::InvalidOptionValue {
                    code: ENTRY_INVALID,
                    key: "dev".to_string(),
                    message: "a device target is a string: `\"mmio:0x09000000+0x1000\"` or `\"irq:33\"`".to_string(),
                    span,
                });
            };
            self.advance();
            if grants.dev.iter().any(|g| g.target == target) {
                return Err(ParseError::DuplicateOption { key: target, context: "dev", span });
            }
            self.expect_kind(&TokenKind::Colon, "':' after a grant target")?;
            let kinds = self.parse_grant_words("dev", DevKind::ALL, |k| k.name())?;
            grants.dev.push(DevGrant { target, kinds });
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
    }

    fn parse_kv_grants(&mut self, grants: &mut OsGrants) -> Result<(), ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                return Ok(());
            }
            let span = self.peek_span();
            let store = match self.peek().clone() {
                TokenKind::String(s) => {
                    self.advance();
                    s
                }
                _ => self.expect_ident()?,
            };
            if grants.kv.contains_key(&store) {
                return Err(ParseError::DuplicateOption {
                    key: store,
                    context: "kv",
                    span,
                });
            }
            self.expect_kind(&TokenKind::Colon, "':' after a grant target")?;
            let modes = self.parse_grant_words("kv", KvMode::ALL, |k| k.name())?;
            grants.kv.insert(store, modes);
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
    }

    /// `["cmd", env NAME, exe-dir "cmd", …]`.
    fn parse_exec_grants(&mut self, grants: &mut OsGrants) -> Result<(), ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                return Ok(());
            }
            if self.at_anchor() {
                let anchor = self.parse_anchor("exec")?;
                let path = match self.peek().clone() {
                    TokenKind::String(path) => {
                        self.advance();
                        Some(path)
                    }
                    _ => None,
                };
                grants.anchored.exec.push(AnchoredExec { anchor, path });
            } else {
                grants.exec.push(self.parse_option_string("exec")?);
            }
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
    }

    /// One half of an anchored net target: `env NAME`, a quoted host or a port number.
    fn parse_net_part(&mut self) -> Result<NetPart, ParseError> {
        if self.at_anchor() {
            return Ok(NetPart::Anchor(self.parse_anchor("net")?));
        }
        match self.peek().clone() {
            TokenKind::Integer(n) => {
                self.advance();
                Ok(NetPart::Literal(n.to_string()))
            }
            _ => Ok(NetPart::Literal(self.parse_option_string("net")?)),
        }
    }

    /// `{ "host:port": [kinds], env HOST, env PORT: [kinds], "0.0.0.0", env PORT: listen, … }`.
    fn parse_net_grants(&mut self, grants: &mut OsGrants) -> Result<(), ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                return Ok(());
            }
            let first = self.parse_net_part()?;
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
                self.skip_newlines();
                let port = self.parse_net_part()?;
                self.expect_kind(&TokenKind::Colon, "':' after a grant target")?;
                let kinds = self.parse_grant_words_or_word("net", NetKind::ALL, |k| k.name())?;
                grants.anchored.net.push(AnchoredNetGrant {
                    host: first,
                    port,
                    kinds,
                });
            } else {
                let NetPart::Literal(target) = first else {
                    return Err(ParseError::InvalidOptionValue {
                        code: ENTRY_INVALID,
                        key: "net".to_string(),
                        message: "an anchored net target names its host and port: `env HOST, env PORT: [listen]`".to_string(),
                        span: self.peek_span(),
                    });
                };
                self.expect_kind(&TokenKind::Colon, "':' after a grant target")?;
                let kinds = self.parse_grant_words_or_word("net", NetKind::ALL, |k| k.name())?;
                grants.net.push(NetGrant { target, kinds });
            }
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
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

#[cfg(test)]
mod tests {
    use crate::{format_program, parse_str};
    use orbital_core_base::os_grants::{
        AnchoredExec, AnchoredFsGrant, AnchoredNetGrant, FsGrant, FsMode, GrantAnchor, NetKind,
        NetPart,
    };

    const ANCHORED: &str = "app a [\n  version: \"1.0.0\",\n  exec: [env ORB_BUN_PATH, exe-dir \"bun\", \"bun\"],\n  fs: { \"/data/**\": [read], exe-dir: \"agent/**\": [read], env ORB_AGENT_DIR: \"**\": [read], cwd: \"docs/**\": [read, meta] },\n  net: { \"db:5432\": [tcp], env HOST, env PORT: listen, \"0.0.0.0\", env PORT: [listen] }\n]\norbital O {\n  entity E {\n    id : string!\n  }\n}\n";

    #[test]
    fn anchored_grants_parse_into_the_typed_record_beside_the_plain_ones() {
        let program = parse_str(ANCHORED).unwrap_or_else(|e| panic!("{e}"));
        let g = &program.app.os_grants;
        assert_eq!(g.exec, ["bun"]);
        assert_eq!(
            g.anchored.exec,
            [
                AnchoredExec {
                    anchor: GrantAnchor::Env("ORB_BUN_PATH".into()),
                    path: None
                },
                AnchoredExec {
                    anchor: GrantAnchor::ExeDir,
                    path: Some("bun".into())
                },
            ]
        );
        assert_eq!(
            g.fs,
            [FsGrant {
                glob: "/data/**".into(),
                modes: vec![FsMode::Read]
            }],
            "control: a quoted glob stays plain"
        );
        assert_eq!(
            g.anchored.fs,
            [
                AnchoredFsGrant {
                    anchor: GrantAnchor::ExeDir,
                    glob: "agent/**".into(),
                    modes: vec![FsMode::Read]
                },
                AnchoredFsGrant {
                    anchor: GrantAnchor::Env("ORB_AGENT_DIR".into()),
                    glob: "**".into(),
                    modes: vec![FsMode::Read]
                },
                AnchoredFsGrant {
                    anchor: GrantAnchor::Cwd,
                    glob: "docs/**".into(),
                    modes: vec![FsMode::Read, FsMode::Meta]
                },
            ]
        );
        assert_eq!(g.net.len(), 1);
        assert_eq!(
            g.anchored.net,
            [
                AnchoredNetGrant {
                    host: NetPart::Anchor(GrantAnchor::Env("HOST".into())),
                    port: NetPart::Anchor(GrantAnchor::Env("PORT".into())),
                    kinds: vec![NetKind::Listen],
                },
                AnchoredNetGrant {
                    host: NetPart::Literal("0.0.0.0".into()),
                    port: NetPart::Anchor(GrantAnchor::Env("PORT".into())),
                    kinds: vec![NetKind::Listen],
                },
            ]
        );
        assert!(g.entry_problems().is_empty(), "{:?}", g.entry_problems());
    }

    #[test]
    fn the_exe_anchor_parses_as_a_command_and_formats_back() {
        let source = ANCHORED.replace("exec: [env ORB_BUN_PATH,", "exec: [exe, env ORB_BUN_PATH,");
        let program = parse_str(&source).unwrap_or_else(|e| panic!("{e}"));
        assert!(program.app.os_grants.anchored.exec.contains(&AnchoredExec { anchor: GrantAnchor::Exe, path: None }));
        let text = format_program(&program);
        assert!(text.contains("exec: [") && text.contains("exe,"), "{text}");
        let again = parse_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(again.app.os_grants, program.app.os_grants);
        assert!(parse_str(ANCHORED).unwrap().app.os_grants.anchored.exec.iter().all(|e| e.anchor != GrantAnchor::Exe), "control");
    }

    #[test]
    fn anchored_grants_format_back_to_source_that_parses_to_the_same_record() {
        let program = parse_str(ANCHORED).unwrap_or_else(|e| panic!("{e}"));
        let text = format_program(&program);
        assert!(
            text.contains("exe-dir: \"agent/**\": [read]")
                && text.contains("env ORB_BUN_PATH")
                && text.contains("env HOST, env PORT: [listen]"),
            "{text}"
        );
        let again = parse_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(again.app.os_grants, program.app.os_grants);
    }

    #[test]
    fn a_lone_anchor_as_a_net_target_and_an_unknown_anchor_word_are_refused() {
        let lone = ANCHORED.replace("env HOST, env PORT: listen, ", "env HOST: [listen], ");
        assert!(
            parse_str(&lone).is_err(),
            "a net target needs both host and port"
        );
        let unknown = ANCHORED.replace("exe-dir: \"agent/**\"", "home-dir: \"agent/**\"");
        assert!(
            parse_str(&unknown).is_err(),
            "control: only the declared anchor words"
        );
    }

    #[test]
    fn a_credentials_grant_parses_formats_and_stays_closed_when_undeclared() {
        use orbital_core_base::os_grants::CredentialKind;
        let declared = "app a [\n  version: \"1.0.0\",\n  credentials: [admin]\n]\norbital O {\n  entity E {\n    id : string!\n  }\n}\n";
        let program = parse_str(declared).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(program.app.os_grants.credentials, [CredentialKind::Admin]);
        assert!(program.app.os_grants.credentials_allow(CredentialKind::Admin));
        let text = format_program(&program);
        assert!(text.contains("credentials: [admin]"), "{text}");
        let again = parse_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(again.app.os_grants, program.app.os_grants);

        let control = parse_str(&declared.replace("credentials: [admin]", "stdio: [out]").replace(",\n  stdio", ",\n  stdio"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(control.app.os_grants.credentials.is_empty(), "control: no declaration grants nothing");
        assert!(!control.app.os_grants.credentials_allow(CredentialKind::Admin));
        let unknown = parse_str(&declared.replace("[admin]", "[superuser]"));
        assert!(unknown.is_err(), "an unknown credential kind is refused");
    }
}
